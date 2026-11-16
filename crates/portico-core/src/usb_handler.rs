use crate::config::UsbSpeedSetting;
use crate::CoreError;
use rusb::{DeviceHandle, Direction as RusbDir, Speed, UsbContext};
use std::any::Any;
use std::io::{Error as IoError, ErrorKind, Result as IoResult};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tracing::warn;
use usbip::{
    Direction, EndpointAttributes, SetupPacket, UsbDevice, UsbDeviceHandler, UsbEndpoint,
    UsbInterface, UsbInterfaceHandler, UsbSpeed,
};

pub const MAX_USB_TRANSFER_LEN: u32 = 16 * 1024 * 1024;
const BULK_FALLBACK_CHUNK: usize = 128 * 1024;

pub fn transfer_timeout(len: usize) -> Duration {
    let extra_secs = (len / (256 * 1024)) as u64;
    Duration::from_secs((10 + extra_secs).min(60))
}

pub fn validate_in_transfer_len(len: u32) -> IoResult<usize> {
    if len > MAX_USB_TRANSFER_LEN {
        return Err(IoError::new(
            ErrorKind::InvalidInput,
            format!("transfer_buffer_length {len} exceeds cap {MAX_USB_TRANSFER_LEN}"),
        ));
    }
    Ok(len as usize)
}

fn run_blocking_usb<F, R>(f: F) -> R
where
    F: FnOnce() -> R,
{
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        if handle.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread {
            return tokio::task::block_in_place(f);
        }
    }
    f()
}

struct InFlightCounter(Arc<AtomicUsize>);

impl InFlightCounter {
    fn enter(counter: &Arc<AtomicUsize>) -> Self {
        counter.fetch_add(1, Ordering::AcqRel);
        Self(counter.clone())
    }
}

impl Drop for InFlightCounter {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

#[derive(Clone)]
pub struct PorticoRusbInterfaceHandler<C: UsbContext + 'static> {
    handle: Arc<DeviceHandle<C>>,
    interface_number: u8,
    in_flight: Arc<AtomicUsize>,
}

impl<C: UsbContext + 'static> std::fmt::Debug for PorticoRusbInterfaceHandler<C> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PorticoRusbInterfaceHandler")
            .field("interface_number", &self.interface_number)
            .finish()
    }
}

impl<C: UsbContext + 'static> PorticoRusbInterfaceHandler<C> {
    pub fn new(handle: Arc<DeviceHandle<C>>) -> Self {
        Self::with_interface(handle, 0)
    }

    pub fn with_interface(handle: Arc<DeviceHandle<C>>, interface_number: u8) -> Self {
        Self {
            handle,
            interface_number,
            in_flight: Arc::new(AtomicUsize::new(0)),
        }
    }
}

fn map_rusb_error(e: rusb::Error) -> IoError {
    match e {
        rusb::Error::Pipe => IoError::new(ErrorKind::BrokenPipe, format!("usb pipe stall: {e}")),
        rusb::Error::Timeout => IoError::new(ErrorKind::TimedOut, format!("usb transfer timeout: {e}")),
        rusb::Error::Overflow => IoError::new(ErrorKind::InvalidData, format!("usb overflow: {e}")),
        rusb::Error::NoDevice => IoError::new(ErrorKind::NotConnected, format!("usb device gone: {e}")),
        rusb::Error::InvalidParam => IoError::new(ErrorKind::InvalidInput, format!("usb invalid param: {e}")),
        rusb::Error::Interrupted => IoError::new(ErrorKind::ConnectionReset, format!("usb interrupted: {e}")),
        other => IoError::other(format!("usb transfer error: {other}")),
    }
}

fn map_rusb_in(res: rusb::Result<usize>, mut buf: Vec<u8>) -> IoResult<Vec<u8>> {
    match res {
        Ok(n) => {
            buf.truncate(n);
            Ok(buf)
        }
        Err(e) => Err(map_rusb_error(e)),
    }
}

fn map_rusb_interrupt_in(res: rusb::Result<usize>, mut buf: Vec<u8>) -> IoResult<Vec<u8>> {
    match res {
        Ok(n) => {
            buf.truncate(n);
            Ok(buf)
        }
        Err(rusb::Error::Timeout) => Ok(Vec::new()),
        Err(e) => Err(map_rusb_error(e)),
    }
}

fn map_rusb_out(res: rusb::Result<usize>, expected: usize) -> IoResult<Vec<u8>> {
    match res {
        Ok(n) if n >= expected || expected == 0 => Ok(Vec::new()),
        Ok(n) => Err(IoError::other(format!(
            "usb short out transfer: wrote {n} of {expected} bytes"
        ))),
        Err(e) => Err(map_rusb_error(e)),
    }
}

fn read_bulk_resilient<C: UsbContext>(
    handle: &DeviceHandle<C>,
    ep: u8,
    buf: &mut [u8],
    timeout: Duration,
) -> rusb::Result<usize> {
    match handle.read_bulk(ep, buf, timeout) {
        Err(rusb::Error::NoMem) if buf.len() > BULK_FALLBACK_CHUNK => {
            let mut total = 0usize;
            while total < buf.len() {
                let end = (total + BULK_FALLBACK_CHUNK).min(buf.len());
                let n = handle.read_bulk(ep, &mut buf[total..end], timeout)?;
                total += n;
                if n < end - total + n {
                    // n < chunk_len means short packet terminated the USB transfer
                    break;
                }
            }
            Ok(total)
        }
        other => other,
    }
}

fn write_bulk_resilient<C: UsbContext>(
    handle: &DeviceHandle<C>,
    ep: u8,
    data: &[u8],
    timeout: Duration,
) -> rusb::Result<usize> {
    match handle.write_bulk(ep, data, timeout) {
        Err(rusb::Error::NoMem) if data.len() > BULK_FALLBACK_CHUNK => {
            let mut total = 0usize;
            while total < data.len() {
                let end = (total + BULK_FALLBACK_CHUNK).min(data.len());
                let n = handle.write_bulk(ep, &data[total..end], timeout)?;
                total += n;
                if n < end - total + n {
                    break;
                }
            }
            Ok(total)
        }
        other => other,
    }
}

impl<C: UsbContext + Send + Sync + 'static> UsbInterfaceHandler for PorticoRusbInterfaceHandler<C> {
    fn get_class_specific_descriptor(&self) -> Vec<u8> {
        Vec::new()
    }

    fn clone_box(&self) -> Option<Box<dyn UsbInterfaceHandler + Send>> {
        Some(Box::new(self.clone()))
    }

    fn on_tunnel_release(&self) {
        if self.in_flight.load(Ordering::Acquire) > 0 {
            let _ = self.handle.set_auto_detach_kernel_driver(false);
            let _ = self.handle.release_interface(self.interface_number);
            let _ = self.handle.claim_interface(self.interface_number);
            let _ = self.handle.set_auto_detach_kernel_driver(true);
        }
    }

    fn handle_urb(
        &mut self,
        _interface: &UsbInterface,
        ep: UsbEndpoint,
        transfer_buffer_length: u32,
        setup: SetupPacket,
        req: &[u8],
    ) -> IoResult<Vec<u8>> {
        let _inflight = InFlightCounter::enter(&self.in_flight);
        let handle = &self.handle;

        if ep.attributes == EndpointAttributes::Control as u8 {
            let timeout = transfer_timeout(transfer_buffer_length.max(req.len() as u32) as usize);
            if setup.request_type == 0x01 && setup.request == 0x0B {
                // SET_INTERFACE must go through libusb/usbfs ioctl so kernel endpoint tables update
                let res = run_blocking_usb(|| {
                    handle.set_alternate_setting(setup.index as u8, setup.value as u8)
                });
                return res.map(|_| Vec::new()).map_err(map_rusb_error);
            }
            if let Direction::In = ep.direction() {
                let len = validate_in_transfer_len(transfer_buffer_length)?;
                let mut buffer = vec![0u8; len];
                let res = run_blocking_usb(|| {
                    handle.read_control(
                        setup.request_type,
                        setup.request,
                        setup.value,
                        setup.index,
                        &mut buffer,
                        timeout,
                    )
                });
                map_rusb_in(res, buffer)
            } else {
                let res = run_blocking_usb(|| {
                    handle.write_control(
                        setup.request_type,
                        setup.request,
                        setup.value,
                        setup.index,
                        req,
                        timeout,
                    )
                });
                map_rusb_out(res, req.len())
            }
        } else if ep.attributes == EndpointAttributes::Interrupt as u8 {
            let timeout = Duration::from_secs(2);
            if let Direction::In = ep.direction() {
                let len = validate_in_transfer_len(transfer_buffer_length)?;
                let mut buffer = vec![0u8; len];
                let res = run_blocking_usb(|| handle.read_interrupt(ep.address, &mut buffer, timeout));
                map_rusb_interrupt_in(res, buffer)
            } else {
                let res = run_blocking_usb(|| handle.write_interrupt(ep.address, req, timeout));
                map_rusb_out(res, req.len())
            }
        } else if ep.attributes == EndpointAttributes::Bulk as u8 {
            if let Direction::In = ep.direction() {
                let len = validate_in_transfer_len(transfer_buffer_length)?;
                let timeout = transfer_timeout(len);
                let mut buffer = vec![0u8; len];
                let res = run_blocking_usb(|| read_bulk_resilient(handle, ep.address, &mut buffer, timeout));
                map_rusb_in(res, buffer)
            } else {
                let timeout = transfer_timeout(req.len());
                let res = run_blocking_usb(|| write_bulk_resilient(handle, ep.address, req, timeout));
                map_rusb_out(res, req.len())
            }
        } else {
            Ok(Vec::new())
        }
    }

    fn as_any(&mut self) -> &mut dyn Any {
        self
    }
}

#[derive(Clone)]
pub struct PorticoRusbDeviceHandler<C: UsbContext + 'static> {
    handle: Arc<DeviceHandle<C>>,
}

impl<C: UsbContext + 'static> std::fmt::Debug for PorticoRusbDeviceHandler<C> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PorticoRusbDeviceHandler").finish()
    }
}

impl<C: UsbContext + 'static> PorticoRusbDeviceHandler<C> {
    pub fn new(handle: Arc<DeviceHandle<C>>) -> Self {
        Self { handle }
    }
}

impl<C: UsbContext + Send + Sync + 'static> UsbDeviceHandler for PorticoRusbDeviceHandler<C> {
    fn clone_box(&self) -> Option<Box<dyn UsbDeviceHandler + Send>> {
        Some(Box::new(self.clone()))
    }

    fn handle_urb(
        &mut self,
        transfer_buffer_length: u32,
        setup: SetupPacket,
        req: &[u8],
    ) -> IoResult<Vec<u8>> {
        let timeout = transfer_timeout(transfer_buffer_length.max(req.len() as u32) as usize);
        let handle = &self.handle;

        if setup.request_type == 0x02 && setup.request == 0x01 && setup.value == 0 {
            // CLEAR_FEATURE(ENDPOINT_HALT) must use libusb clear_halt to reset host toggle & usbfs state
            let res = run_blocking_usb(|| handle.clear_halt(setup.index as u8));
            return res.map(|_| Vec::new()).map_err(map_rusb_error);
        }

        if setup.request_type & 0x80 == 0 {
            let res = run_blocking_usb(|| {
                handle.write_control(
                    setup.request_type,
                    setup.request,
                    setup.value,
                    setup.index,
                    req,
                    timeout,
                )
            });
            map_rusb_out(res, req.len())
        } else {
            let len = validate_in_transfer_len(transfer_buffer_length)?;
            let mut buffer = vec![0u8; len];
            let res = run_blocking_usb(|| {
                handle.read_control(
                    setup.request_type,
                    setup.request,
                    setup.value,
                    setup.index,
                    &mut buffer,
                    timeout,
                )
            });
            map_rusb_in(res, buffer)
        }
    }

    fn as_any(&mut self) -> &mut dyn Any {
        self
    }
}

fn map_speed(speed: Speed, override_setting: UsbSpeedSetting) -> UsbSpeed {
    match override_setting {
        UsbSpeedSetting::Low => UsbSpeed::Low,
        UsbSpeedSetting::Full => UsbSpeed::Full,
        UsbSpeedSetting::High => UsbSpeed::High,
        UsbSpeedSetting::Super => UsbSpeed::Super,
        UsbSpeedSetting::Auto => match speed {
            Speed::Low => UsbSpeed::Low,
            Speed::Full => UsbSpeed::Full,
            Speed::High => UsbSpeed::High,
            Speed::Super => UsbSpeed::Super,
            Speed::SuperPlus => UsbSpeed::SuperPlus,
            _ => UsbSpeed::Unknown,
        },
    }
}

pub fn usb_device_from_rusb_handle<C: UsbContext + Send + Sync + 'static>(
    open_device: DeviceHandle<C>,
    bus_id: &str,
    bus_num: u32,
    dev_num: u32,
    speed_override: UsbSpeedSetting,
    fallback_product: Option<&str>,
) -> Result<(UsbDevice, String, String, String), CoreError> {
    let dev = open_device.device();
    let desc = dev
        .device_descriptor()
        .map_err(|e| CoreError::Usb(format!("device descriptor: {e}")))?;
    let cfg = dev
        .active_config_descriptor()
        .map_err(|e| CoreError::Usb(format!("config descriptor: {e}")))?;

    let handle = Arc::new(open_device);

    let _ = handle.set_auto_detach_kernel_driver(true);
    for intf in cfg.interfaces() {
        if let Some(intf_desc) = intf.descriptors().next() {
            let _ = handle.claim_interface(intf_desc.interface_number());
        }
    }

    let mut device = UsbDevice::new(dev_num);
    device.path = format!("/sys/bus/usb/devices/{bus_id}");
    device.bus_id = bus_id.to_string();
    device.bus_num = bus_num;
    device.dev_num = dev_num;
    device.speed = map_speed(dev.speed(), speed_override) as u32;
    device.vendor_id = desc.vendor_id();
    device.product_id = desc.product_id();
    device.device_class = desc.class_code();
    device.device_subclass = desc.sub_class_code();
    device.device_protocol = desc.protocol_code();
    device.device_bcd = desc.device_version().into();
    device.usb_version = desc.usb_version().into();
    device.set_ep0_max_packet_size(desc.max_packet_size() as u16);
    device.configuration_value = cfg.number();
    device.num_configurations = desc.num_configurations();

    let mut manufacturer = String::new();
    let mut product = fallback_product.unwrap_or("USB Device").to_string();
    let mut serial = String::new();

    if let Some(idx) = desc.manufacturer_string_index() {
        match handle.read_string_descriptor_ascii(idx) {
            Ok(s) => {
                device.set_manufacturer_name(&s);
                manufacturer = s;
            }
            Err(e) => warn!("[{bus_id}] failed reading mfr string: {e}"),
        }
    } else {
        device.unset_manufacturer_name();
    }

    if let Some(idx) = desc.product_string_index() {
        match handle.read_string_descriptor_ascii(idx) {
            Ok(s) => {
                device.set_product_name(&s);
                product = s;
            }
            Err(e) => {
                warn!("[{bus_id}] failed reading product string: {e}");
                device.set_product_name(&product);
            }
        }
    } else {
        device.set_product_name(&product);
    }

    if let Some(idx) = desc.serial_number_string_index() {
        match handle.read_string_descriptor_ascii(idx) {
            Ok(s) => {
                device.set_serial_number(&s);
                serial = s;
            }
            Err(e) => warn!("[{bus_id}] failed reading serial string: {e}"),
        }
    } else {
        device.unset_serial_number();
    }

    for intf in cfg.interfaces() {
        let Some(intf_desc) = intf.descriptors().next() else {
            continue;
        };
        let mut endpoints = Vec::new();
        let mut endpoint_extras = Vec::new();
        for ep_desc in intf_desc.endpoint_descriptors() {
            let dir_bit = match ep_desc.direction() {
                RusbDir::In => 0x80,
                RusbDir::Out => 0x00,
            };
            endpoints.push(UsbEndpoint {
                address: ep_desc.address() | dir_bit,
                attributes: ep_desc.transfer_type() as u8,
                max_packet_size: ep_desc.max_packet_size(),
                interval: ep_desc.interval(),
            });
            endpoint_extras.push(ep_desc.extra().map(Vec::from).unwrap_or_default());
        }
        let intf_name = intf_desc
            .description_string_index()
            .and_then(|idx| handle.read_string_descriptor_ascii(idx).ok());
        let handler: Arc<std::sync::Mutex<Box<dyn UsbInterfaceHandler + Send>>> =
            Arc::new(std::sync::Mutex::new(Box::new(
                PorticoRusbInterfaceHandler::with_interface(
                    handle.clone(),
                    intf_desc.interface_number(),
                ),
            )));
        device = device.with_interface(
            intf_desc.class_code(),
            intf_desc.sub_class_code(),
            intf_desc.protocol_code(),
            intf_name.as_deref(),
            endpoints,
            handler,
        );
        if let Some(last) = device.interfaces.last_mut() {
            last.class_specific_descriptor = Vec::from(intf_desc.extra());
            last.endpoint_extras = endpoint_extras;
        }
    }

    device = device.with_device_handler(Arc::new(std::sync::Mutex::new(Box::new(
        PorticoRusbDeviceHandler::new(handle),
    ))));

    Ok((device, manufacturer, product, serial))
}
