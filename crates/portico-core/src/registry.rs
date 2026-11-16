use crate::config::VidPid;
use crate::logging::DaemonEvent;
use crate::CoreError;
use portico_proto::frame::{DeviceSummary, TunnelStatus};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{broadcast, watch, Notify};
use usbip::{UsbDevice, UsbIpServer};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceState {
    Shared,
    InUse(String),
    Ignored,
    PendingPermission,
}

impl DeviceState {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Shared => "shared",
            Self::InUse(_) => "in_use",
            Self::Ignored => "ignored",
            Self::PendingPermission => "pending_perm",
        }
    }

    pub fn peer(&self) -> &str {
        match self {
            Self::InUse(p) => p.as_str(),
            _ => "-",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceRecord {
    pub bus_id: String,
    pub vid: u16,
    pub pid: u16,
    pub device_class: u8,
    pub device_subclass: u8,
    pub device_protocol: u8,
    pub speed: u32,
    pub manufacturer: String,
    pub product: String,
    pub serial: String,
    pub state: DeviceState,
}

impl DeviceRecord {
    pub fn to_summary(&self) -> DeviceSummary {
        DeviceSummary {
            bus_id: self.bus_id.clone(),
            vid: self.vid,
            pid: self.pid,
            device_class: self.device_class,
            device_subclass: self.device_subclass,
            device_protocol: self.device_protocol,
            speed: self.speed,
            manufacturer: self.manufacturer.clone(),
            product: self.product.clone(),
            serial: self.serial.clone(),
            in_use: matches!(self.state, DeviceState::InUse(_)),
        }
    }
}

pub struct ClaimedDevice {
    pub bus_id: String,
    pub usbip_server: Arc<UsbIpServer>,
    pub cancel: Arc<Notify>,
    pub done_tx: Arc<watch::Sender<bool>>,
}

struct RegisteredEntry {
    record: DeviceRecord,
    usb_device: Option<UsbDevice>,
    active_cancel: Option<Arc<Notify>>,
    active_done_rx: Option<watch::Receiver<bool>>,
}

pub struct DeviceRegistry {
    usbip: Arc<UsbIpServer>,
    devices: tokio::sync::Mutex<HashMap<String, RegisteredEntry>>,
    events_tx: Option<broadcast::Sender<DaemonEvent>>,
}

impl DeviceRegistry {
    pub fn new() -> Self {
        Self {
            usbip: Arc::new(UsbIpServer::new_simulated(Vec::new())),
            devices: tokio::sync::Mutex::new(HashMap::new()),
            events_tx: None,
        }
    }

    pub fn with_events(events_tx: broadcast::Sender<DaemonEvent>) -> Self {
        Self {
            usbip: Arc::new(UsbIpServer::new_simulated(Vec::new())),
            devices: tokio::sync::Mutex::new(HashMap::new()),
            events_tx: Some(events_tx),
        }
    }

    pub fn usbip_server(&self) -> Arc<UsbIpServer> {
        self.usbip.clone()
    }

    fn emit(&self, ev: DaemonEvent) {
        if let Some(tx) = &self.events_tx {
            let _ = tx.send(ev);
        }
    }

    fn is_ignored(vid: u16, pid: u16, ignored: &[VidPid]) -> bool {
        ignored.iter().any(|vp| vp.vid == vid && vp.pid == pid)
    }

    pub async fn register_usb_device(
        &self,
        dev: UsbDevice,
        manufacturer: String,
        product: String,
        serial: String,
        ignored: &[VidPid],
    ) -> Result<DeviceRecord, CoreError> {
        let bus_id = dev.bus_id.clone();
        let is_ign = Self::is_ignored(dev.vendor_id, dev.product_id, ignored);
        let state = if is_ign {
            DeviceState::Ignored
        } else {
            DeviceState::Shared
        };
        let record = DeviceRecord {
            bus_id: bus_id.clone(),
            vid: dev.vendor_id,
            pid: dev.product_id,
            device_class: dev.device_class,
            device_subclass: dev.device_subclass,
            device_protocol: dev.device_protocol,
            speed: dev.speed,
            manufacturer,
            product: product.clone(),
            serial,
            state: state.clone(),
        };

        let mut wait_done: Option<(Arc<Notify>, watch::Receiver<bool>)> = None;
        let mut guard = self.devices.lock().await;
        if let Some(prev) = guard.remove(&bus_id) {
            if let (Some(cancel), Some(done_rx)) = (prev.active_cancel, prev.active_done_rx) {
                wait_done = Some((cancel, done_rx));
            }
        }
        drop(guard);
        if let Some((cancel, mut done_rx)) = wait_done {
            cancel.notify_waiters();
            let _ = tokio::time::timeout(
                std::time::Duration::from_secs(2),
                done_rx.wait_for(|&done| done),
            )
            .await;
        }
        let mut guard = self.devices.lock().await;
        let _ = self.usbip.remove_device(&bus_id).await;
        if !is_ign {
            self.usbip.add_device(dev.clone()).await;
        }

        guard.insert(
            bus_id.clone(),
            RegisteredEntry {
                record: record.clone(),
                usb_device: Some(dev),
                active_cancel: None,
                active_done_rx: None,
            },
        );
        drop(guard);

        self.emit(DaemonEvent::DeviceAdded {
            bus_id,
            vid: record.vid,
            pid: record.pid,
            product,
            state: state.label().to_string(),
        });
        Ok(record)
    }

    pub async fn register_pending_permission(
        &self,
        bus_id: String,
        vid: u16,
        pid: u16,
        product: String,
    ) -> DeviceRecord {
        let record = DeviceRecord {
            bus_id: bus_id.clone(),
            vid,
            pid,
            device_class: 0,
            device_subclass: 0,
            device_protocol: 0,
            speed: 0,
            manufacturer: String::new(),
            product: product.clone(),
            serial: String::new(),
            state: DeviceState::PendingPermission,
        };
        self.devices.lock().await.insert(
            bus_id.clone(),
            RegisteredEntry {
                record: record.clone(),
                usb_device: None,
                active_cancel: None,
                active_done_rx: None,
            },
        );
        self.emit(DaemonEvent::DeviceAdded {
            bus_id,
            vid,
            pid,
            product,
            state: DeviceState::PendingPermission.label().to_string(),
        });
        record
    }

    pub async fn detach_device(&self, bus_id: &str) -> Result<DeviceRecord, CoreError> {
        let removed = self.devices.lock().await.remove(bus_id);
        let Some(entry) = removed else {
            return Err(CoreError::NotFound(format!("device {bus_id} not found")));
        };
        if let Some(cancel) = entry.active_cancel {
            cancel.notify_waiters();
        }
        if let Some(mut done_rx) = entry.active_done_rx {
            let _ = tokio::time::timeout(
                std::time::Duration::from_secs(2),
                done_rx.wait_for(|&done| done),
            )
            .await;
        }
        let _ = self.usbip.remove_device(bus_id).await;
        self.emit(DaemonEvent::DeviceRemoved {
            bus_id: bus_id.to_string(),
        });
        Ok(entry.record)
    }

    pub async fn cancel_claim(&self, bus_id: &str, ignored: &[VidPid]) -> Result<(), CoreError> {
        let (cancel, done_rx) = {
            let guard = self.devices.lock().await;
            let Some(entry) = guard.get(bus_id) else {
                return Err(CoreError::NotFound(format!("device {bus_id} not found")));
            };
            (entry.active_cancel.clone(), entry.active_done_rx.clone())
        };
        if let Some(c) = cancel {
            c.notify_waiters();
        }
        if let Some(mut rx) = done_rx {
            let _ = tokio::time::timeout(
                std::time::Duration::from_secs(2),
                rx.wait_for(|&done| done),
            )
            .await;
        }
        self.release_claim(bus_id, ignored).await;
        Ok(())
    }

    pub async fn apply_ignored_list(&self, ignored: &[VidPid]) -> Result<(), CoreError> {
        let mut events = Vec::new();
        {
            let mut guard = self.devices.lock().await;
            for entry in guard.values_mut() {
                let should_ignore =
                    Self::is_ignored(entry.record.vid, entry.record.pid, ignored);
                match (&entry.record.state, should_ignore) {
                    (DeviceState::Shared, true) => {
                        let _ = self.usbip.remove_device(&entry.record.bus_id).await;
                        entry.record.state = DeviceState::Ignored;
                        events.push(DaemonEvent::DeviceState {
                            bus_id: entry.record.bus_id.clone(),
                            state: "ignored".into(),
                            peer: "-".into(),
                        });
                    }
                    (DeviceState::Ignored, false) => {
                        if let Some(dev) = entry.usb_device.as_ref() {
                            self.usbip.add_device(dev.clone()).await;
                            entry.record.state = DeviceState::Shared;
                            events.push(DaemonEvent::DeviceState {
                                bus_id: entry.record.bus_id.clone(),
                                state: "shared".into(),
                                peer: "-".into(),
                            });
                        }
                    }
                    _ => {}
                }
            }
        }
        for ev in events {
            self.emit(ev);
        }
        Ok(())
    }

    pub async fn list_devices(&self) -> Vec<DeviceRecord> {
        let guard = self.devices.lock().await;
        let mut list: Vec<DeviceRecord> = guard.values().map(|e| e.record.clone()).collect();
        list.sort_by(|a, b| a.bus_id.cmp(&b.bus_id));
        list
    }

    pub async fn list_shareable_summaries(&self) -> Vec<DeviceSummary> {
        let guard = self.devices.lock().await;
        let mut list: Vec<DeviceSummary> = guard
            .values()
            .filter(|e| matches!(e.record.state, DeviceState::Shared | DeviceState::InUse(_)))
            .map(|e| e.record.to_summary())
            .collect();
        list.sort_by(|a, b| a.bus_id.cmp(&b.bus_id));
        list
    }

    pub async fn check_tunnel_eligible(&self, bus_id: &str) -> Result<(), TunnelStatus> {
        let guard = self.devices.lock().await;
        let Some(entry) = guard.get(bus_id) else {
            return Err(TunnelStatus::NotFound);
        };
        match entry.record.state {
            DeviceState::Shared => Ok(()),
            DeviceState::InUse(_) => Err(TunnelStatus::InUse),
            DeviceState::Ignored | DeviceState::PendingPermission => Err(TunnelStatus::Ignored),
        }
    }

    pub async fn try_claim_device(
        &self,
        bus_id: &str,
        peer: &str,
    ) -> Result<ClaimedDevice, TunnelStatus> {
        let mut guard = self.devices.lock().await;
        let Some(entry) = guard.get_mut(bus_id) else {
            return Err(TunnelStatus::NotFound);
        };
        match entry.record.state {
            DeviceState::Shared => {}
            DeviceState::InUse(_) => return Err(TunnelStatus::InUse),
            DeviceState::Ignored | DeviceState::PendingPermission => {
                return Err(TunnelStatus::Ignored);
            }
        }
        let Some(dev) = entry.usb_device.as_ref() else {
            return Err(TunnelStatus::NotFound);
        };
        let tunnel_server = Arc::new(UsbIpServer::new_simulated(vec![dev.clone()]));
        let cancel = Arc::new(Notify::new());
        let (done_tx, done_rx) = watch::channel(false);
        let done_tx = Arc::new(done_tx);
        entry.record.state = DeviceState::InUse(peer.to_string());
        entry.active_cancel = Some(cancel.clone());
        entry.active_done_rx = Some(done_rx);
        drop(guard);

        self.emit(DaemonEvent::DeviceState {
            bus_id: bus_id.to_string(),
            state: "in_use".into(),
            peer: peer.to_string(),
        });

        Ok(ClaimedDevice {
            bus_id: bus_id.to_string(),
            usbip_server: tunnel_server,
            cancel,
            done_tx,
        })
    }

    pub async fn release_claim(&self, bus_id: &str, ignored: &[VidPid]) {
        let dev_to_release = {
            let guard = self.devices.lock().await;
            guard.get(bus_id).and_then(|e| e.usb_device.clone())
        };
        if let Some(dev) = dev_to_release {
            let _ = tokio::task::spawn_blocking(move || {
                dev.release_tunnel_handlers();
            })
            .await;
        }

        let ev = {
            let mut guard = self.devices.lock().await;
            let Some(entry) = guard.get_mut(bus_id) else {
                return;
            };
            entry.active_cancel = None;
            entry.active_done_rx = None;
            if Self::is_ignored(entry.record.vid, entry.record.pid, ignored) {
                let _ = self.usbip.remove_device(bus_id).await;
                entry.record.state = DeviceState::Ignored;
            } else if matches!(entry.record.state, DeviceState::InUse(_)) {
                entry.record.state = DeviceState::Shared;
            }
            DaemonEvent::DeviceState {
                bus_id: bus_id.to_string(),
                state: entry.record.state.label().to_string(),
                peer: "-".into(),
            }
        };
        self.emit(ev);
    }
}

impl Default for DeviceRegistry {
    fn default() -> Self {
        Self::new()
    }
}
