//! The raw functions an extension reaches through the `atoll` table of the prelude. Every one of
//! them checks what the extension is allowed to do before it asks the browser.

use std::cell::{Cell, RefCell};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use mlua::{Lua, LuaSerdeExt, Table, Value};

use crate::content::{self, RunAt, ScriptWorld};
use crate::host::{
    HttpRequest, LogLevel, PendingId, PluginHost, PluginId, missing_host, missing_permission,
};
use crate::manifest::{Manifest, Permission};
use crate::pattern::UrlPattern;

/// How long an extension may sleep in one call.
const MAX_SLEEP: u64 = 5 * 60 * 1000;

/// A command an extension added to the palette.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    pub id: String,
    pub title: String,
    pub shortcut: Option<String>,
}

/// Everything the raw functions of one extension need. Holds no Lua values, so dropping the
/// extension frees its state.
pub(crate) struct PluginContext {
    pub(crate) id: PluginId,
    pub(crate) manifest: Manifest,
    pub(crate) granted: Vec<Permission>,
    pub(crate) host: Rc<dyn PluginHost>,
    pub(crate) next_pending: Rc<Cell<u64>>,
    pub(crate) commands: RefCell<Vec<Command>>,
    /// The scripts the extension put into pages, by the name the browser knows them by.
    pub(crate) scripts: RefCell<BTreeSet<String>>,
}

impl PluginContext {
    /// Whether the user granted `permission` and the manifest asked for it.
    pub(crate) fn allows(&self, permission: Permission) -> bool {
        self.manifest.allows(permission) && self.granted.contains(&permission)
    }

    fn require(&self, permission: Permission) -> mlua::Result<()> {
        if self.allows(permission) {
            Ok(())
        } else {
            Err(mlua::Error::runtime(missing_permission(permission)))
        }
    }

    fn require_site(&self, url: &str) -> mlua::Result<()> {
        if self.manifest.covers(url) {
            Ok(())
        } else {
            Err(mlua::Error::runtime(missing_host(url)))
        }
    }

    /// The page the user is looking at, if the extension may see it.
    fn page(&self) -> mlua::Result<String> {
        self.require(Permission::Pages)?;
        let url = self
            .host
            .current_page_url(&self.id)
            .ok_or_else(|| mlua::Error::runtime("gerade ist keine Seite offen"))?;
        self.require_site(&url)?;
        Ok(url)
    }

    fn pending(&self) -> PendingId {
        let id = self.next_pending.get() + 1;
        self.next_pending.set(id);
        PendingId(id)
    }
}

/// Builds the table the prelude is given.
pub(crate) fn native_table(lua: &Lua, context: &Rc<PluginContext>) -> mlua::Result<Table> {
    let native = lua.create_table()?;
    native.set("id", context.id.clone())?;
    native.set("version", context.manifest.version.clone())?;

    let held = context.clone();
    native.set(
        "log",
        lua.create_function(move |_, (level, message): (String, String)| {
            let level = match level.as_str() {
                "warn" => LogLevel::Warn,
                "error" => LogLevel::Error,
                _ => LogLevel::Info,
            };
            held.host.log(&held.id, level, &message);
            Ok(())
        })?,
    )?;

    let held = context.clone();
    native.set(
        "report",
        lua.create_function(move |_, message: String| {
            held.host.report(&held.id, &message);
            Ok(())
        })?,
    )?;

    let held = context.clone();
    native.set(
        "toast",
        lua.create_function(move |_, message: String| {
            held.host.toast(&held.id, &message);
            Ok(())
        })?,
    )?;

    let held = context.clone();
    native.set(
        "tabs",
        lua.create_function(move |lua, ()| {
            held.require(Permission::Tabs)?;
            lua.to_value(&held.host.tabs(&held.id))
        })?,
    )?;

    let held = context.clone();
    native.set(
        "open_tab",
        lua.create_function(move |_, (url, background): (String, bool)| {
            held.require(Permission::Tabs)?;
            held.host
                .open_tab(&held.id, &url, background)
                .map_err(mlua::Error::external)
        })?,
    )?;

    let held = context.clone();
    native.set(
        "close_tab",
        lua.create_function(move |_, tab: i64| {
            held.require(Permission::Tabs)?;
            held.host
                .close_tab(&held.id, tab)
                .map_err(mlua::Error::external)
        })?,
    )?;

    let held = context.clone();
    native.set(
        "activate_tab",
        lua.create_function(move |_, tab: i64| {
            held.require(Permission::Tabs)?;
            held.host
                .activate_tab(&held.id, tab)
                .map_err(mlua::Error::external)
        })?,
    )?;

    let held = context.clone();
    native.set(
        "storage_get",
        lua.create_function(move |_, key: String| {
            held.require(Permission::Storage)?;
            Ok(held.host.stored(&held.id, &key))
        })?,
    )?;

    let held = context.clone();
    native.set(
        "storage_set",
        lua.create_function(move |_, (key, value): (String, Option<String>)| {
            held.require(Permission::Storage)?;
            held.host.store(&held.id, &key, value.as_deref());
            Ok(())
        })?,
    )?;

    let held = context.clone();
    native.set(
        "storage_keys",
        lua.create_function(move |_, ()| {
            held.require(Permission::Storage)?;
            Ok(held.host.stored_keys(&held.id))
        })?,
    )?;

    let held = context.clone();
    native.set(
        "register_command",
        lua.create_function(
            move |_, (id, title, shortcut): (String, String, Option<String>)| {
                let command = Command {
                    id,
                    title,
                    shortcut,
                };
                let mut commands = held.commands.borrow_mut();
                match commands
                    .iter_mut()
                    .find(|candidate| candidate.id == command.id)
                {
                    Some(existing) => *existing = command,
                    None => commands.push(command),
                }
                drop(commands);
                held.host.commands_changed(&held.id);
                Ok(())
            },
        )?,
    )?;

    let held = context.clone();
    native.set(
        "unregister_command",
        lua.create_function(move |_, id: String| {
            held.commands
                .borrow_mut()
                .retain(|command| command.id != id);
            held.host.commands_changed(&held.id);
            Ok(())
        })?,
    )?;

    let held = context.clone();
    native.set(
        "sleep",
        lua.create_function(move |_, milliseconds: u64| {
            let pending = held.pending();
            held.host
                .sleep(&held.id, pending, milliseconds.min(MAX_SLEEP));
            Ok(pending.0)
        })?,
    )?;

    let held = context.clone();
    native.set(
        "page_url",
        lua.create_function(move |_, ()| held.page().map(Some))?,
    )?;

    let held = context.clone();
    native.set(
        "evaluate",
        lua.create_function(move |_, (code, world): (String, Option<String>)| {
            held.page()?;
            let world = world
                .as_deref()
                .map_or(Some(ScriptWorld::Isolated), ScriptWorld::parse)
                .ok_or_else(|| mlua::Error::runtime("world ist \"isolated\" oder \"page\""))?;
            let pending = held.pending();
            held.host.evaluate(&held.id, pending, code, world);
            Ok(pending.0)
        })?,
    )?;

    let held = context.clone();
    native.set(
        "inject_css",
        lua.create_function(move |_, (css, world): (String, Option<String>)| {
            held.page()?;
            let world = world
                .as_deref()
                .map_or(Some(ScriptWorld::Isolated), ScriptWorld::parse)
                .ok_or_else(|| mlua::Error::runtime("world ist \"isolated\" oder \"page\""))?;
            let pending = held.pending();
            held.host
                .evaluate(&held.id, pending, content::stylesheet(&css), world);
            Ok(pending.0)
        })?,
    )?;

    let held = context.clone();
    native.set(
        "content_register",
        lua.create_function(
            move |_,
                  (id, matches, css, js, world, run_at): (
                String,
                Vec<String>,
                String,
                String,
                Option<String>,
                Option<String>,
            )| {
                held.require(Permission::Pages)?;
                let wanted = matches
                    .iter()
                    .map(|pattern| {
                        UrlPattern::parse(pattern).map_err(|error| {
                            mlua::Error::runtime(format!("\"{pattern}\": {error}"))
                        })
                    })
                    .collect::<mlua::Result<Vec<_>>>()?;
                if wanted.is_empty() {
                    return Err(mlua::Error::runtime(
                        "ein Seiten-Skript braucht mindestens ein Muster unter \"matches\"",
                    ));
                }
                let world = world
                    .as_deref()
                    .map_or(Some(ScriptWorld::Isolated), ScriptWorld::parse)
                    .ok_or_else(|| mlua::Error::runtime("world ist \"isolated\" oder \"page\""))?;
                let run_at = run_at
                    .as_deref()
                    .map_or(Some(RunAt::DocumentStart), RunAt::parse)
                    .ok_or_else(|| {
                        mlua::Error::runtime("runAt ist \"documentStart\" oder \"documentEnd\"")
                    })?;
                let script = content::build(content::Request {
                    plugin: &held.id,
                    id: &id,
                    allowed: &held.manifest.hosts,
                    wanted: &wanted,
                    css: &css,
                    js: &js,
                    run_at,
                    world,
                });
                held.scripts.borrow_mut().insert(script.name.clone());
                held.host.set_content_script(&held.id, script);
                Ok(())
            },
        )?,
    )?;

    let held = context.clone();
    native.set(
        "content_unregister",
        lua.create_function(move |_, id: String| {
            let name = format!("{}/{id}", held.id);
            held.scripts.borrow_mut().remove(&name);
            held.host.remove_content_script(&held.id, &name);
            Ok(())
        })?,
    )?;

    let held = context.clone();
    native.set(
        "fetch",
        lua.create_function(
            move |_, (method, url, headers, body): (String, String, Option<Table>, Option<String>)| {
                held.require(Permission::Network)?;
                held.require_site(&url)?;
                let mut fields = Vec::new();
                if let Some(headers) = headers {
                    for pair in headers.pairs::<String, String>() {
                        let (name, value) = pair?;
                        fields.push((name, value));
                    }
                }
                let pending = held.pending();
                held.host.fetch(
                    &held.id,
                    pending,
                    HttpRequest {
                        method: method.to_uppercase(),
                        url,
                        headers: fields,
                        body,
                    },
                );
                Ok(pending.0)
            },
        )?,
    )?;

    Ok(native)
}

/// Installs `require`, which reaches the extension interface as `@atoll` and other files of the
/// extension by their path. Nothing outside the extension folder can be reached.
pub(crate) fn install_require(lua: &Lua, directory: &Path) -> mlua::Result<()> {
    let directory = directory.to_path_buf();
    let require = lua.create_function(move |lua, name: String| {
        if name == "@atoll" {
            return lua.named_registry_value::<Value>(crate::runtime::API_KEY);
        }
        let modules: Table = lua.named_registry_value(crate::runtime::MODULES_KEY)?;
        if let Ok(cached) = modules.get::<Value>(name.clone())
            && cached != Value::Nil
        {
            return Ok(cached);
        }
        let path = module_path(&directory, &name).ok_or_else(|| {
            mlua::Error::runtime(format!("\"{name}\" liegt nicht in dieser Erweiterung"))
        })?;
        let source = std::fs::read_to_string(&path).map_err(|error| {
            mlua::Error::runtime(format!("\"{name}\" kann nicht gelesen werden: {error}"))
        })?;
        let value: Value = lua.load(source).set_name(name.clone()).call(())?;
        modules.set(name, value.clone())?;
        Ok(value)
    })?;
    lua.globals().set("require", require)?;
    Ok(())
}

/// The file `name` points at, `None` when it would leave the extension folder.
fn module_path(directory: &Path, name: &str) -> Option<PathBuf> {
    let relative = name.trim_start_matches("./");
    if relative.is_empty() || relative.starts_with('@') {
        return None;
    }
    let mut path = PathBuf::from(relative);
    if path.extension().is_none() {
        path.set_extension("luau");
    }
    if !path
        .components()
        .all(|component| matches!(component, std::path::Component::Normal(_)))
    {
        return None;
    }
    Some(directory.join(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modules_stay_inside_the_extension_folder() {
        let directory = Path::new("/plugins/demo");
        assert_eq!(
            module_path(directory, "./util"),
            Some(PathBuf::from("/plugins/demo/util.luau"))
        );
        assert_eq!(
            module_path(directory, "shared/list.luau"),
            Some(PathBuf::from("/plugins/demo/shared/list.luau"))
        );
        assert_eq!(module_path(directory, "../../etc/passwd"), None);
        assert_eq!(module_path(directory, "/etc/passwd"), None);
        assert_eq!(module_path(directory, ""), None);
    }
}
