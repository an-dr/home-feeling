//! An opt-in record of everything the desktop module sees and does, one JSON
//! object per line, for diagnosing layout problems from a real session.
//!
//! Each line has `t` (milliseconds since the Unix epoch) and `kind`:
//! - `start`: tracing was switched on; `snapshot` follows on the next frame
//! - `snapshot`: every window and monitor as the OS reports them now
//! - `out`: an event the module published, with `topic` and `payload`
//! - `in`: a request a Thing sent, with `topic`, `sender`, and `payload`
//! - `error`: a request that failed, with `topic`, `sender`, and `error`
//! - `stop`: tracing was switched off
//!
//! Off by default: a [`Trace`] does nothing until [`Trace::set_enabled`].
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

/// A file past this size is moved aside to `<name>.old` when tracing starts.
const ROTATE_BYTES: u64 = 50 * 1024 * 1024;

struct Inner {
    path: PathBuf,
    file: Option<File>,
    snapshot_due: bool,
}

/// A handle to the trace file; clones share it.
#[derive(Clone)]
pub struct Trace(Arc<Mutex<Inner>>);

impl Trace {
    /// A trace that will write to `path` once enabled.
    pub fn new(path: PathBuf) -> Self {
        Self(Arc::new(Mutex::new(Inner {
            path,
            file: None,
            snapshot_due: false,
        })))
    }

    /// A trace that never writes.
    pub fn off() -> Self {
        Self::new(PathBuf::new())
    }

    pub fn path(&self) -> PathBuf {
        self.0.lock().unwrap().path.clone()
    }

    pub fn enabled(&self) -> bool {
        self.0.lock().unwrap().file.is_some()
    }

    /// Starts or stops writing. Starting appends to the file, after moving an
    /// oversized one aside, and asks for a snapshot on the next frame.
    pub fn set_enabled(&self, on: bool) -> Result<(), String> {
        let mut inner = self.0.lock().unwrap();
        if on == inner.file.is_some() {
            return Ok(());
        }
        if on {
            let file = open(&inner.path)
                .map_err(|err| format!("cannot open the trace {}: {err}", inner.path.display()))?;
            inner.file = Some(file);
            inner.snapshot_due = true;
            write(&mut inner, json!({ "kind": "start" }));
        } else {
            write(&mut inner, json!({ "kind": "stop" }));
            inner.file = None;
        }
        Ok(())
    }

    /// True once after tracing starts: the caller should record a snapshot.
    pub(crate) fn snapshot_due(&self) -> bool {
        let mut inner = self.0.lock().unwrap();
        std::mem::take(&mut inner.snapshot_due) && inner.file.is_some()
    }

    /// Writes `record` with a timestamp, if tracing is on.
    pub(crate) fn record(&self, record: Value) {
        let mut inner = self.0.lock().unwrap();
        if inner.file.is_some() {
            write(&mut inner, record);
        }
    }

    /// Writes a bus payload, parsed as JSON when it is JSON.
    pub(crate) fn message(&self, kind: &str, topic: &str, sender: Option<&str>, payload: &[u8]) {
        if !self.enabled() {
            return;
        }
        let payload = serde_json::from_slice::<Value>(payload)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(payload).into_owned()));
        let mut record = json!({ "kind": kind, "topic": topic, "payload": payload });
        if let Some(sender) = sender {
            record["sender"] = Value::String(sender.to_string());
        }
        self.record(record);
    }
}

fn open(path: &Path) -> std::io::Result<File> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if std::fs::metadata(path).is_ok_and(|meta| meta.len() > ROTATE_BYTES) {
        let mut old = path.as_os_str().to_owned();
        old.push(".old");
        std::fs::rename(path, old)?;
    }
    OpenOptions::new().create(true).append(true).open(path)
}

fn write(inner: &mut Inner, mut record: Value) {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64);
    let mut line = serde_json::Map::new();
    line.insert("t".into(), millis.into());
    if let Some(fields) = record.as_object_mut() {
        line.append(fields);
    }
    let mut text = Value::Object(line).to_string();
    text.push('\n');
    // A write failure must not disturb the desktop; stop tracing instead.
    if let Some(file) = &mut inner.file {
        if file.write_all(text.as_bytes()).is_err() {
            inner.file = None;
        }
    }
}
