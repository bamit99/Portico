use crate::config::ConfigStore;
use crate::logging::{DaemonEvent, LogLevel, LogRingBuffer};
use crate::registry::DeviceRegistry;
use crate::CoreError;
use portico_proto::identity::NodeIdentity;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicUsize;
use std::sync::Arc;
use tokio::sync::{broadcast, Mutex, RwLock};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RelayStatus {
    Disabled,
    Initializing,
    Ready(String),
    Error(String),
}

impl RelayStatus {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::Initializing => "initializing",
            Self::Ready(_) => "ready",
            Self::Error(_) => "error",
        }
    }

    pub fn address(&self) -> &str {
        match self {
            Self::Ready(addr) => addr.as_str(),
            _ => "",
        }
    }
}

pub struct DaemonState {
    pub data_dir: PathBuf,
    pub config: Mutex<ConfigStore>,
    pub identity: RwLock<NodeIdentity>,
    pub registry: Arc<DeviceRegistry>,
    pub logs: Arc<LogRingBuffer>,
    pub events_tx: broadcast::Sender<DaemonEvent>,
    pub server_running: AtomicBool,
    pub active_peers: AtomicUsize,
    pub relay_status: RwLock<RelayStatus>,
    pub registered_clients: Mutex<std::collections::HashMap<String, String>>,
}

impl DaemonState {
    pub fn new(data_dir: PathBuf) -> Result<Arc<Self>, CoreError> {
        let store = ConfigStore::load_or_init(&data_dir)?;
        let identity = NodeIdentity::load_or_generate(&data_dir, "identity", "portico-node")?;
        let (events_tx, _) = broadcast::channel(256);
        Ok(Arc::new(Self {
            data_dir,
            config: Mutex::new(store),
            identity: RwLock::new(identity),
            registry: Arc::new(DeviceRegistry::new()),
            logs: Arc::new(LogRingBuffer::new(1000)),
            events_tx,
            server_running: AtomicBool::new(false),
            active_peers: AtomicUsize::new(0),
            relay_status: RwLock::new(RelayStatus::Disabled),
            registered_clients: Mutex::new(std::collections::HashMap::new()),
        }))
    }

    pub fn log_and_emit(&self, level: LogLevel, target: &str, message: &str) {
        let entry = self.logs.push(level, target, message);
        let _ = self.events_tx.send(DaemonEvent::Log(entry));
    }

    pub fn emit(&self, ev: DaemonEvent) {
        let _ = self.events_tx.send(ev);
    }
}
