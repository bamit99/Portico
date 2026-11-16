use crate::config::VidPid;
use crate::CoreError;
use portico_proto::frame::{DeviceSummary, TunnelStatus};
use std::collections::HashMap;
use std::sync::Arc;
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

struct RegisteredEntry {
    record: DeviceRecord,
    usb_device: Option<UsbDevice>,
}

pub struct DeviceRegistry {
    usbip: Arc<UsbIpServer>,
    devices: tokio::sync::Mutex<HashMap<String, RegisteredEntry>>,
}

impl DeviceRegistry {
    pub fn new() -> Self {
        Self {
            usbip: Arc::new(UsbIpServer::new_simulated(Vec::new())),
            devices: tokio::sync::Mutex::new(HashMap::new()),
        }
    }

    pub fn usbip_server(&self) -> Arc<UsbIpServer> {
        self.usbip.clone()
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
            product,
            serial,
            state,
        };

        // Replace existing entry if same bus_id re-registered
        let _ = self.usbip.remove_device(&bus_id).await;
        if !is_ign {
            self.usbip.add_device(dev.clone()).await;
        }

        self.devices.lock().await.insert(
            bus_id,
            RegisteredEntry {
                record: record.clone(),
                usb_device: Some(dev),
            },
        );
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
            product,
            serial: String::new(),
            state: DeviceState::PendingPermission,
        };
        self.devices.lock().await.insert(
            bus_id,
            RegisteredEntry {
                record: record.clone(),
                usb_device: None,
            },
        );
        record
    }

    pub async fn detach_device(&self, bus_id: &str) -> Result<DeviceRecord, CoreError> {
        let removed = self.devices.lock().await.remove(bus_id);
        let Some(entry) = removed else {
            return Err(CoreError::NotFound(format!("device {bus_id} not found")));
        };
        let _ = self.usbip.remove_device(bus_id).await;
        Ok(entry.record)
    }

    pub async fn apply_ignored_list(&self, ignored: &[VidPid]) -> Result<(), CoreError> {
        let mut guard = self.devices.lock().await;
        for entry in guard.values_mut() {
            let should_ignore =
                Self::is_ignored(entry.record.vid, entry.record.pid, ignored);
            match (&entry.record.state, should_ignore) {
                (DeviceState::Shared, true) => {
                    if self.usbip.remove_device(&entry.record.bus_id).await.is_ok() {
                        entry.record.state = DeviceState::Ignored;
                    }
                }
                (DeviceState::Ignored, false) => {
                    if let Some(dev) = entry.usb_device.as_ref() {
                        self.usbip.add_device(dev.clone()).await;
                        entry.record.state = DeviceState::Shared;
                    }
                }
                _ => {}
            }
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

    pub async fn mark_in_use(&self, bus_id: &str, peer: &str) {
        let mut guard = self.devices.lock().await;
        if let Some(entry) = guard.get_mut(bus_id) {
            entry.record.state = DeviceState::InUse(peer.to_string());
        }
    }

    pub async fn mark_released(&self, bus_id: &str, ignored: &[VidPid]) {
        let mut guard = self.devices.lock().await;
        if let Some(entry) = guard.get_mut(bus_id) {
            if Self::is_ignored(entry.record.vid, entry.record.pid, ignored) {
                if self.usbip.remove_device(bus_id).await.is_ok() {
                    entry.record.state = DeviceState::Ignored;
                    return;
                }
            }
            if matches!(entry.record.state, DeviceState::InUse(_)) {
                entry.record.state = DeviceState::Shared;
            }
        }
    }
}

impl Default for DeviceRegistry {
    fn default() -> Self {
        Self::new()
    }
}
