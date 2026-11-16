//! A library for running a USB/IP server

use log::*;
use num_derive::FromPrimitive;
use num_traits::FromPrimitive;
#[cfg(not(target_os = "android"))]
use nusb::MaybeFuture;
use rusb::*;
use std::any::Any;
use std::collections::{HashMap, VecDeque};
use std::io::{ErrorKind, Result};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use tokio::sync::{mpsc, Notify, RwLock};
use tokio::task::JoinHandle;
use usbip_protocol::UsbIpCommand;

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

pub mod cdc;
mod consts;
mod device;
mod endpoint;
pub mod hid;
mod host;
mod interface;
mod setup;
pub mod usbip_protocol;
mod util;
pub use consts::*;
pub use device::*;
pub use endpoint::*;
pub use host::*;
pub use interface::*;
pub use setup::*;
pub use util::*;

use crate::usbip_protocol::{USBIP_RET_SUBMIT, USBIP_RET_UNLINK, UsbIpResponse};

/// Main struct of a USB/IP server
#[derive(Default, Debug)]
pub struct UsbIpServer {
    available_devices: RwLock<Vec<UsbDevice>>,
    used_devices: RwLock<HashMap<String, UsbDevice>>,
}

impl UsbIpServer {
    /// Create a [UsbIpServer] with simulated devices
    pub fn new_simulated(devices: Vec<UsbDevice>) -> Self {
        Self {
            available_devices: RwLock::new(devices),
            used_devices: RwLock::new(HashMap::new()),
        }
    }

    /// Create a [UsbIpServer] with Vec<[nusb::DeviceInfo]> for sharing host devices
    #[cfg(not(target_os = "android"))]
    pub fn with_nusb_devices(nusb_device_infos: Vec<nusb::DeviceInfo>) -> Vec<UsbDevice> {
        let mut devices = vec![];
        for device_info in nusb_device_infos {
            let dev = match device_info.open().wait() {
                Ok(dev) => dev,
                Err(err) => {
                    warn!("Impossible to open device {device_info:?}: {err}, ignoring device",);
                    continue;
                }
            };
            let cfg = match dev.active_configuration() {
                Ok(cfg) => cfg,
                Err(err) => {
                    warn!(
                        "Impossible to get active configuration {device_info:?}: {err}, ignoring device",
                    );
                    continue;
                }
            };
            let mut interfaces = vec![];
            for intf in cfg.interfaces() {
                // ignore alternate settings
                let intf_num = intf.interface_number();
                let intf = dev.claim_interface(intf_num).wait().unwrap();
                let alt_setting = intf.descriptors().next().unwrap();
                let mut endpoints = vec![];

                for ep_desc in alt_setting.endpoints() {
                    endpoints.push(UsbEndpoint {
                        address: ep_desc.address(),
                        attributes: ep_desc.transfer_type() as u8,
                        max_packet_size: ep_desc.max_packet_size() as u16,
                        interval: ep_desc.interval(),
                    });
                }

                let handler = Arc::new(Mutex::new(Box::new(NusbUsbHostInterfaceHandler::new(
                    Arc::new(Mutex::new(intf.clone())),
                ))
                    as Box<dyn UsbInterfaceHandler + Send>));
                interfaces.push(UsbInterface {
                    interface_class: alt_setting.class(),
                    interface_subclass: alt_setting.subclass(),
                    interface_protocol: alt_setting.protocol(),
                    endpoints,
                    string_interface: alt_setting.string_index().map(|nz| nz.get()).unwrap_or(0),
                    class_specific_descriptor: Vec::new(),
                    endpoint_extras: Vec::new(),
                    handler,
                });
            }

            // Platform-specific bus number (Linux-only)
            let bus_num_val: u32;
            #[cfg(target_os = "linux")]
            {
                bus_num_val = device_info.busnum() as u32;
            }
            #[cfg(not(target_os = "linux"))]
            {
                bus_num_val = 0;
            }

            let device_address = device_info.device_address();

            let mut device = UsbDevice {
                path: format!("/sys/bus/{}/{}/{}", bus_num_val, device_address, 0),
                bus_id: format!("{}-{}-{}", bus_num_val, device_address, 0),
                bus_num: bus_num_val,
                dev_num: 0,
                speed: device_info.speed().unwrap() as u32,
                vendor_id: device_info.vendor_id(),
                product_id: device_info.product_id(),
                device_class: device_info.class(),
                device_subclass: device_info.subclass(),
                device_protocol: device_info.protocol(),
                device_bcd: device_info.device_version().into(),
                configuration_value: cfg.configuration_value(),
                num_configurations: dev.configurations().count() as u8,
                ep0_in: UsbEndpoint {
                    address: 0x80,
                    attributes: EndpointAttributes::Control as u8,
                    max_packet_size: 16,
                    interval: 0,
                },
                ep0_out: UsbEndpoint {
                    address: 0x00,
                    attributes: EndpointAttributes::Control as u8,
                    max_packet_size: 16,
                    interval: 0,
                },
                interfaces,
                device_handler: Some(Arc::new(Mutex::new(Box::new(
                    NusbUsbHostDeviceHandler::new(Arc::new(Mutex::new(dev))),
                )))),
                ..UsbDevice::default()
            };

            // set strings
            if let Some(s) = device_info.manufacturer_string() {
                device.string_manufacturer = device.new_string(s)
            }
            if let Some(s) = device_info.product_string() {
                device.string_product = device.new_string(s)
            }
            if let Some(s) = device_info.serial_number() {
                device.string_serial = device.new_string(s)
            }
            devices.push(device);
        }
        devices
    }

    /// Create a [UsbIpServer] with Vec<[rusb::DeviceHandle]> for sharing host devices
    pub fn with_rusb_device_handles(
        device_handles: Vec<DeviceHandle<GlobalContext>>,
    ) -> Vec<UsbDevice> {
        let mut devices = vec![];
        for open_device in device_handles {
            let dev = open_device.device();
            let desc = match dev.device_descriptor() {
                Ok(desc) => desc,
                Err(err) => {
                    warn!(
                        "Impossible to get device descriptor for {dev:?}: {err}, ignoring device",
                    );
                    continue;
                }
            };
            let cfg = match dev.active_config_descriptor() {
                Ok(desc) => desc,
                Err(err) => {
                    warn!(
                        "Impossible to get config descriptor for {dev:?}: {err}, ignoring device",
                    );
                    continue;
                }
            };

            let handle = Arc::new(Mutex::new(open_device));
            let mut interfaces = vec![];
            handle
                .lock()
                .unwrap()
                .set_auto_detach_kernel_driver(true)
                .ok();
            for intf in cfg.interfaces() {
                // ignore alternate settings
                let intf_desc = intf.descriptors().next().unwrap();
                handle
                    .lock()
                    .unwrap()
                    .set_auto_detach_kernel_driver(true)
                    .ok();
                let mut endpoints = vec![];

                for ep_desc in intf_desc.endpoint_descriptors() {
                    endpoints.push(UsbEndpoint {
                        address: ep_desc.address(),
                        attributes: ep_desc.transfer_type() as u8,
                        max_packet_size: ep_desc.max_packet_size(),
                        interval: ep_desc.interval(),
                    });
                }

                let handler = Arc::new(Mutex::new(Box::new(RusbUsbHostInterfaceHandler::new(
                    handle.clone(),
                ))
                    as Box<dyn UsbInterfaceHandler + Send>));
                interfaces.push(UsbInterface {
                    interface_class: intf_desc.class_code(),
                    interface_subclass: intf_desc.sub_class_code(),
                    interface_protocol: intf_desc.protocol_code(),
                    endpoints,
                    string_interface: intf_desc.description_string_index().unwrap_or(0),
                    class_specific_descriptor: Vec::from(intf_desc.extra()),
                    endpoint_extras: Vec::new(),
                    handler,
                });
            }
            let mut device = UsbDevice {
                path: format!(
                    "/sys/bus/{}/{}/{}",
                    dev.bus_number(),
                    dev.address(),
                    dev.port_number()
                ),
                bus_id: format!(
                    "{}-{}-{}",
                    dev.bus_number(),
                    dev.address(),
                    dev.port_number()
                ),
                bus_num: dev.bus_number() as u32,
                dev_num: dev.port_number() as u32,
                speed: dev.speed() as u32,
                vendor_id: desc.vendor_id(),
                product_id: desc.product_id(),
                device_class: desc.class_code(),
                device_subclass: desc.sub_class_code(),
                device_protocol: desc.protocol_code(),
                device_bcd: desc.device_version().into(),
                configuration_value: cfg.number(),
                num_configurations: desc.num_configurations(),
                ep0_in: UsbEndpoint {
                    address: 0x80,
                    attributes: EndpointAttributes::Control as u8,
                    max_packet_size: desc.max_packet_size() as u16,
                    interval: 0,
                },
                ep0_out: UsbEndpoint {
                    address: 0x00,
                    attributes: EndpointAttributes::Control as u8,
                    max_packet_size: desc.max_packet_size() as u16,
                    interval: 0,
                },
                interfaces,
                device_handler: Some(Arc::new(Mutex::new(Box::new(
                    RusbUsbHostDeviceHandler::new(handle.clone()),
                )))),
                usb_version: desc.usb_version().into(),
                ..UsbDevice::default()
            };

            // set strings
            //
            // Devices are not required to respond successfully to a string descriptor
            // read (some transiently NAK it, some don't implement the language ID we
            // ask for). A failure here used to be treated as fatal via .unwrap(), which
            // crashed enumeration - and with it every other device - instead of just
            // leaving that one string unset.
            if let Some(index) = desc.manufacturer_string_index() {
                match handle.lock().unwrap().read_string_descriptor_ascii(index) {
                    Ok(s) => device.string_manufacturer = device.new_string(&s),
                    Err(err) => warn!(
                        "[{:04x}:{:04x} {}] failed to read manufacturer string descriptor (index={index}): {err}",
                        device.vendor_id, device.product_id, device.bus_id
                    ),
                }
            }
            if let Some(index) = desc.product_string_index() {
                match handle.lock().unwrap().read_string_descriptor_ascii(index) {
                    Ok(s) => device.string_product = device.new_string(&s),
                    Err(err) => warn!(
                        "[{:04x}:{:04x} {}] failed to read product string descriptor (index={index}): {err}",
                        device.vendor_id, device.product_id, device.bus_id
                    ),
                }
            }
            if let Some(index) = desc.serial_number_string_index() {
                match handle.lock().unwrap().read_string_descriptor_ascii(index) {
                    Ok(s) => device.string_serial = device.new_string(&s),
                    Err(err) => warn!(
                        "[{:04x}:{:04x} {}] failed to read serial number string descriptor (index={index}): {err}",
                        device.vendor_id, device.product_id, device.bus_id
                    ),
                }
            }
            devices.push(device);
        }
        devices
    }

    fn with_rusb_devices(device_list: Vec<Device<GlobalContext>>) -> Vec<UsbDevice> {
        let mut device_handles = vec![];

        for dev in device_list {
            let open_device = match dev.open() {
                Ok(dev) => dev,
                Err(err) => {
                    warn!("Impossible to share {dev:?}: {err}, ignoring device");
                    continue;
                }
            };
            device_handles.push(open_device);
        }
        Self::with_rusb_device_handles(device_handles)
    }

    /// Create a [UsbIpServer] exposing devices in the host, and redirect all USB transfers to them using libusb
    pub fn new_from_host() -> Self {
        Self::new_from_host_with_filter(|_| true)
    }

    /// Create a [UsbIpServer] exposing filtered devices in the host, and redirect all USB transfers to them using libusb
    pub fn new_from_host_with_filter<F>(filter: F) -> Self
    where
        F: FnMut(&Device<GlobalContext>) -> bool,
    {
        match rusb::devices() {
            Ok(list) => {
                let mut devs = vec![];
                for d in list.iter().filter(filter) {
                    devs.push(d)
                }
                Self {
                    available_devices: RwLock::new(Self::with_rusb_devices(devs)),
                    ..Default::default()
                }
            }
            Err(_) => Default::default(),
        }
    }

    pub async fn add_device(&self, device: UsbDevice) {
        self.available_devices.write().await.push(device);
    }

    pub async fn remove_device(&self, bus_id: &str) -> Result<()> {
        let mut available_devices = self.available_devices.write().await;

        if let Some(device) = available_devices.iter().position(|d| d.bus_id == bus_id) {
            available_devices.remove(device);
            Ok(())
        } else if let Some(device) = self
            .used_devices
            .read()
            .await
            .values()
            .find(|d| d.bus_id == bus_id)
        {
            Err(std::io::Error::other(format!(
                "Device {} is in use",
                device.bus_id
            )))
        } else {
            Err(std::io::Error::new(
                ErrorKind::NotFound,
                format!("Device {bus_id} not found"),
            ))
        }
    }
}

fn map_io_err_to_usbip_errno(err: &std::io::Error) -> i32 {
    match err.kind() {
        ErrorKind::BrokenPipe => -32,     // -EPIPE (endpoint stall)
        ErrorKind::TimedOut => -110,      // -ETIMEDOUT
        ErrorKind::NotConnected => -19,   // -ENODEV
        ErrorKind::InvalidData => -75,    // -EOVERFLOW
        ErrorKind::InvalidInput => -22,   // -EINVAL
        ErrorKind::ConnectionReset | ErrorKind::ConnectionAborted => -104, // -ECONNRESET
        _ => -71,                         // -EPROTO
    }
}

struct UrbJob {
    device: UsbDevice,
    ep: UsbEndpoint,
    intf: Option<UsbInterface>,
    header: usbip_protocol::UsbIpHeaderBasic,
    transfer_buffer_length: u32,
    setup: SetupPacket,
    data: Vec<u8>,
    out: bool,
    cancelled: Arc<AtomicBool>,
}

struct InFlightGuard {
    seqnum: u32,
    pending_unlinks: Arc<Mutex<HashMap<u32, Arc<AtomicBool>>>>,
    in_flight: Arc<AtomicUsize>,
    zero_notify: Arc<Notify>,
}

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        if let Ok(mut map) = self.pending_unlinks.lock() {
            map.remove(&self.seqnum);
        }
        if self.in_flight.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.zero_notify.notify_waiters();
        }
    }
}

struct HandlerGuard {
    server: Arc<UsbIpServer>,
    imported_id: Arc<Mutex<Option<String>>>,
    worker_handles: Vec<JoinHandle<()>>,
    cleaned: bool,
}

impl Drop for HandlerGuard {
    fn drop(&mut self) {
        for h in self.worker_handles.drain(..) {
            h.abort();
        }
        if self.cleaned {
            return;
        }
        let dev_id = self
            .imported_id
            .lock()
            .ok()
            .and_then(|mut g| g.take());
        if let Some(dev_id) = dev_id {
            if let (Ok(mut used), Ok(mut avail)) = (
                self.server.used_devices.try_write(),
                self.server.available_devices.try_write(),
            ) {
                if let Some(dev) = used.remove(&dev_id) {
                    dev.release_tunnel_handlers();
                    avail.push(dev);
                }
            } else {
                let server = self.server.clone();
                if let Ok(handle) = tokio::runtime::Handle::try_current() {
                    handle.spawn(async move {
                        let mut used = server.used_devices.write().await;
                        let mut avail = server.available_devices.write().await;
                        if let Some(dev) = used.remove(&dev_id) {
                            dev.release_tunnel_handlers();
                            avail.push(dev);
                        }
                    });
                }
            }
        }
    }
}

async fn release_imported_device(
    server: &Arc<UsbIpServer>,
    imported_id: &Arc<Mutex<Option<String>>>,
) {
    let dev_id = imported_id.lock().ok().and_then(|mut g| g.take());
    if let Some(dev_id) = dev_id {
        let removed = {
            let mut used = server.used_devices.write().await;
            used.remove(&dev_id)
        };
        if let Some(dev) = removed {
            let dev_clone = dev.clone();
            let _ = tokio::task::spawn_blocking(move || {
                dev_clone.release_tunnel_handlers();
            })
            .await;
            server.available_devices.write().await.push(dev);
        }
    }
}

pub async fn handler<T: AsyncReadExt + AsyncWriteExt + Unpin>(
    socket: &mut T,
    server: Arc<UsbIpServer>,
) -> Result<()> {
    let (mut reader, mut writer) = tokio::io::split(socket);
    let (resp_tx, mut resp_rx) = mpsc::channel::<UsbIpResponse>(128);
    let mut resp_tx_opt = Some(resp_tx);

    let imported_id = Arc::new(Mutex::new(None::<String>));
    let mut current_device: Option<UsbDevice> = None;
    let mut ep_workers: HashMap<u8, mpsc::Sender<UrbJob>> = HashMap::new();
    let pending_unlinks: Arc<Mutex<HashMap<u32, Arc<AtomicBool>>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let in_flight = Arc::new(AtomicUsize::new(0));
    let zero_notify = Arc::new(Notify::new());

    let mut guard = HandlerGuard {
        server: server.clone(),
        imported_id: imported_id.clone(),
        worker_handles: Vec::new(),
        cleaned: false,
    };

    let write_fut = async {
        while let Some(first) = resp_rx.recv().await {
            first.write_to_socket(&mut writer).await?;
            while let Ok(next) = resp_rx.try_recv() {
                next.write_to_socket(&mut writer).await?;
            }
            writer.flush().await?;
        }
        Ok::<(), std::io::Error>(())
    };
    tokio::pin!(write_fut);

    let read_res: Result<()> = loop {
        let cmd_res = tokio::select! {
            r = UsbIpCommand::read_from_socket(&mut reader) => r,
            w = &mut write_fut => {
                guard.worker_handles.iter().for_each(|h| h.abort());
                release_imported_device(&server, &imported_id).await;
                guard.cleaned = true;
                return w;
            }
        };

        let command = match cmd_res {
            Ok(c) => c,
            Err(err) => break Err(err),
        };

        let Some(ref tx) = resp_tx_opt else {
            break Ok(());
        };

        match command {
            UsbIpCommand::OpReqDevlist { .. } => {
                trace!("Got OP_REQ_DEVLIST");
                let devices = server.available_devices.read().await;
                let resp = UsbIpResponse::op_rep_devlist(&devices);
                drop(devices);
                if tx.send(resp).await.is_err() {
                    break Err(std::io::Error::new(
                        ErrorKind::BrokenPipe,
                        "response writer closed",
                    ));
                }
                trace!("Queued OP_REP_DEVLIST");
            }
            UsbIpCommand::OpReqImport { busid, .. } => {
                trace!("Got OP_REQ_IMPORT");
                ep_workers.clear();
                for h in guard.worker_handles.drain(..) {
                    h.abort();
                }
                current_device = None;
                release_imported_device(&server, &imported_id).await;

                let mut used_devices = server.used_devices.write().await;
                let mut available_devices = server.available_devices.write().await;
                let busid_compare =
                    &busid[..busid.iter().position(|&x| x == 0).unwrap_or(busid.len())];
                for (i, dev) in available_devices.iter().enumerate() {
                    if busid_compare == dev.bus_id.as_bytes() {
                        let dev = available_devices.remove(i);
                        let dev_id = dev.bus_id.clone();
                        current_device = Some(dev.clone());
                        used_devices.insert(dev_id.clone(), dev);
                        if let Ok(mut g) = imported_id.lock() {
                            *g = Some(dev_id);
                        }
                        break;
                    }
                }
                drop(available_devices);
                drop(used_devices);

                let res = if let Some(ref dev) = current_device {
                    UsbIpResponse::op_rep_import_success(dev)
                } else {
                    UsbIpResponse::op_rep_import_fail()
                };
                if tx.send(res).await.is_err() {
                    break Err(std::io::Error::new(
                        ErrorKind::BrokenPipe,
                        "response writer closed",
                    ));
                }
                trace!("Queued OP_REP_IMPORT");
            }
            UsbIpCommand::UsbIpCmdSubmit {
                mut header,
                transfer_buffer_length,
                setup,
                data,
                ..
            } => {
                trace!("Got USBIP_CMD_SUBMIT seq={}", header.seqnum);
                let Some(ref device) = current_device else {
                    header.command = USBIP_RET_SUBMIT.into();
                    let _ = tx
                        .send(UsbIpResponse::usbip_ret_submit_err(&header, -19))
                        .await;
                    continue;
                };

                let out = header.direction == 0;
                let real_ep = (if out { header.ep } else { header.ep | 0x80 }) as u8;
                header.command = USBIP_RET_SUBMIT.into();

                let Some((ep, intf)) = device.find_ep(real_ep) else {
                    warn!("Endpoint {real_ep:02x?} not found");
                    if tx
                        .send(UsbIpResponse::usbip_ret_submit_err(&header, -22))
                        .await
                        .is_err()
                    {
                        break Err(std::io::Error::new(
                            ErrorKind::BrokenPipe,
                            "response writer closed",
                        ));
                    }
                    continue;
                };

                let ep_tx = if let Some(existing) = ep_workers.get(&real_ep) {
                    existing.clone()
                } else {
                    let (wtx, mut wrx) = mpsc::channel::<UrbJob>(32);
                    let rtx = tx.clone();
                    let unlinks = pending_unlinks.clone();
                    let inf = in_flight.clone();
                    let znot = zero_notify.clone();
                    let jh = tokio::spawn(async move {
                        while let Some(job) = wrx.recv().await {
                            let _inflight_guard = InFlightGuard {
                                seqnum: job.header.seqnum,
                                pending_unlinks: unlinks.clone(),
                                in_flight: inf.clone(),
                                zero_notify: znot.clone(),
                            };
                            let res = if job.cancelled.load(Ordering::Acquire) {
                                UsbIpResponse::usbip_ret_submit_err(&job.header, -104)
                            } else {
                                let resp = job
                                    .device
                                    .handle_urb(
                                        job.ep,
                                        job.intf.as_ref(),
                                        job.transfer_buffer_length,
                                        job.setup,
                                        &job.data,
                                    )
                                    .await;
                                match resp {
                                    Ok(resp) => {
                                        let actual_length = if job.out {
                                            debug_assert!(
                                                resp.is_empty(),
                                                "OUT transfer should return empty response buffer"
                                            );
                                            job.data.len() as u32
                                        } else {
                                            resp.len() as u32
                                        };
                                        UsbIpResponse::usbip_ret_submit_success(
                                            &job.header,
                                            0,
                                            0,
                                            actual_length,
                                            resp,
                                            vec![],
                                        )
                                    }
                                    Err(err) => {
                                        let errno = map_io_err_to_usbip_errno(&err);
                                        warn!("Error handling URB seq={} ep={:#04x}: {err} (errno={errno})", job.header.seqnum, job.ep.address);
                                        UsbIpResponse::usbip_ret_submit_err(&job.header, errno)
                                    }
                                }
                            };
                            if rtx.send(res).await.is_err() {
                                break;
                            }
                        }
                    });
                    guard.worker_handles.push(jh);
                    ep_workers.insert(real_ep, wtx.clone());
                    wtx
                };

                let seqnum = header.seqnum;
                let cancelled = Arc::new(AtomicBool::new(false));
                if let Ok(mut map) = pending_unlinks.lock() {
                    map.insert(seqnum, cancelled.clone());
                }
                in_flight.fetch_add(1, Ordering::AcqRel);

                let job = UrbJob {
                    device: device.clone(),
                    ep,
                    intf: intf.cloned(),
                    header,
                    transfer_buffer_length,
                    setup: SetupPacket::parse(&setup),
                    data,
                    out,
                    cancelled,
                };
                if ep_tx.send(job).await.is_err() {
                    if let Ok(mut map) = pending_unlinks.lock() {
                        map.remove(&seqnum);
                    }
                    if in_flight.fetch_sub(1, Ordering::AcqRel) == 1 {
                        zero_notify.notify_waiters();
                    }
                    break Err(std::io::Error::new(
                        ErrorKind::BrokenPipe,
                        "endpoint worker closed",
                    ));
                }
            }
            UsbIpCommand::UsbIpCmdUnlink {
                mut header,
                unlink_seqnum,
            } => {
                trace!("Got USBIP_CMD_UNLINK for {unlink_seqnum:10x?}");
                if let Ok(map) = pending_unlinks.lock() {
                    if let Some(flag) = map.get(&unlink_seqnum) {
                        flag.store(true, Ordering::Release);
                    }
                }
                header.command = USBIP_RET_UNLINK.into();
                let res = UsbIpResponse::usbip_ret_unlink_success(&header);
                if tx.send(res).await.is_err() {
                    break Err(std::io::Error::new(
                        ErrorKind::BrokenPipe,
                        "response writer closed",
                    ));
                }
                trace!("Queued USBIP_RET_UNLINK");
            }
        }
    };

    let ret = match read_res {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == ErrorKind::UnexpectedEof => {
            info!("Remote closed the connection");
            if in_flight.load(Ordering::Acquire) > 0 {
                let _ = tokio::time::timeout(Duration::from_millis(50), async {
                    while in_flight.load(Ordering::Acquire) > 0 {
                        zero_notify.notified().await;
                    }
                })
                .await;
            }
            ep_workers.clear();
            resp_tx_opt.take();
            let _ = tokio::time::timeout(Duration::from_millis(50), &mut write_fut).await;
            Ok(())
        }
        Err(err) => {
            ep_workers.clear();
            resp_tx_opt.take();
            Err(err)
        }
    };

    for h in guard.worker_handles.drain(..) {
        h.abort();
    }
    release_imported_device(&server, &imported_id).await;
    guard.cleaned = true;
    ret
}

/// Spawn a USB/IP server at `addr` using [TcpListener]
pub async fn server(addr: SocketAddr, server: Arc<UsbIpServer>) {
    let listener = TcpListener::bind(addr).await.expect("bind to addr");

    let server = async move {
        loop {
            match listener.accept().await {
                Ok((mut socket, _addr)) => {
                    info!("Got connection from {:?}", socket.peer_addr());
                    let new_server = server.clone();
                    tokio::spawn(async move {
                        let res = handler(&mut socket, new_server).await;
                        info!("Handler ended with {res:?}");
                    });
                }
                Err(err) => {
                    warn!("Got error {err:?}");
                }
            }
        }
    };

    server.await
}

#[cfg(test)]
mod tests {
    use tokio::{net::TcpStream, task::JoinSet};

    use super::*;
    use crate::{
        usbip_protocol::{USBIP_CMD_SUBMIT, UsbIpHeaderBasic},
        util::tests::*,
    };

    const SINGLE_DEVICE_BUSID: &str = "0-0-0";

    fn new_server_with_single_device() -> UsbIpServer {
        UsbIpServer::new_simulated(vec![UsbDevice::new(0).with_interface(
            ClassCode::CDC as u8,
            cdc::CDC_ACM_SUBCLASS,
            0x00,
            Some("Test CDC ACM"),
            cdc::UsbCdcAcmHandler::endpoints(),
            Arc::new(Mutex::new(
                Box::new(cdc::UsbCdcAcmHandler::new()) as Box<dyn UsbInterfaceHandler + Send>
            )),
        )])
    }

    fn op_req_import(busid: &str) -> Vec<u8> {
        let mut busid = busid.to_string().as_bytes().to_vec();
        busid.resize(32, 0);
        UsbIpCommand::OpReqImport {
            status: 0,
            busid: busid.try_into().unwrap(),
        }
        .to_bytes()
    }

    async fn attach_device(connection: &mut TcpStream, busid: &str) -> u32 {
        let req = op_req_import(busid);
        connection.write_all(req.as_slice()).await.unwrap();
        connection.read_u32().await.unwrap();
        let result = connection.read_u32().await.unwrap();
        if result == 0 {
            connection.read_exact(&mut vec![0; 0x138]).await.unwrap();
        }
        result
    }

    #[tokio::test]
    async fn req_empty_devlist() {
        setup_test_logger();
        let server = UsbIpServer::new_simulated(vec![]);
        let req = UsbIpCommand::OpReqDevlist { status: 0 };

        let mut mock_socket = MockSocket::new(req.to_bytes());
        handler(&mut mock_socket, Arc::new(server)).await.ok();

        assert_eq!(
            mock_socket.output,
            UsbIpResponse::op_rep_devlist(&[]).to_bytes(),
        );
    }

    #[tokio::test]
    async fn req_sample_devlist() {
        setup_test_logger();
        let server = new_server_with_single_device();
        let req = UsbIpCommand::OpReqDevlist { status: 0 };

        let mut mock_socket = MockSocket::new(req.to_bytes());
        handler(&mut mock_socket, Arc::new(server)).await.ok();

        // OP_REP_DEVLIST
        // header: 0xC
        // device: 0x138
        // interface: 4 * 0x1
        assert_eq!(mock_socket.output.len(), 0xC + 0x138 + 4);
    }

    #[tokio::test]
    async fn req_import() {
        setup_test_logger();
        let server = new_server_with_single_device();

        // OP_REQ_IMPORT
        let req = op_req_import(SINGLE_DEVICE_BUSID);
        let mut mock_socket = MockSocket::new(req);
        handler(&mut mock_socket, Arc::new(server)).await.ok();
        // OP_REQ_IMPORT
        assert_eq!(mock_socket.output.len(), 0x140);
    }

    #[tokio::test]
    async fn add_and_remove_10_devices() {
        setup_test_logger();
        let server_ = Arc::new(UsbIpServer::new_simulated(vec![]));
        let addr = get_free_address().await;
        tokio::spawn(server(addr, server_.clone()));

        let mut join_set = JoinSet::new();
        let devices = (0..10).map(UsbDevice::new).collect::<Vec<_>>();

        for device in devices.iter() {
            let new_server = server_.clone();
            let new_device = device.clone();
            join_set.spawn(async move {
                new_server.add_device(new_device).await;
            });
        }

        for device in devices.iter() {
            let new_server = server_.clone();
            let new_device = device.clone();
            join_set.spawn(async move {
                new_server.remove_device(&new_device.bus_id).await.unwrap();
            });
        }

        while join_set.join_next().await.is_some() {}

        let device_len = server_.clone().available_devices.read().await.len();

        assert_eq!(device_len, 0);
    }

    #[tokio::test]
    async fn send_usb_traffic_while_adding_and_removing_devices() {
        setup_test_logger();
        let server_ = Arc::new(new_server_with_single_device());

        let addr = get_free_address().await;
        tokio::spawn(server(addr, server_.clone()));

        let cmd_loop_handle = tokio::spawn(async move {
            let mut connection = poll_connect(addr).await;
            let result = attach_device(&mut connection, SINGLE_DEVICE_BUSID).await;
            assert_eq!(result, 0);

            let cdc_loopback_bulk_cmd = UsbIpCommand::UsbIpCmdSubmit {
                header: usbip_protocol::UsbIpHeaderBasic {
                    command: USBIP_CMD_SUBMIT.into(),
                    seqnum: 1,
                    devid: 0,
                    direction: 0, // OUT
                    ep: 2,
                },
                transfer_flags: 0,
                transfer_buffer_length: 8,
                start_frame: 0,
                number_of_packets: 0,
                interval: 0,
                setup: [0; 8],
                data: vec![1, 2, 3, 4, 5, 6, 7, 8],
                iso_packet_descriptor: vec![],
            };

            loop {
                connection
                    .write_all(cdc_loopback_bulk_cmd.to_bytes().as_slice())
                    .await
                    .unwrap();
                let mut result = vec![0; 4 * 12];
                connection.read_exact(&mut result).await.unwrap();
            }
        });

        let add_and_remove_device_handle = tokio::spawn(async move {
            let mut join_set = JoinSet::new();
            let devices = (1..4).map(UsbDevice::new).collect::<Vec<_>>();

            loop {
                for device in devices.iter() {
                    let new_server = server_.clone();
                    let new_device = device.clone();
                    join_set.spawn(async move {
                        new_server.add_device(new_device).await;
                    });
                }

                for device in devices.iter() {
                    let new_server = server_.clone();
                    let new_device = device.clone();
                    join_set.spawn(async move {
                        new_server.remove_device(&new_device.bus_id).await.unwrap();
                    });
                }
                while join_set.join_next().await.is_some() {}
                tokio::time::sleep(tokio::time::Duration::from_millis(20)).await;
            }
        });

        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
        cmd_loop_handle.abort();
        add_and_remove_device_handle.abort();
    }

    #[tokio::test]
    async fn only_single_connection_allowed_to_device() {
        setup_test_logger();
        let server_ = Arc::new(new_server_with_single_device());

        let addr = get_free_address().await;
        tokio::spawn(server(addr, server_.clone()));

        let mut first_connection = poll_connect(addr).await;
        let mut second_connection = TcpStream::connect(addr).await.unwrap();

        let result = attach_device(&mut first_connection, SINGLE_DEVICE_BUSID).await;
        assert_eq!(result, 0);

        let result = attach_device(&mut second_connection, SINGLE_DEVICE_BUSID).await;
        assert_eq!(result, 1);
    }

    #[tokio::test]
    async fn device_gets_released_on_closed_socket() {
        setup_test_logger();
        let server_ = Arc::new(new_server_with_single_device());

        let addr = get_free_address().await;
        tokio::spawn(server(addr, server_.clone()));

        let mut connection = poll_connect(addr).await;
        let result = attach_device(&mut connection, SINGLE_DEVICE_BUSID).await;
        assert_eq!(result, 0);

        std::mem::drop(connection);

        let mut connection = TcpStream::connect(addr).await.unwrap();
        let result = attach_device(&mut connection, SINGLE_DEVICE_BUSID).await;
        assert_eq!(result, 0);
    }

    #[tokio::test]
    async fn req_import_get_device_desc() {
        setup_test_logger();
        let server = new_server_with_single_device();

        let mut req = op_req_import(SINGLE_DEVICE_BUSID);
        req.extend(
            UsbIpCommand::UsbIpCmdSubmit {
                header: UsbIpHeaderBasic {
                    command: USBIP_CMD_SUBMIT.into(),
                    seqnum: 1,
                    devid: 0,
                    direction: 1, // IN
                    ep: 0,
                },
                transfer_flags: 0,
                transfer_buffer_length: 0,
                start_frame: 0,
                number_of_packets: 0,
                interval: 0,
                // GetDescriptor to Device
                setup: [0x80, 0x06, 0x00, 0x01, 0x00, 0x00, 0x40, 0x00],
                data: vec![],
                iso_packet_descriptor: vec![],
            }
            .to_bytes(),
        );

        let mut mock_socket = MockSocket::new(req);
        handler(&mut mock_socket, Arc::new(server)).await.ok();
        // OP_REQ_IMPORT + USBIP_CMD_SUBMIT + Device Descriptor
        assert_eq!(mock_socket.output.len(), 0x140 + 0x30 + 0x12);
    }
}
