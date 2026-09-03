//! Luau states on threads of their own, for work that would hold up the window.
//!
//! A worker shares nothing with the extension that started it: jobs go in as values and answers
//! come back as values. While a worker runs a job it may ask its extension for something with
//! `host.call`, which stops the worker until the answer arrives. The window never waits for a
//! worker, so the two can never hold each other up.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, VecDeque};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, TryRecvError, channel};
use std::thread;
use std::time::{Duration, Instant};

use mlua::{Lua, LuaSerdeExt, MultiValue, Table, VmState};
use serde_json::Value;

use crate::host::{LogLevel, PendingId};

/// How much memory one worker may use.
const MEMORY_LIMIT: usize = 64 * 1024 * 1024;

/// How long one job of a worker may run.
const JOB_BUDGET: Duration = Duration::from_secs(30);

/// How long a worker waits for its extension to answer `host.call`.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(30);

/// How many workers one extension may have at a time.
pub(crate) const MAX_WORKERS: usize = 4;

const STOPPED: &str = "the worker was stopped";
const TIMEOUT: &str = "the worker ran too long and was stopped";

/// A way to tell the browser that a worker has something to say. Workers run on their own
/// threads, so this is what gets the main thread to look at them.
pub type Waker = Arc<dyn Fn() + Send + Sync>;

/// Identifies a worker of one extension.
pub(crate) type WorkerId = u64;

/// What the extension sends into a worker.
enum Job {
    /// Run the function `name` of the table the worker returned.
    Call {
        id: u64,
        name: String,
        args: Value,
    },
    /// The answer to a `host.call` of the worker.
    Answer {
        ticket: u64,
        result: Result<Value, String>,
    },
    Stop,
}

/// What a worker sends to its extension.
pub(crate) enum Report {
    /// The worker could not be started, or its source ran into an error.
    Failed(String),
    /// A job is done, and the script that waits for it can go on.
    Done {
        id: u64,
        result: Result<Value, String>,
    },
    /// `host.call`, which the extension answers with [`Worker::answer`].
    Call {
        ticket: u64,
        name: String,
        args: Value,
    },
    Log {
        level: LogLevel,
        text: String,
    },
}

/// A worker as its extension sees it.
pub(crate) struct Worker {
    jobs: Sender<Job>,
    reports: Receiver<Report>,
    cancel: Arc<AtomicBool>,
    /// The scripts that wait for a job, by the number of the job.
    waiting: BTreeMap<u64, PendingId>,
    next_job: u64,
}

impl Worker {
    /// Starts a worker that runs `source` and answers jobs afterwards.
    pub(crate) fn spawn(plugin: &str, source: String, wake: Waker) -> std::io::Result<Self> {
        let (to_worker, jobs) = channel::<Job>();
        let (to_plugin, reports) = channel::<Report>();
        let cancel = Arc::new(AtomicBool::new(false));
        let stopped = cancel.clone();
        thread::Builder::new()
            .name(format!("{plugin}-worker"))
            .spawn(move || {
                let channel = Rc::new(Channel {
                    jobs,
                    reports: to_plugin,
                    queued: RefCell::default(),
                    next_ticket: Cell::new(0),
                    wake,
                });
                run(&source, &channel, &stopped);
            })?;
        Ok(Self {
            jobs: to_worker,
            reports,
            cancel,
            waiting: BTreeMap::new(),
            next_job: 0,
        })
    }

    /// Asks the worker to run `name`, and remembers the script that waits for the answer.
    pub(crate) fn call(&mut self, name: String, args: Value, pending: PendingId) {
        self.next_job += 1;
        let id = self.next_job;
        self.waiting.insert(id, pending);
        if self.jobs.send(Job::Call { id, name, args }).is_err() {
            self.waiting.remove(&id);
        }
    }

    /// Answers a `host.call` of the worker.
    pub(crate) fn answer(&self, ticket: u64, result: Result<Value, String>) {
        let _ = self.jobs.send(Job::Answer { ticket, result });
    }

    /// The script that waits for a job, once the job is done.
    pub(crate) fn finished(&mut self, id: u64) -> Option<PendingId> {
        self.waiting.remove(&id)
    }

    /// What the worker said since the last look.
    pub(crate) fn take_reports(&self) -> Vec<Report> {
        let mut reports = Vec::new();
        loop {
            match self.reports.try_recv() {
                Ok(report) => reports.push(report),
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => return reports,
            }
        }
    }

    /// The scripts that will never get an answer because the worker is going away.
    pub(crate) fn abandoned(&mut self) -> Vec<PendingId> {
        std::mem::take(&mut self.waiting).into_values().collect()
    }

    /// Stops the worker. A job that is still running is cut short.
    pub(crate) fn stop(&self) {
        self.cancel.store(true, Ordering::Relaxed);
        let _ = self.jobs.send(Job::Stop);
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.stop();
    }
}

/// The workers of one extension.
#[derive(Default)]
pub(crate) struct Workers {
    running: BTreeMap<WorkerId, Worker>,
    next: WorkerId,
}

impl Workers {
    /// Starts a worker and answers with the number the extension knows it by.
    pub(crate) fn spawn(
        &mut self,
        plugin: &str,
        source: String,
        wake: Waker,
    ) -> mlua::Result<WorkerId> {
        if self.running.len() >= MAX_WORKERS {
            return Err(mlua::Error::runtime(format!(
                "an extension may have at most {MAX_WORKERS} workers"
            )));
        }
        let worker = Worker::spawn(plugin, source, wake)
            .map_err(|error| mlua::Error::runtime(format!("the worker does not start: {error}")))?;
        self.next += 1;
        self.running.insert(self.next, worker);
        Ok(self.next)
    }

    pub(crate) fn get_mut(&mut self, id: WorkerId) -> Option<&mut Worker> {
        self.running.get_mut(&id)
    }

    /// Stops a worker and answers with the scripts that were waiting for it.
    pub(crate) fn remove(&mut self, id: WorkerId) -> Vec<PendingId> {
        match self.running.remove(&id) {
            Some(mut worker) => worker.abandoned(),
            None => Vec::new(),
        }
    }

    pub(crate) fn ids(&self) -> Vec<WorkerId> {
        self.running.keys().copied().collect()
    }
}

/// The two directions of one worker, as its own thread sees them.
struct Channel {
    jobs: Receiver<Job>,
    reports: Sender<Report>,
    /// Jobs that arrived while the worker waited for an answer.
    queued: RefCell<VecDeque<Job>>,
    next_ticket: Cell<u64>,
    wake: Waker,
}

impl Channel {
    fn send(&self, report: Report) -> bool {
        let sent = self.reports.send(report).is_ok();
        if sent {
            (self.wake)();
        }
        sent
    }

    /// Asks the extension for something and waits for its answer.
    fn call(&self, name: String, args: Value) -> Result<Value, String> {
        self.next_ticket.set(self.next_ticket.get() + 1);
        let ticket = self.next_ticket.get();
        if !self.send(Report::Call { ticket, name, args }) {
            return Err("the extension is no longer loaded".to_owned());
        }
        let until = Instant::now() + ANSWER_TIMEOUT;
        loop {
            let left = until.saturating_duration_since(Instant::now());
            match self.jobs.recv_timeout(left) {
                Ok(Job::Answer {
                    ticket: answered,
                    result,
                }) if answered == ticket => {
                    return result;
                }
                Ok(Job::Stop) => {
                    self.queued.borrow_mut().push_back(Job::Stop);
                    return Err(STOPPED.to_owned());
                }
                Ok(other) => self.queued.borrow_mut().push_back(other),
                Err(RecvTimeoutError::Timeout) => {
                    return Err("the extension did not answer".to_owned());
                }
                Err(RecvTimeoutError::Disconnected) => {
                    return Err("the extension is no longer loaded".to_owned());
                }
            }
        }
    }

    /// The next job, jobs that arrived out of turn first.
    fn next_job(&self) -> Option<Job> {
        if let Some(queued) = self.queued.borrow_mut().pop_front() {
            return Some(queued);
        }
        self.jobs.recv().ok()
    }
}

/// The life of one worker: load the source, then answer jobs until it is stopped.
fn run(source: &str, channel: &Rc<Channel>, cancel: &Arc<AtomicBool>) {
    let deadline = Rc::new(Cell::new(None));
    let lua = match prepare(channel, cancel, &deadline) {
        Ok(lua) => lua,
        Err(error) => {
            channel.send(Report::Failed(crate::runtime::message_of(&error)));
            return;
        }
    };
    deadline.set(Some(Instant::now() + JOB_BUDGET));
    let loaded = lua
        .load(source)
        .set_name("worker")
        .call::<MultiValue>(())
        .and_then(|values| match values.front() {
            Some(mlua::Value::Table(table)) => Ok(table.clone()),
            _ => Err(mlua::Error::runtime(
                "a worker has to return a table with its functions",
            )),
        });
    deadline.set(None);
    let functions = match loaded {
        Ok(functions) => functions,
        Err(error) => {
            channel.send(Report::Failed(crate::runtime::message_of(&error)));
            return;
        }
    };
    while let Some(job) = channel.next_job() {
        match job {
            Job::Stop => break,
            Job::Answer { .. } => {}
            Job::Call { id, name, args } => {
                let result = perform(&lua, &functions, &name, args, &deadline);
                if !channel.send(Report::Done { id, result }) {
                    break;
                }
            }
        }
        if cancel.load(Ordering::Relaxed) {
            break;
        }
    }
}

/// Builds the Luau state of a worker: limits, the `host` table, then the sandbox.
fn prepare(
    channel: &Rc<Channel>,
    cancel: &Arc<AtomicBool>,
    deadline: &Rc<Cell<Option<Instant>>>,
) -> mlua::Result<Lua> {
    let lua = Lua::new();
    lua.set_memory_limit(MEMORY_LIMIT)?;
    let stopped = cancel.clone();
    let watched = deadline.clone();
    lua.set_interrupt(move |_| {
        if stopped.load(Ordering::Relaxed) {
            return Err(mlua::Error::runtime(STOPPED));
        }
        match watched.get() {
            Some(at) if Instant::now() > at => Err(mlua::Error::runtime(TIMEOUT)),
            _ => Ok(VmState::Continue),
        }
    });
    let host = lua.create_table()?;
    let held = channel.clone();
    host.set(
        "call",
        lua.create_function(move |lua, (name, args): (String, mlua::Value)| {
            let args: Value = lua.from_value(args)?;
            match held.call(name, args) {
                Ok(value) => lua.to_value(&value),
                Err(message) => Err(mlua::Error::runtime(message)),
            }
        })?,
    )?;
    for (name, level) in [
        ("log", LogLevel::Info),
        ("warn", LogLevel::Warn),
        ("error", LogLevel::Error),
    ] {
        let held = channel.clone();
        host.set(
            name,
            lua.create_function(move |_, text: String| {
                held.send(Report::Log { level, text });
                Ok(())
            })?,
        )?;
    }
    lua.globals().set("host", host)?;
    lua.sandbox(true)?;
    Ok(lua)
}

/// Runs one function of the worker with the value the extension sent.
fn perform(
    lua: &Lua,
    functions: &Table,
    name: &str,
    args: Value,
    deadline: &Rc<Cell<Option<Instant>>>,
) -> Result<Value, String> {
    let run = || -> mlua::Result<Value> {
        let function: mlua::Function = functions.get(name).map_err(|_| {
            mlua::Error::runtime(format!("this worker has no function named \"{name}\""))
        })?;
        let argument = lua.to_value(&args)?;
        let answer: mlua::Value = function.call(argument)?;
        lua.from_value(answer)
    };
    deadline.set(Some(Instant::now() + JOB_BUDGET));
    let result = run();
    deadline.set(None);
    result.map_err(|error| crate::runtime::message_of(&error))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quiet() -> Waker {
        Arc::new(|| {})
    }

    /// Waits for what the worker says, without holding up the test forever.
    fn await_reports(worker: &Worker, wanted: usize) -> Vec<Report> {
        let until = Instant::now() + Duration::from_secs(10);
        let mut reports = Vec::new();
        while reports.len() < wanted && Instant::now() < until {
            reports.extend(worker.take_reports());
            if reports.len() < wanted {
                thread::sleep(Duration::from_millis(5));
            }
        }
        reports
    }

    #[test]
    fn a_worker_answers_with_values() {
        let mut worker = Worker::spawn(
            "demo",
            "return { doppelt = function(zahl) return zahl * 2 end }".to_owned(),
            quiet(),
        )
        .unwrap();
        worker.call("doppelt".to_owned(), Value::from(21), PendingId(7));
        let reports = await_reports(&worker, 1);
        match reports.first() {
            Some(Report::Done { id, result }) => {
                assert_eq!(worker.finished(*id), Some(PendingId(7)));
                assert_eq!(result.as_ref().unwrap().as_f64(), Some(42.0));
            }
            _ => panic!("the worker did not answer"),
        }
    }

    #[test]
    fn a_worker_asks_its_extension_and_waits_for_the_answer() {
        let mut worker = Worker::spawn(
            "demo",
            "return { frag = function() return host.call(\"wert\", nil) .. \"!\" end }".to_owned(),
            quiet(),
        )
        .unwrap();
        worker.call("frag".to_owned(), Value::Null, PendingId(1));
        let asked = await_reports(&worker, 1);
        match asked.first() {
            Some(Report::Call { ticket, name, .. }) => {
                assert_eq!(name, "wert");
                worker.answer(*ticket, Ok(Value::from("hallo")));
            }
            _ => panic!("the worker did not ask"),
        }
        let done = await_reports(&worker, 1);
        match done.first() {
            Some(Report::Done { result, .. }) => {
                assert_eq!(result.as_ref().unwrap(), &Value::from("hallo!"));
            }
            _ => panic!("the worker did not finish"),
        }
    }

    #[test]
    fn an_error_of_the_extension_reaches_the_worker() {
        let mut worker = Worker::spawn(
            "demo",
            "return { frag = function() \
             local ok, problem = pcall(function() return host.call(\"wert\", nil) end) \
             return if ok then \"ok\" else tostring(problem) end }"
                .to_owned(),
            quiet(),
        )
        .unwrap();
        worker.call("frag".to_owned(), Value::Null, PendingId(1));
        let asked = await_reports(&worker, 1);
        let Some(Report::Call { ticket, .. }) = asked.first() else {
            panic!("the worker did not ask");
        };
        worker.answer(*ticket, Err("geht nicht".to_owned()));
        let done = await_reports(&worker, 1);
        match done.first() {
            Some(Report::Done { result, .. }) => {
                assert!(
                    result
                        .as_ref()
                        .unwrap()
                        .as_str()
                        .unwrap()
                        .contains("geht nicht"),
                    "{result:?}"
                );
            }
            _ => panic!("the worker did not finish"),
        }
    }

    #[test]
    fn a_worker_that_does_not_return_functions_says_so() {
        let worker = Worker::spawn("demo", "return 5".to_owned(), quiet()).unwrap();
        let reports = await_reports(&worker, 1);
        match reports.first() {
            Some(Report::Failed(message)) => {
                assert!(message.contains("table with its functions"), "{message}")
            }
            _ => panic!("the worker did not report the problem"),
        }
    }

    #[test]
    fn a_worker_that_never_ends_is_stopped() {
        let mut worker = Worker::spawn(
            "demo",
            "return { ewig = function() while true do end return 1 end }".to_owned(),
            quiet(),
        )
        .unwrap();
        worker.call("ewig".to_owned(), Value::Null, PendingId(1));
        thread::sleep(Duration::from_millis(50));
        worker.stop();
        let reports = await_reports(&worker, 1);
        match reports.first() {
            Some(Report::Done { result, .. }) => {
                assert!(result.as_ref().unwrap_err().contains(STOPPED));
            }
            _ => panic!("the worker did not stop"),
        }
    }

    #[test]
    fn what_a_worker_logs_reaches_the_extension() {
        let worker = Worker::spawn(
            "demo",
            "host.warn(\"achtung\") return {}".to_owned(),
            quiet(),
        )
        .unwrap();
        let reports = await_reports(&worker, 1);
        match reports.first() {
            Some(Report::Log { level, text }) => {
                assert_eq!(*level, LogLevel::Warn);
                assert_eq!(text, "achtung");
            }
            _ => panic!("the worker said nothing"),
        }
    }
}
