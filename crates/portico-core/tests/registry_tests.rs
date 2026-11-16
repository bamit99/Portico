use portico_core::config::VidPid;
use portico_core::registry::{DeviceRegistry, DeviceState};
use portico_core::usb_handler::{validate_in_transfer_len, MAX_USB_TRANSFER_LEN};
use portico_core::CoreError;
use portico_proto::frame::TunnelStatus;
use std::sync::{Arc, Mutex};
use usbip::cdc::{UsbCdcAcmHandler, CDC_ACM_SUBCLASS};
use usbip::usbip_protocol::UsbIpCommand;
use usbip::{ClassCode, UsbDevice};

fn make_sim_cdc(bus_id: &str, vid: u16, pid: u16) -> UsbDevice {
    let mut d = UsbDevice::new(1).with_interface(
        ClassCode::CDC as u8,
        CDC_ACM_SUBCLASS,
        0x00,
        Some("CDC"),
        UsbCdcAcmHandler::endpoints(),
        Arc::new(Mutex::new(Box::new(UsbCdcAcmHandler::new()))),
    );
    d.bus_id = bus_id.to_string();
    d.vendor_id = vid;
    d.product_id = pid;
    d
}

#[tokio::test]
async fn ignored_devices_never_auto_shared_and_toggle_updates_usbip_server() {
    let reg = DeviceRegistry::new();
    let ignored = vec![VidPid {
        vid: 0x0123,
        pid: 0xAE3C,
    }];

    let d1 = make_sim_cdc("1-1", 0x0123, 0xAE3C);
    let r1 = reg
        .register_usb_device(d1, "Acme".into(), "SecretKey".into(), "S1".into(), &ignored)
        .await
        .unwrap();
    assert_eq!(r1.state, DeviceState::Ignored);
    assert_eq!(
        reg.check_tunnel_eligible("1-1").await,
        Err(TunnelStatus::Ignored)
    );

    let d2 = make_sim_cdc("1-2", 0x0781, 0x5583);
    let r2 = reg
        .register_usb_device(d2, "SanDisk".into(), "Ultra".into(), "S2".into(), &ignored)
        .await
        .unwrap();
    assert_eq!(r2.state, DeviceState::Shared);
    assert_eq!(reg.list_shareable_summaries().await.len(), 1);
    assert_eq!(reg.check_tunnel_eligible("1-2").await, Ok(()));

    // Un-ignore 0123/AE3C and ignore 0781/5583
    let new_ignored = vec![VidPid {
        vid: 0x0781,
        pid: 0x5583,
    }];
    reg.apply_ignored_list(&new_ignored).await.unwrap();

    let devs = reg.list_devices().await;
    let st1 = &devs.iter().find(|d| d.bus_id == "1-1").unwrap().state;
    let st2 = &devs.iter().find(|d| d.bus_id == "1-2").unwrap().state;
    assert_eq!(*st1, DeviceState::Shared);
    assert_eq!(*st2, DeviceState::Ignored);

    // Verify UsbIpServer rejects importing ignored 1-2
    let mut busid_buf = b"1-2".to_vec();
    busid_buf.resize(32, 0);
    let import_ignored = UsbIpCommand::OpReqImport {
        status: 0,
        busid: busid_buf.try_into().unwrap(),
    }
    .to_bytes();
    let (mut client_end, mut server_end) = tokio::io::duplex(4096);
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let srv = reg.usbip_server();
    let h = tokio::spawn(async move { usbip::handler(&mut server_end, srv).await });
    client_end.write_all(&import_ignored).await.unwrap();
    let _cmd = client_end.read_u32().await.unwrap();
    let status = client_end.read_u32().await.unwrap();
    assert_ne!(status, 0);
    drop(client_end);
    let _ = h.await;
    // Actually test detach_device error on missing
    assert!(matches!(
        reg.detach_device("9-9").await,
        Err(CoreError::NotFound(_))
    ));

    // Concurrent claim exclusion + cross-device isolation + detach while InUse
    let d3 = make_sim_cdc("1-3", 0xAAAA, 0xBBBB);
    reg.register_usb_device(d3, "M".into(), "P".into(), "S3".into(), &[])
        .await
        .unwrap();
    let claim1 = reg.try_claim_device("1-3", "peerA").await.unwrap();
    assert_eq!(
        reg.try_claim_device("1-3", "peerB").await.err(),
        Some(TunnelStatus::InUse)
    );
    // Detach while InUse triggers cancel token, waits for tunnel drop, and removes device
    let notified = claim1.cancel.clone();
    let done_tx = claim1.done_tx.clone();
    let waiter = tokio::spawn(async move {
        notified.notified().await;
        let _ = done_tx.send(true);
    });
    tokio::task::yield_now().await;
    reg.detach_device("1-3").await.unwrap();
    waiter.await.unwrap();
    assert!(reg.list_devices().await.iter().all(|d| d.bus_id != "1-3"));

    // cancel_claim releases InUse device back to Shared without removing it
    let d4 = make_sim_cdc("1-4", 0xCCCC, 0xDDDD);
    reg.register_usb_device(d4, "M".into(), "P4".into(), "S4".into(), &[])
        .await
        .unwrap();
    let claim4 = reg.try_claim_device("1-4", "peerC").await.unwrap();
    let c4_notified = claim4.cancel.clone();
    let c4_done = claim4.done_tx.clone();
    let w4 = tokio::spawn(async move {
        c4_notified.notified().await;
        let _ = c4_done.send(true);
    });
    tokio::task::yield_now().await;
    reg.cancel_claim("1-4", &[]).await.unwrap();
    w4.await.unwrap();
    assert_eq!(
        reg.list_devices()
            .await
            .into_iter()
            .find(|d| d.bus_id == "1-4")
            .unwrap()
            .state,
        DeviceState::Shared
    );

    assert_eq!(validate_in_transfer_len(1024).unwrap(), 1024);
    assert!(validate_in_transfer_len(MAX_USB_TRANSFER_LEN + 1).is_err());
    assert!(validate_in_transfer_len(u32::MAX).is_err());
}
