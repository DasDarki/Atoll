//! Loading extensions and running their scripts.
//!
//! Every extension gets a Luau state of its own, sandboxed, with a memory limit and a time budget
//! per call. Handlers run as coroutines: anything the browser has to do first stops the coroutine
//! and [`PluginRuntime::resume`] continues it with the answer, so a waiting script never blocks
//! the window.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use mlua::{Function, Lua, LuaSerdeExt, MultiValue, Table, Thread, VmState};
use serde_json::Value;

use mlua::Value as LuaValue;

use crate::api::{self, Command, PluginContext};
use crate::event::Event;
use crate::host::{PendingId, PendingResult, PluginHost, PluginId};
use crate::manifest::{MANIFEST_NAME, Manifest, ManifestError, Panel, Permission};
use crate::worker::{Report, Waker, WorkerId};

/// How much memory one extension may use.
const MEMORY_LIMIT: usize = 64 * 1024 * 1024;

/// How long one call of an extension may run before it is stopped.
const CALL_BUDGET: Duration = Duration::from_millis(250);

/// How long the first run of an extension may take, which usually does more work.
const LOAD_BUDGET: Duration = Duration::from_secs(2);

/// How often an extension may be stopped before Atoll switches it off.
const MAX_STRIKES: u32 = 3;

/// The message of the error that stops a script that ran too long.
const TIMEOUT: &str = "the script ran too long and was stopped";

/// What a script hears when the worker it waits for is stopped.
const WORKER_STOPPED: &str = "the worker was stopped";

pub(crate) const API_KEY: &str = "atoll.api";
pub(crate) const DISPATCH_KEY: &str = "atoll.dispatch";
pub(crate) const MODULES_KEY: &str = "atoll.modules";

/// Why an extension could not be loaded.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PluginError {
    #[error("{MANIFEST_NAME} could not be read: {0}")]
    Unreadable(String),
    #[error("{0}")]
    Manifest(#[from] ManifestError),
    #[error("\"{0}\" could not be read: {1}")]
    MissingEntry(String, String),
    #[error("the extension reported an error: {0}")]
    Failed(String),
}

/// A message a page sent with `window.atoll.send`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageMessage {
    pub frame_url: String,
    pub channel: String,
    pub payload: Value,
    /// What the browser needs to answer the page.
    pub reply: u64,
    /// The extension the message belongs to, for messages from its own panel. Without it the
    /// message goes to every extension that may see the page.
    pub target: Option<PluginId>,
}

/// What the browser shows about a loaded extension.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginInfo {
    pub id: PluginId,
    pub name: String,
    pub version: String,
    pub description: String,
    pub permissions: Vec<Permission>,
    pub commands: Vec<Command>,
    pub panels: Vec<Panel>,
    /// Why the extension was switched off, if it was.
    pub failure: Option<String>,
}

/// Why a coroutine is running, which decides what happens when it finishes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Purpose {
    /// Nothing is waiting for the result.
    Plain,
    /// A page is waiting for an answer.
    Message(u64),
    /// A worker asked with `host.call` and waits for the answer.
    WorkerCall(u64),
}

/// What driving a coroutine ended in.
enum Outcome {
    Finished(Option<Value>),
    Waiting,
    Failed(String),
}

struct Plugin {
    context: Rc<PluginContext>,
    lua: Lua,
    deadline: Rc<Cell<Option<Instant>>>,
    waiting: HashMap<PendingId, (Thread, Purpose)>,
    strikes: u32,
    failure: Option<String>,
}

impl Plugin {
    /// Whether the extension may put scripts into `url` and read what they find.
    fn sees_page(&self, url: &str) -> bool {
        self.failure.is_none()
            && self.context.allows(Permission::Pages)
            && self.context.manifest.covers(url)
    }

    /// Whether the extension may see `event`.
    fn receives(&self, event: &Event) -> bool {
        if self.failure.is_some() || !self.context.allows(event.permission()) {
            return false;
        }
        match event.url() {
            Some(url) => self.context.manifest.covers(url),
            None => true,
        }
    }

    fn info(&self) -> PluginInfo {
        let manifest = &self.context.manifest;
        PluginInfo {
            id: manifest.id.clone(),
            name: manifest.name.clone(),
            version: manifest.version.clone(),
            description: manifest.description.clone(),
            permissions: manifest.listed_permissions(),
            commands: self.context.commands.borrow().clone(),
            panels: manifest.panels.clone(),
            failure: self.failure.clone(),
        }
    }
}

/// All loaded extensions.
pub struct PluginRuntime {
    host: Rc<dyn PluginHost>,
    plugins: BTreeMap<PluginId, Plugin>,
    waiting: HashMap<PendingId, PluginId>,
    next_pending: Rc<Cell<u64>>,
    /// How many handlers still have to finish before a page message is answered.
    open_messages: HashMap<u64, usize>,
    /// The workers that wait for an answer to their `host.call`.
    open_calls: HashMap<u64, (PluginId, WorkerId, u64)>,
    next_call: u64,
    waker: Waker,
}

impl PluginRuntime {
    pub fn new(host: Rc<dyn PluginHost>) -> Self {
        Self {
            host,
            plugins: BTreeMap::new(),
            waiting: HashMap::new(),
            next_pending: Rc::default(),
            open_messages: HashMap::new(),
            open_calls: HashMap::new(),
            next_call: 0,
            waker: Arc::new(|| {}),
        }
    }

    /// Sets how a worker asks the browser to look at it. Without one the browser only sees a
    /// worker's answer the next time it looks by itself.
    pub fn set_waker(&mut self, waker: Waker) {
        self.waker = waker;
    }

    /// Reads the manifest of the extension in `directory`, without running anything.
    pub fn read_manifest(directory: &Path) -> Result<Manifest, PluginError> {
        let path = directory.join(MANIFEST_NAME);
        let text = std::fs::read_to_string(&path)
            .map_err(|error| PluginError::Unreadable(error.to_string()))?;
        Ok(Manifest::parse(&text)?)
    }

    /// Loads the extension in `directory` and runs its entry file. `granted` is what the user
    /// allowed, usually everything the manifest asks for.
    pub fn load(
        &mut self,
        directory: &Path,
        granted: &[Permission],
    ) -> Result<PluginInfo, PluginError> {
        let manifest = Self::read_manifest(directory)?;
        let entry = directory.join(&manifest.entry);
        let source = std::fs::read_to_string(&entry).map_err(|error| {
            PluginError::MissingEntry(manifest.entry.clone(), error.to_string())
        })?;
        let id = manifest.id.clone();
        self.unload(&id);

        let context = Rc::new(PluginContext {
            id: id.clone(),
            manifest,
            granted: granted.to_vec(),
            host: self.host.clone(),
            next_pending: self.next_pending.clone(),
            commands: RefCell::default(),
            scripts: RefCell::default(),
            workers: RefCell::default(),
            abandoned: RefCell::default(),
            waker: self.waker.clone(),
        });
        let deadline = Rc::new(Cell::new(None));
        let lua = Self::prepare(&context, &deadline, directory)
            .map_err(|error| PluginError::Failed(message_of(&error)))?;
        let plugin = Plugin {
            context,
            lua,
            deadline,
            waiting: HashMap::new(),
            strikes: 0,
            failure: None,
        };
        self.plugins.insert(id.clone(), plugin);

        let start = self.plugins[&id]
            .lua
            .load(source)
            .set_name(self.plugins[&id].context.manifest.entry.clone())
            .into_function()
            .and_then(|function| self.plugins[&id].lua.create_thread(function));
        let failure = match start {
            Ok(thread) => self.drive(&id, thread, MultiValue::new(), LOAD_BUDGET, Purpose::Plain),
            Err(error) => Some(message_of(&error)),
        };
        match failure {
            Some(failure) => {
                self.unload(&id);
                Err(PluginError::Failed(failure))
            }
            None => Ok(self.plugins[&id].info()),
        }
    }

    /// Builds the Luau state: limits, the raw functions, the prelude, then the sandbox.
    fn prepare(
        context: &Rc<PluginContext>,
        deadline: &Rc<Cell<Option<Instant>>>,
        directory: &Path,
    ) -> mlua::Result<Lua> {
        let lua = Lua::new();
        lua.set_memory_limit(MEMORY_LIMIT)?;
        let watched = deadline.clone();
        lua.set_interrupt(move |_| match watched.get() {
            Some(at) if Instant::now() > at => Err(mlua::Error::runtime(TIMEOUT)),
            _ => Ok(VmState::Continue),
        });
        lua.set_named_registry_value(MODULES_KEY, lua.create_table()?)?;
        let native = api::native_table(&lua, context)?;
        api::install_require(&lua, directory)?;
        let (api, dispatch): (LuaValue, Table) = lua
            .load(include_str!("prelude.luau"))
            .set_name("@atoll")
            .call(native)?;
        lua.set_named_registry_value(API_KEY, api)?;
        lua.set_named_registry_value(DISPATCH_KEY, dispatch)?;
        lua.sandbox(true)?;
        Ok(lua)
    }

    /// Forgets the extension, its scripts in pages and everything it was waiting for.
    pub fn unload(&mut self, id: &PluginId) {
        if let Some(plugin) = self.plugins.remove(id) {
            for name in plugin.context.scripts.borrow().iter() {
                self.host.remove_content_script(id, name);
            }
            self.host.remove_request_rules(id, None);
            self.waiting.retain(|_, waiting| waiting != id);
            self.host.commands_changed(id);
        }
    }

    pub fn is_loaded(&self, id: &PluginId) -> bool {
        self.plugins.contains_key(id)
    }

    pub fn loaded(&self) -> Vec<PluginInfo> {
        self.plugins.values().map(Plugin::info).collect()
    }

    /// Every command of every extension, for the command palette.
    pub fn commands(&self) -> Vec<(PluginId, Command)> {
        self.plugins
            .values()
            .filter(|plugin| plugin.failure.is_none())
            .flat_map(|plugin| {
                let id = plugin.context.id.clone();
                plugin
                    .context
                    .commands
                    .borrow()
                    .clone()
                    .into_iter()
                    .map(move |command| (id.clone(), command))
            })
            .collect()
    }

    /// Tells one extension what happened, if it may hear it.
    pub fn dispatch_to(&mut self, plugin: &PluginId, event: &Event) {
        if self
            .plugins
            .get(plugin)
            .is_some_and(|loaded| loaded.receives(event))
        {
            self.deliver(std::slice::from_ref(plugin), event);
        }
    }

    /// Tells every extension that may see it what happened.
    pub fn dispatch(&mut self, event: &Event) {
        let listeners: Vec<PluginId> = self
            .plugins
            .iter()
            .filter(|(_, plugin)| plugin.receives(event))
            .map(|(id, _)| id.clone())
            .collect();
        self.deliver(&listeners, event);
    }

    fn deliver(&mut self, listeners: &[PluginId], event: &Event) {
        let payload = event.payload();
        for id in listeners {
            let prepared = {
                let plugin = &self.plugins[id];
                let name = event.name().to_owned();
                thread_for(&plugin.lua, "event").and_then(|thread| {
                    let payload = plugin.lua.to_value(&payload)?;
                    Ok((thread, (name, payload)))
                })
            };
            match prepared {
                Ok((thread, arguments)) => {
                    if let Some(failure) =
                        self.drive(id, thread, arguments, CALL_BUDGET, Purpose::Plain)
                    {
                        self.failed(id, &failure);
                    }
                }
                Err(error) => self.host.report(id, &message_of(&error)),
            }
        }
    }

    /// Runs a command an extension added to the palette.
    pub fn run_command(&mut self, id: &PluginId, command: &str) {
        let Some(plugin) = self.plugins.get(id) else {
            return;
        };
        match thread_for(&plugin.lua, "command") {
            Ok(thread) => {
                if let Some(failure) =
                    self.drive(id, thread, command.to_owned(), CALL_BUDGET, Purpose::Plain)
                {
                    self.failed(id, &failure);
                }
            }
            Err(error) => self.host.report(id, &message_of(&error)),
        }
    }

    /// Hands the answer of the browser to the script that waits for it.
    pub fn resume(&mut self, pending: PendingId, result: PendingResult) {
        let Some(id) = self.waiting.remove(&pending) else {
            return;
        };
        let Some(plugin) = self.plugins.get_mut(&id) else {
            return;
        };
        let Some((thread, purpose)) = plugin.waiting.remove(&pending) else {
            return;
        };
        let arguments = match result {
            Ok(value) => plugin.lua.to_value(&value).map(|value| (true, value)),
            Err(message) => plugin
                .lua
                .to_value(&message)
                .map(|message| (false, message)),
        };
        match arguments {
            Ok(arguments) => {
                if let Some(failure) = self.drive(&id, thread, arguments, CALL_BUDGET, purpose) {
                    self.failed(&id, &failure);
                }
            }
            Err(error) => self.host.report(&id, &message_of(&error)),
        }
    }

    /// Looks at what the workers of every extension have to say, and answers what they ask for.
    /// The browser calls this whenever a worker wakes it.
    pub fn poll_workers(&mut self) {
        for id in self.plugins.keys().cloned().collect::<Vec<_>>() {
            let (abandoned, workers) = {
                let context = &self.plugins[&id].context;
                (
                    std::mem::take(&mut *context.abandoned.borrow_mut()),
                    context.workers.borrow().ids(),
                )
            };
            for pending in abandoned {
                self.resume(pending, Err(WORKER_STOPPED.to_owned()));
            }
            for worker in workers {
                let reports = match self.plugins.get(&id) {
                    Some(plugin) => plugin
                        .context
                        .workers
                        .borrow_mut()
                        .get_mut(worker)
                        .map(|running| running.take_reports())
                        .unwrap_or_default(),
                    None => Vec::new(),
                };
                for report in reports {
                    self.report_of_worker(&id, worker, report);
                }
            }
        }
    }

    /// One message of one worker.
    fn report_of_worker(&mut self, id: &PluginId, worker: WorkerId, report: Report) {
        match report {
            Report::Log { level, text } => self.host.log(id, level, &text),
            Report::Failed(message) => {
                let waiting = match self.plugins.get(id) {
                    Some(plugin) => plugin.context.workers.borrow_mut().remove(worker),
                    None => Vec::new(),
                };
                for pending in waiting {
                    self.resume(pending, Err(message.clone()));
                }
                self.host.report(id, &format!("Worker: {message}"));
            }
            Report::Done { id: job, result } => {
                let pending = self.plugins.get(id).and_then(|plugin| {
                    plugin
                        .context
                        .workers
                        .borrow_mut()
                        .get_mut(worker)
                        .and_then(|running| running.finished(job))
                });
                if let Some(pending) = pending {
                    self.resume(pending, result);
                }
            }
            Report::Call { ticket, name, args } => {
                self.call_from_worker(id, worker, ticket, name, args);
            }
        }
    }

    /// A worker asked its extension for something with `host.call`.
    fn call_from_worker(
        &mut self,
        id: &PluginId,
        worker: WorkerId,
        ticket: u64,
        name: String,
        args: Value,
    ) {
        self.next_call += 1;
        let call = self.next_call;
        self.open_calls.insert(call, (id.clone(), worker, ticket));
        let prepared = {
            let plugin = &self.plugins[id];
            thread_for(&plugin.lua, "worker").and_then(|thread| {
                let payload = plugin.lua.to_value(&args)?;
                Ok((thread, (worker, name, payload)))
            })
        };
        match prepared {
            Ok((thread, arguments)) => {
                if let Some(failure) = self.drive(
                    id,
                    thread,
                    arguments,
                    CALL_BUDGET,
                    Purpose::WorkerCall(call),
                ) {
                    self.answer_worker(call, Err(failure.clone()));
                    self.failed(id, &failure);
                }
            }
            Err(error) => {
                let message = message_of(&error);
                self.answer_worker(call, Err(message.clone()));
                self.host.report(id, &message);
            }
        }
    }

    /// Sends the answer of the extension back into the worker that waits for it.
    fn answer_worker(&mut self, call: u64, result: Result<Value, String>) {
        let Some((plugin, worker, ticket)) = self.open_calls.remove(&call) else {
            return;
        };
        if let Some(loaded) = self.plugins.get(&plugin)
            && let Some(running) = loaded.context.workers.borrow_mut().get_mut(worker)
        {
            running.answer(ticket, result);
        }
    }

    /// Whether any script is waiting for an answer, which the tests use.
    pub fn waiting(&self) -> usize {
        self.waiting.len()
    }

    /// Runs a coroutine until it finishes or waits for the browser. Returns the message of the
    /// error it ran into, if it ran into one.
    fn drive(
        &mut self,
        id: &PluginId,
        thread: Thread,
        arguments: impl mlua::IntoLuaMulti,
        budget: Duration,
        purpose: Purpose,
    ) -> Option<String> {
        let plugin = self.plugins.get_mut(id)?;
        plugin.deadline.set(Some(Instant::now() + budget));
        let resumed = thread.resume::<MultiValue>(arguments);
        plugin.deadline.set(None);
        let outcome = match resumed {
            Ok(values) if thread.is_resumable() => match pending_of(&values) {
                Some(pending) => {
                    plugin.waiting.insert(pending, (thread, purpose));
                    self.waiting.insert(pending, id.clone());
                    Outcome::Waiting
                }
                None => Outcome::Failed(
                    "a script used coroutine.yield without a call into Atoll".to_owned(),
                ),
            },
            Ok(values) => Outcome::Finished(returned(&plugin.lua, &values)),
            Err(error) => Outcome::Failed(message_of(&error)),
        };
        match outcome {
            Outcome::Waiting => None,
            Outcome::Finished(value) => {
                self.handler_finished(id, purpose, Ok(value));
                None
            }
            Outcome::Failed(message) => {
                self.handler_finished(id, purpose, Err(message.clone()));
                Some(message)
            }
        }
    }

    /// A handler is done. If a page or a worker waits for an answer, it gets it now.
    fn handler_finished(
        &mut self,
        id: &PluginId,
        purpose: Purpose,
        result: Result<Option<Value>, String>,
    ) {
        let reply = match purpose {
            Purpose::Plain => return,
            Purpose::Message(reply) => reply,
            Purpose::WorkerCall(call) => {
                let answer = match result {
                    Ok(value) => Ok(value.unwrap_or(Value::Null)),
                    Err(message) => Err(message),
                };
                self.answer_worker(call, answer);
                return;
            }
        };
        let value = result.ok().flatten();
        let answered = value.is_some();
        if answered {
            self.open_messages.remove(&reply);
            self.host
                .answer_page(id, reply, Ok(value.unwrap_or(Value::Null)));
            return;
        }
        let Some(open) = self.open_messages.get_mut(&reply) else {
            return;
        };
        *open = open.saturating_sub(1);
        if *open == 0 {
            self.open_messages.remove(&reply);
            self.host.answer_page(id, reply, Ok(Value::Null));
        }
    }

    /// Hands a message of a page to every extension that may see that page. The first answer a
    /// handler gives is sent back, even when it had to wait for something first.
    pub fn dispatch_message(&mut self, message: PageMessage) {
        let listeners: Vec<PluginId> = match &message.target {
            Some(target) => self
                .plugins
                .keys()
                .filter(|id| *id == target)
                .cloned()
                .collect(),
            None => self
                .plugins
                .iter()
                .filter(|(_, plugin)| plugin.sees_page(&message.frame_url))
                .map(|(id, _)| id.clone())
                .collect(),
        };
        if listeners.is_empty() {
            self.host
                .answer_page(&String::new(), message.reply, Ok(Value::Null));
            return;
        }
        self.open_messages.insert(message.reply, listeners.len());
        for id in listeners {
            let prepared = {
                let plugin = &self.plugins[&id];
                thread_for(&plugin.lua, "message").and_then(|thread| {
                    let payload = plugin.lua.to_value(&message.payload)?;
                    Ok((
                        thread,
                        (message.channel.clone(), payload, message.frame_url.clone()),
                    ))
                })
            };
            match prepared {
                Ok((thread, arguments)) => {
                    if let Some(failure) = self.drive(
                        &id,
                        thread,
                        arguments,
                        CALL_BUDGET,
                        Purpose::Message(message.reply),
                    ) {
                        self.failed(&id, &failure);
                    }
                }
                Err(error) => self.host.report(&id, &message_of(&error)),
            }
        }
    }

    /// A script ran into an error. Too many of them switch the extension off.
    fn failed(&mut self, id: &PluginId, message: &str) {
        let Some(plugin) = self.plugins.get_mut(id) else {
            return;
        };
        let timed_out = message.contains(TIMEOUT);
        if timed_out {
            plugin.strikes += 1;
        }
        tracing::warn!(plugin = %id, message, "an extension reported an error");
        self.host.report(id, message);
        if timed_out && plugin.strikes >= MAX_STRIKES {
            let reason = format!("{TIMEOUT} ({MAX_STRIKES} times)");
            plugin.failure = Some(reason.clone());
            plugin.waiting.clear();
            self.waiting.retain(|_, waiting| waiting != id);
            self.host.disabled(id, &reason);
        }
    }
}

/// The value a handler returned, `None` when it returned nothing.
fn returned(lua: &Lua, values: &MultiValue) -> Option<Value> {
    let first = values.front()?;
    if matches!(first, LuaValue::Nil) {
        return None;
    }
    lua.from_value(first.clone()).ok()
}

/// A fresh coroutine for `dispatch.<name>` of the prelude.
fn thread_for(lua: &Lua, name: &str) -> mlua::Result<Thread> {
    let dispatch: Table = lua.named_registry_value(DISPATCH_KEY)?;
    let function: Function = dispatch.get(name)?;
    lua.create_thread(function)
}

/// The work a coroutine waits for, if that is what it yielded.
fn pending_of(values: &MultiValue) -> Option<PendingId> {
    let Some(LuaValue::Table(table)) = values.front() else {
        return None;
    };
    table.get::<u64>("__atoll_pending").ok().map(PendingId)
}

/// The message of a Lua error without the stack trace Luau appends.
pub(crate) fn message_of(error: &mlua::Error) -> String {
    let text = match error {
        mlua::Error::CallbackError { cause, .. } => message_of(cause),
        other => other.to_string(),
    };
    text.lines()
        .next()
        .unwrap_or("unbekannter Fehler")
        .trim()
        .to_owned()
}

/// The folders under `directory` that hold an extension.
pub fn installed(directory: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut folders: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.join(MANIFEST_NAME).is_file())
        .collect();
    folders.sort();
    folders
}
