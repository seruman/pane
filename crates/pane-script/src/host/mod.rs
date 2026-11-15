//! Trusted, opt-in native capabilities. One scope per VM, cancelled when that VM dies.

mod bridge;
mod fs;
mod http;
mod json;
mod process;
mod streams;

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{
    Arc, Mutex, OnceLock,
    atomic::{AtomicU64, Ordering},
};

use rune_luau::{
    Host,
    mlua::{self, IntoLua, Lua, Value},
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Copy, Default)]
pub(crate) struct Capabilities {
    pub http: bool,
    pub process: bool,
    pub fs: bool,
}

#[derive(Clone, Debug)]
struct Error {
    pub kind: String,
    pub message: String,
}

impl Error {
    pub(super) fn new(kind: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            message: message.into(),
        }
    }

    fn into_lua(self, lua: &Lua) -> mlua::Result<Value> {
        let table = lua.create_table()?;
        table.set("kind", self.kind)?;
        table.set("message", self.message)?;
        Ok(Value::Table(table))
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}
impl std::error::Error for Error {}
impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        if let Some(error) = error.get_ref().and_then(|e| e.downcast_ref::<Self>()) {
            return error.clone();
        }
        let kind = match error.kind() {
            std::io::ErrorKind::NotFound => "not_found",
            std::io::ErrorKind::PermissionDenied => "permission_denied",
            std::io::ErrorKind::AlreadyExists => "already_exists",
            std::io::ErrorKind::TimedOut => "timeout",
            _ => "io",
        };
        Self::new(kind, error.to_string())
    }
}

type Result<T> = std::result::Result<T, Error>;

#[derive(Debug)]
enum Reply {
    Nil,
    Bool(bool),
    Number(i64),
    Text(String),
    Bytes(Vec<u8>),
    List(Vec<Reply>),
    Map(BTreeMap<String, Reply>),
    Handle(Resource),
}

impl Reply {
    pub(super) fn map<K: Into<String>>(fields: impl IntoIterator<Item = (K, Self)>) -> Self {
        Self::Map(
            fields
                .into_iter()
                .map(|(key, value)| (key.into(), value))
                .collect(),
        )
    }

    fn into_lua(self, lua: &Lua) -> mlua::Result<Value> {
        match self {
            Self::Nil => Ok(Value::Nil),
            Self::Bool(v) => v.into_lua(lua),
            Self::Number(v) => v.into_lua(lua),
            Self::Text(v) => v.into_lua(lua),
            Self::Bytes(v) => Ok(Value::String(lua.create_string(v)?)),
            Self::List(values) => {
                let table = lua.create_table()?;
                for (i, value) in values.into_iter().enumerate() {
                    table.raw_set(i + 1, value.into_lua(lua)?)?;
                }
                Ok(Value::Table(table))
            }
            Self::Map(values) => {
                let table = lua.create_table()?;
                for (key, value) in values {
                    table.raw_set(key, value.into_lua(lua)?)?;
                }
                Ok(Value::Table(table))
            }
            Self::Handle(resource) => Ok(Value::UserData(lua.create_userdata(resource)?)),
        }
    }
}

// Opaque handles. Dropping an unclaimed completion closes its resources too.
enum Resource {
    Reader(Arc<streams::Reader>),
    Writer(Arc<streams::Writer>),
    Process(Arc<process::Process>),
}
impl std::fmt::Debug for Resource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Reader(_) => "Reader",
            Self::Writer(_) => "Writer",
            Self::Process(_) => "Process",
        })
    }
}
impl mlua::UserData for Resource {}
impl Drop for Resource {
    fn drop(&mut self) {
        match self {
            Self::Reader(reader) => reader.close(),
            Self::Writer(writer) => writer.abort(),
            Self::Process(process) => process.kill(),
        }
    }
}

struct Job {
    cancel: CancellationToken,
    reply: Option<Result<Reply>>,
}

struct Scope {
    pub root: PathBuf,
    pub cancel: CancellationToken,
    budget: Arc<Semaphore>,
    jobs: Mutex<HashMap<u64, Job>>,
    next_job: AtomicU64,
}

impl Scope {
    pub(super) fn new(root: PathBuf) -> Arc<Self> {
        Arc::new(Self {
            root,
            cancel: CancellationToken::new(),
            budget: Arc::new(Semaphore::new(64)),
            jobs: Mutex::new(HashMap::new()),
            next_job: AtomicU64::new(1),
        })
    }

    pub(super) fn reserve(&self) -> Result<OwnedSemaphorePermit> {
        self.budget
            .clone()
            .try_acquire_owned()
            .map_err(|_| Error::new("limit", "at most 64 open resources per script"))
    }

    fn new_job(&self) -> Result<u64> {
        let mut jobs = self
            .jobs
            .lock()
            .map_err(|_| Error::new("io", "job lock poisoned"))?;
        if jobs.len() >= 64 {
            return Err(Error::new(
                "limit",
                "at most 64 pending operations per script",
            ));
        }
        let id = self.next_job.fetch_add(1, Ordering::Relaxed);
        jobs.insert(
            id,
            Job {
                cancel: self.cancel.child_token(),
                reply: None,
            },
        );
        Ok(id)
    }

    fn cancel_job(&self, id: u64) {
        if let Ok(mut jobs) = self.jobs.lock()
            && let Some(job) = jobs.remove(&id)
        {
            job.cancel.cancel();
        }
    }

    fn stop(&self) {
        self.cancel.cancel();
        if let Ok(mut jobs) = self.jobs.lock() {
            jobs.clear();
        }
    }
}

struct VmGuard(Arc<Scope>);
impl Drop for VmGuard {
    fn drop(&mut self) {
        self.0.stop();
    }
}

fn runtime() -> &'static tokio::runtime::Runtime {
    static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap_or_else(|e| panic!("pane-script I/O runtime: {e}"))
    })
}

pub(crate) fn install(
    host: &Host<'_>,
    root: PathBuf,
    capabilities: Capabilities,
) -> mlua::Result<()> {
    let scope = Scope::new(root);
    host.lua().set_app_data(VmGuard(scope.clone()));
    json::install(host)?;
    let native = bridge::install(host, scope, capabilities)?;
    let modules: mlua::Table = host
        .lua()
        .load(include_str!("api.luau"))
        .set_name("=@pane/host")
        .call(native)?;
    for name in ["http", "process", "fs"] {
        host.register_module(&format!("@pane/{name}"), modules.get::<mlua::Table>(name)?)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
