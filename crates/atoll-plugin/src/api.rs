//! The raw functions an extension reaches through the `atoll` table of the prelude. Every one of
//! them checks what the extension is allowed to do before it asks the browser.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use mlua::{Lua, LuaSerdeExt, Table, Value};

use crate::host::{
    HttpRequest, LogLevel, PendingId, PluginHost, PluginId, missing_host, missing_permission,
};
use crate::manifest::{Manifest, Permission};

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
