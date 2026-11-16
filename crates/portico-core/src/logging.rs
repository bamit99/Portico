use percent_encoding::{utf8_percent_encode, AsciiSet, CONTROLS};
use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

const CTRL_ENCODE_SET: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'%')
    .add(b'=')
    .add(b'\t');

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Info,
    Warn,
    Error,
}

impl LogLevel {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Info => "INFO",
            Self::Warn => "WARN",
            Self::Error => "ERROR",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogEntry {
    pub seq: u64,
    pub timestamp_ms: u64,
    pub level: LogLevel,
    pub target: String,
    pub message: String,
}

pub struct LogRingBuffer {
    cap: usize,
    next_seq: Mutex<u64>,
    entries: Mutex<VecDeque<LogEntry>>,
}

impl LogRingBuffer {
    pub fn new(capacity: usize) -> Self {
        Self {
            cap: capacity.max(1),
            next_seq: Mutex::new(1),
            entries: Mutex::new(VecDeque::with_capacity(capacity.max(1))),
        }
    }

    pub fn push(&self, level: LogLevel, target: &str, message: &str) -> LogEntry {
        let mut seq_guard = self.next_seq.lock().unwrap();
        let seq = *seq_guard;
        *seq_guard += 1;
        drop(seq_guard);

        let timestamp_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        let entry = LogEntry {
            seq,
            timestamp_ms,
            level,
            target: target.to_string(),
            message: message.to_string(),
        };

        let mut buf = self.entries.lock().unwrap();
        if buf.len() >= self.cap {
            buf.pop_front();
        }
        buf.push_back(entry.clone());
        entry
    }

    pub fn since(&self, after_seq: u64) -> Vec<LogEntry> {
        let buf = self.entries.lock().unwrap();
        buf.iter()
            .filter(|e| e.seq > after_seq)
            .cloned()
            .collect()
    }

    pub fn clear(&self) {
        self.entries.lock().unwrap().clear();
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DaemonEvent {
    DeviceAdded {
        bus_id: String,
        vid: u16,
        pid: u16,
        product: String,
        state: String,
    },
    DeviceRemoved {
        bus_id: String,
    },
    DeviceState {
        bus_id: String,
        state: String,
        peer: String,
    },
    ServerState {
        running: bool,
        port: u16,
    },
    RelayState {
        status: String,
        address: String,
    },
    LockChanged {
        net_lock: bool,
        wake_lock: bool,
    },
    Log(LogEntry),
}

impl DaemonEvent {
    pub fn to_control_line(&self) -> String {
        match self {
            DaemonEvent::DeviceAdded {
                bus_id,
                vid,
                pid,
                product,
                state,
            } => format!(
                "EVENT DEVICE_ADDED bus_id={} vid={:04X} pid={:04X} product={} state={}",
                pct_encode(bus_id),
                vid,
                pid,
                pct_encode(product),
                pct_encode(state)
            ),
            DaemonEvent::DeviceRemoved { bus_id } => {
                format!("EVENT DEVICE_REMOVED bus_id={}", pct_encode(bus_id))
            }
            DaemonEvent::DeviceState {
                bus_id,
                state,
                peer,
            } => format!(
                "EVENT DEVICE_STATE bus_id={} state={} peer={}",
                pct_encode(bus_id),
                pct_encode(state),
                pct_encode(peer)
            ),
            DaemonEvent::ServerState { running, port } => {
                format!("EVENT SERVER_STATE running={running} port={port}")
            }
            DaemonEvent::RelayState { status, address } => format!(
                "EVENT RELAY_STATE status={} address={}",
                pct_encode(status),
                pct_encode(address)
            ),
            DaemonEvent::LockChanged {
                net_lock,
                wake_lock,
            } => format!("EVENT LOCK_CHANGED net_lock={net_lock} wake_lock={wake_lock}"),
            DaemonEvent::Log(e) => format!(
                "EVENT LOG seq={} ts={} level={} target={} msg={}",
                e.seq,
                e.timestamp_ms,
                e.level.as_str(),
                pct_encode(&e.target),
                pct_encode(&e.message)
            ),
        }
    }
}

pub fn pct_encode(s: &str) -> String {
    utf8_percent_encode(s, CTRL_ENCODE_SET).to_string()
}
