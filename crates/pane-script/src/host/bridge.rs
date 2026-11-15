//! The only VM/thread boundary: arguments leave the UI thread as owned Rust values.
//! Rune's completion payload is UTF-8-only Data, so it carries a ticket, not bytes.
//! The resumed task takes its byte-preserving result from the per-VM completion table.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use rune_luau::{
    Host,
    mlua::{self, AnyUserData, MultiValue, Table, Value},
};
use tokio_util::sync::CancellationToken;

use super::{
    Capabilities, Error, Reply, Resource, Result, Scope, fs, http, process, runtime, streams,
};

impl From<mlua::Error> for Error {
    fn from(error: mlua::Error) -> Self {
        Self::new("invalid_argument", error.to_string())
    }
}

enum Operation {
    Read(Arc<streams::Reader>, usize),
    ReadAll(Arc<streams::Reader>, usize),
    Write(Arc<streams::Writer>, Vec<u8>),
    CloseWriter(Arc<streams::Writer>),
    Http(http::Options),
    Spawn(process::Options),
    Wait(Arc<process::Process>),
    Collect(Arc<process::Process>, usize),
    Open(PathBuf, String),
    Stat(PathBuf),
    ReadDir(PathBuf),
    Mkdir(PathBuf, bool),
    Rename(PathBuf, PathBuf),
    Remove(PathBuf),
}

impl Operation {
    async fn run(self, scope: Arc<Scope>, cancel: CancellationToken) -> Result<Reply> {
        match self {
            Self::Read(reader, size) => reader
                .read(size)
                .await
                .map(|v| v.map_or(Reply::Nil, Reply::Bytes)),
            Self::ReadAll(reader, size) => reader.read_all(size).await.map(Reply::Bytes),
            Self::Write(writer, bytes) => writer.write(&bytes).await.map(|()| Reply::Bool(true)),
            Self::CloseWriter(writer) => writer.close().await.map(|()| Reply::Bool(true)),
            Self::Http(options) => http::request(scope, options, cancel).await,
            Self::Spawn(options) => process::spawn(scope, options, cancel).await,
            Self::Wait(process) => process.wait().await,
            Self::Collect(process, size) => process.collect(size).await,
            Self::Open(path, mode) => fs::open(scope, path, mode, cancel).await,
            Self::Stat(path) => fs::stat(scope, path).await,
            Self::ReadDir(path) => fs::read_dir(scope, path).await,
            Self::Mkdir(path, recursive) => fs::mkdir(scope, path, recursive).await,
            Self::Rename(from, to) => fs::rename(scope, from, to).await,
            Self::Remove(path) => fs::remove(scope, path).await,
        }
    }

    fn cancellation(&self) -> Option<Box<dyn FnOnce() + Send>> {
        match self {
            Self::Read(reader, _) | Self::ReadAll(reader, _) => {
                let reader = reader.clone();
                Some(Box::new(move || reader.close()))
            }
            Self::Write(writer, _) | Self::CloseWriter(writer) => {
                let writer = writer.clone();
                Some(Box::new(move || writer.abort()))
            }
            Self::Wait(process) | Self::Collect(process, _) => {
                let process = process.clone();
                Some(Box::new(move || process.kill()))
            }
            _ => None,
        }
    }
}

fn reader(args: &Table) -> Result<Arc<streams::Reader>> {
    let handle: AnyUserData = args.get("handle")?;
    let resource = handle.borrow::<Resource>()?;
    match &*resource {
        Resource::Reader(reader) => Ok(reader.clone()),
        _ => Err(Error::new("invalid_argument", "expected a reader")),
    }
}
fn writer(args: &Table) -> Result<Arc<streams::Writer>> {
    let handle: AnyUserData = args.get("handle")?;
    let resource = handle.borrow::<Resource>()?;
    match &*resource {
        Resource::Writer(writer) => Ok(writer.clone()),
        _ => Err(Error::new("invalid_argument", "expected a writer")),
    }
}
fn process_handle(args: &Table) -> Result<Arc<process::Process>> {
    let handle: AnyUserData = args.get("handle")?;
    let resource = handle.borrow::<Resource>()?;
    match &*resource {
        Resource::Process(process) => Ok(process.clone()),
        _ => Err(Error::new("invalid_argument", "expected a process")),
    }
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
fn limit(args: &Table, key: &str, default: usize, max: usize) -> Result<usize> {
    let value = args.get::<Option<f64>>(key)?.unwrap_or(default as f64);
    if !value.is_finite() || value.fract() != 0.0 || value < 1.0 || value > max as f64 {
        return Err(Error::new(
            "invalid_argument",
            format!("{key} must be an integer between 1 and {max}"),
        ));
    }
    Ok(value as usize)
}
fn path(args: &Table, key: &str) -> Result<PathBuf> {
    Ok(PathBuf::from(args.get::<String>(key)?))
}
fn require(enabled: bool, name: &str) -> Result<()> {
    if enabled {
        Ok(())
    } else {
        Err(Error::new(
            "permission_denied",
            format!("{name} is disabled; run with --allow-{name} for a trusted script"),
        ))
    }
}
fn max_bytes(args: &Table) -> Result<usize> {
    limit(
        args,
        "maxBytes",
        streams::DEFAULT_LIMIT,
        streams::HARD_LIMIT,
    )
}

fn push_header(
    headers: &mut Vec<(String, String)>,
    bytes: &mut usize,
    key: &str,
    value: &mlua::LuaString,
) -> Result<()> {
    *bytes += key.len() + value.as_bytes().len();
    if headers.len() >= 128 || *bytes > 65536 {
        return Err(Error::new(
            "limit",
            "HTTP headers exceed 128 values or 64 KiB",
        ));
    }
    headers.push((key.to_owned(), value.to_str()?.to_owned()));
    Ok(())
}

// Validate dense arrays without silently dropping holes or named keys. Check byte budgets
// before cloning strings out of the VM (the same Lua string may appear many times).
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
fn string_array(table: Table, max_items: usize, max_bytes: usize) -> Result<Vec<String>> {
    let len = table.raw_len();
    if len > max_items {
        return Err(Error::new("limit", "too many array entries"));
    }
    let mut result = vec![None; len];
    let mut bytes = 0;
    for pair in table.pairs::<Value, Value>() {
        let (key, value) = pair?;
        let index = match key {
            Value::Integer(n) => usize::try_from(n).ok(),
            Value::Number(n) if n.fract() == 0.0 && n >= 1.0 && n <= max_items as f64 => {
                Some(n as usize)
            }
            _ => None,
        }
        .filter(|i| *i >= 1 && *i <= len)
        .ok_or_else(|| Error::new("invalid_argument", "expected a dense array"))?;
        let Value::String(value) = value else {
            return Err(Error::new(
                "invalid_argument",
                "array entries must be strings",
            ));
        };
        bytes += value.as_bytes().len();
        if bytes > max_bytes {
            return Err(Error::new("limit", "array strings exceed byte limit"));
        }
        result[index - 1] = Some(value.to_str()?.to_owned());
    }
    result
        .into_iter()
        .map(|v| v.ok_or_else(|| Error::new("invalid_argument", "expected a dense array")))
        .collect()
}

fn http_options(args: &Table) -> Result<http::Options> {
    let mut headers = Vec::new();
    let mut header_bytes = 0;
    if let Some(table) = args.get::<Option<Table>>("headers")? {
        for pair in table.pairs::<String, Value>() {
            let (key, value) = pair?;
            match value {
                Value::String(s) => push_header(&mut headers, &mut header_bytes, &key, &s)?,
                Value::Table(values) => {
                    for value in string_array(values, 128, 65536)? {
                        header_bytes += key.len() + value.len();
                        if headers.len() >= 128 || header_bytes > 65536 {
                            return Err(Error::new(
                                "limit",
                                "HTTP headers exceed 128 values or 64 KiB",
                            ));
                        }
                        headers.push((key.clone(), value));
                    }
                }
                _ => {
                    return Err(Error::new(
                        "invalid_argument",
                        "headers need strings or arrays of strings",
                    ));
                }
            }
        }
    }
    let body = args.get::<Option<mlua::LuaString>>("body")?;
    if body
        .as_ref()
        .is_some_and(|b| b.as_bytes().len() > streams::HARD_LIMIT)
    {
        return Err(Error::new("limit", "request body exceeds 16 MiB"));
    }
    let body = body.map(|b| b.as_bytes().to_vec());
    Ok(http::Options {
        url: args.get("url")?,
        method: args
            .get::<Option<String>>("method")?
            .unwrap_or_else(|| "GET".into()),
        headers,
        body,
        timeout_ms: limit(args, "timeoutMs", 30000, 3600000)? as u64,
        max_body_bytes: limit(
            args,
            "maxBodyBytes",
            streams::HARD_LIMIT,
            streams::HARD_LIMIT,
        )?,
    })
}

fn process_options(args: &Table) -> Result<process::Options> {
    let arguments = string_array(args.get("args")?, 512, 1024 * 1024)?;
    let mut env = BTreeMap::new();
    if let Some(table) = args.get::<Option<Table>>("env")? {
        let mut bytes = 0;
        for pair in table.pairs::<String, String>() {
            let (key, value) = pair?;
            bytes += key.len() + value.len();
            if bytes > 65536 {
                return Err(Error::new("limit", "environment overrides exceed 64 KiB"));
            }
            env.insert(key, value);
        }
    }
    let stdin = match args.get::<Option<String>>("stdin")?.as_deref() {
        None | Some("null") => false,
        Some("pipe") => true,
        _ => {
            return Err(Error::new(
                "invalid_argument",
                "stdin must be 'null' or 'pipe'",
            ));
        }
    };
    Ok(process::Options {
        command: args.get("command")?,
        args: arguments,
        cwd: args.get::<Option<String>>("cwd")?.map(PathBuf::from),
        env,
        inherit_env: args.get::<Option<bool>>("inheritEnv")?.unwrap_or(true),
        stdin,
        timeout_ms: limit(args, "timeoutMs", 30000, 3600000)? as u64,
    })
}

fn prepare(name: &str, args: &Table, caps: Capabilities) -> Result<Operation> {
    Ok(match name {
        "read" => Operation::Read(
            reader(args)?,
            limit(args, "size", streams::CHUNK, streams::CHUNK)?,
        ),
        "readAll" => Operation::ReadAll(reader(args)?, max_bytes(args)?),
        "write" => {
            let bytes = args.get::<mlua::LuaString>("bytes")?;
            if bytes.as_bytes().len() > streams::HARD_LIMIT {
                return Err(Error::new("limit", "write exceeds 16 MiB"));
            }
            Operation::Write(writer(args)?, bytes.as_bytes().to_vec())
        }
        "closeWriter" => Operation::CloseWriter(writer(args)?),
        "wait" => Operation::Wait(process_handle(args)?),
        "collect" => Operation::Collect(process_handle(args)?, max_bytes(args)?),
        "request" => {
            require(caps.http, "http")?;
            Operation::Http(http_options(args)?)
        }
        "spawn" => {
            require(caps.process, "process")?;
            Operation::Spawn(process_options(args)?)
        }
        "open" | "stat" | "readDir" | "mkdir" | "rename" | "remove" => {
            require(caps.fs, "fs")?;
            match name {
                "open" => Operation::Open(
                    path(args, "path")?,
                    args.get::<Option<String>>("mode")?
                        .unwrap_or_else(|| "read".into()),
                ),
                "stat" => Operation::Stat(path(args, "path")?),
                "readDir" => Operation::ReadDir(path(args, "path")?),
                "mkdir" => Operation::Mkdir(
                    path(args, "path")?,
                    args.get::<Option<bool>>("recursive")?.unwrap_or(false),
                ),
                "rename" => Operation::Rename(path(args, "from")?, path(args, "to")?),
                _ => Operation::Remove(path(args, "path")?),
            }
        }
        _ => return Err(Error::new("invalid_argument", "unknown host operation")),
    })
}

fn await_function(
    host: &Host<'_>,
    s: Arc<Scope>,
    caps: Capabilities,
) -> mlua::Result<mlua::Function> {
    host.async_function("host I/O", move |lua, args: MultiValue, done| {
        use mlua::FromLuaMulti;
        let (id, name, args): (u64, String, Table) = FromLuaMulti::from_lua_multi(args, lua)?;
        let token = s
            .jobs
            .lock()
            .ok()
            .and_then(|jobs| jobs.get(&id).map(|job| job.cancel.clone()));
        let Some(token) = token else {
            return Ok(());
        };
        let operation = prepare(&name, &args, caps);
        let scope = s.clone();
        let _task = runtime().spawn(async move {
            let result = match operation {
                Ok(operation) => {
                    let cleanup = operation.cancellation();
                    tokio::select! {
                        biased;
                        () = token.cancelled() => {
                            if let Some(cleanup) = cleanup { cleanup(); }
                            Err(Error::new("cancelled", "operation was cancelled"))
                        }
                        result = operation.run(scope.clone(), token.clone()) => result,
                    }
                }
                Err(error) => Err(error),
            };
            if let Ok(mut jobs) = scope.jobs.lock()
                && let Some(job) = jobs.get_mut(&id)
            {
                job.reply = Some(result);
            }
            // Only a ticket crosses Rune's UTF-8 Data boundary. Binary results stay in Rust.
            done.complete(true);
        });
        Ok(())
    })
}

pub(super) fn install(
    host: &Host<'_>,
    scope: Arc<Scope>,
    caps: Capabilities,
) -> mlua::Result<Table> {
    let lua = host.lua();
    let native = lua.create_table()?;
    let s = scope.clone();
    native.set(
        "newJob",
        lua.create_function(move |lua, ()| match s.new_job() {
            Ok(id) => Ok((
                Value::Integer(id.try_into().map_err(mlua::Error::external)?),
                Value::Nil,
            )),
            Err(error) => Ok((Value::Nil, error.into_lua(lua)?)),
        })?,
    )?;
    let s = scope.clone();
    native.set(
        "cancel",
        lua.create_function(move |_, id: u64| {
            s.cancel_job(id);
            Ok(())
        })?,
    )?;
    let s = scope.clone();
    native.set(
        "take",
        lua.create_function(move |lua, id: u64| {
            let result = s
                .jobs
                .lock()
                .ok()
                .and_then(|mut jobs| jobs.remove(&id))
                .and_then(|job| job.reply)
                .unwrap_or_else(|| Err(Error::new("cancelled", "operation was cancelled")));
            match result {
                Ok(value) => Ok((value.into_lua(lua)?, Value::Nil)),
                Err(error) => Ok((Value::Nil, error.into_lua(lua)?)),
            }
        })?,
    )?;
    native.set("await", await_function(host, scope.clone(), caps)?)?;
    native.set(
        "closeReader",
        lua.create_function(|_, handle: AnyUserData| {
            let resource = handle.borrow::<Resource>()?;
            if let Resource::Reader(reader) = &*resource {
                reader.close();
                Ok(true)
            } else {
                Err(mlua::Error::runtime("expected a reader"))
            }
        })?,
    )?;
    native.set(
        "kill",
        lua.create_function(|_, handle: AnyUserData| {
            let resource = handle.borrow::<Resource>()?;
            if let Resource::Process(process) = &*resource {
                process.kill();
                Ok(true)
            } else {
                Err(mlua::Error::runtime("expected a process"))
            }
        })?,
    )?;
    Ok(native)
}
