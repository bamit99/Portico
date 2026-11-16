use crate::CliError;
use portico_core::config::UsbSpeedSetting;
use portico_core::daemon::PorticoDaemon;
use portico_core::usb_handler::usb_device_from_rusb_handle;
use rusb::UsbContext;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use usbip::cdc::{UsbCdcAcmHandler, CDC_ACM_SUBCLASS};
use usbip::{ClassCode, UsbDevice};

pub struct ServeOptions {
    pub data_dir: PathBuf,
    pub ctrl_socket: PathBuf,
    pub tcp_bind: Option<SocketAddr>,
    pub udp_bind: Option<SocketAddr>,
    pub scan_host: bool,
    pub simulate_cdc: bool,
}

pub async fn start_host_daemon(opts: ServeOptions) -> Result<PorticoDaemon, CliError> {
    let daemon = PorticoDaemon::start_with_bind_overrides(
        opts.data_dir,
        opts.ctrl_socket,
        opts.tcp_bind,
        opts.udp_bind,
    )
    .await?;

    let h = daemon.handle();
    let ignored = h.state.config.lock().await.config.ignored_devices.clone();

    if opts.simulate_cdc {
        let mut d = UsbDevice::new(1).with_interface(
            ClassCode::CDC as u8,
            CDC_ACM_SUBCLASS,
            0,
            Some("Simulated CDC ACM"),
            UsbCdcAcmHandler::endpoints(),
            Arc::new(Mutex::new(Box::new(UsbCdcAcmHandler::new()))),
        );
        d.bus_id = "1-1".into();
        d.vendor_id = 0x1D6B;
        d.product_id = 0x0104;
        h.state
            .registry
            .register_usb_device(
                d,
                "Portico".into(),
                "Simulated CDC ACM".into(),
                "SIM-001".into(),
                &ignored,
            )
            .await?;
    }

    if opts.scan_host {
        if let Ok(ctx) = rusb::Context::new() {
            if let Ok(list) = ctx.devices() {
                for dev in list.iter() {
                    if let Ok(desc) = dev.device_descriptor() {
                        if desc.class_code() == 0x09 {
                            continue; // skip USB hubs
                        }
                    }
                    let Ok(handle) = dev.open() else {
                        continue;
                    };
                    let bus_num = dev.bus_number() as u32;
                    let dev_num = dev.address() as u32;
                    let bus_id = format!("{bus_num}-{dev_num}");
                    if let Ok((usb_dev, mfr, prod, ser)) = usb_device_from_rusb_handle(
                        handle,
                        &bus_id,
                        bus_num,
                        dev_num,
                        UsbSpeedSetting::Auto,
                        None,
                    ) {
                        let _ = h
                            .state
                            .registry
                            .register_usb_device(usb_dev, mfr, prod, ser, &ignored)
                            .await;
                    }
                }
            }
        }
    }

    let start_res = h.exec_command("START_SERVER").await;
    if let Some(err) = start_res.strip_prefix("ERR ") {
        return Err(CliError::Remote(format!("failed to start server: {err}")));
    }

    Ok(daemon)
}
