use portico_core::daemon::PorticoDaemon;
use portico_jni::{attach_usb_fd, init_fd_usb_context};
use std::os::fd::AsRawFd;
use std::sync::{Arc, Mutex};
use tempfile::TempDir;
use usbip::cdc::{UsbCdcAcmHandler, CDC_ACM_SUBCLASS};
use usbip::{ClassCode, UsbDevice};

#[tokio::test]
async fn fd_usb_context_initializes_and_invalid_or_mock_pipe_fd_returns_error_without_panic() {
    let ctx = init_fd_usb_context().expect("init_fd_usb_context should succeed");

    let dir = TempDir::new().unwrap();
    let daemon = PorticoDaemon::start_with_bind_overrides(
        dir.path().to_path_buf(),
        dir.path().join("portico-ctrl.sock"),
        Some("127.0.0.1:0".parse().unwrap()),
        Some("127.0.0.1:0".parse().unwrap()),
    )
    .await
    .unwrap();

    // 1. Negative fd rejected
    let res = attach_usb_fd(
        &daemon.handle(),
        &ctx,
        -1,
        "1-99",
        1,
        99,
        Some("InvalidFD"),
    )
    .await;
    assert!(res.is_err());

    // 2. Non-usbfs fd (mock pipe fd) fails cleanly without panicking or leaving stale entry
    let tmp_file = tempfile::tempfile().unwrap();
    let res2 = attach_usb_fd(
        &daemon.handle(),
        &ctx,
        tmp_file.as_raw_fd(),
        "1-98",
        1,
        98,
        Some("MockNonUsbFd"),
    )
    .await;
    assert!(res2.is_err());
    assert!(daemon
        .handle()
        .state
        .registry
        .list_devices()
        .await
        .is_empty());

    daemon.shutdown().await;
}

#[tokio::test]
async fn jni_daemon_claim_and_release_cycle_recovers_device_to_shared() {
    let dir = TempDir::new().unwrap();
    let daemon = PorticoDaemon::start_with_bind_overrides(
        dir.path().to_path_buf(),
        dir.path().join("portico-ctrl.sock"),
        Some("127.0.0.1:0".parse().unwrap()),
        Some("127.0.0.1:0".parse().unwrap()),
    )
    .await
    .unwrap();
    let h = daemon.handle();

    let mut d = UsbDevice::new(1).with_interface(
        ClassCode::CDC as u8,
        CDC_ACM_SUBCLASS,
        0,
        Some("MockUsb"),
        UsbCdcAcmHandler::endpoints(),
        Arc::new(Mutex::new(Box::new(UsbCdcAcmHandler::new()))),
    );
    d.bus_id = "1-1".into();
    d.vendor_id = 0x0781;
    d.product_id = 0x5595;
    h.state
        .registry
        .register_usb_device(d, "SanDisk".into(), "Pendrive".into(), "S1".into(), &[])
        .await
        .unwrap();

    let claim = h.state.registry.try_claim_device("1-1", "peer-1").await.unwrap();
    assert!(h.exec_command("LIST_DEVICES").await.contains("state=in_use"));
    assert_eq!(h.exec_command("RELEASE_DEVICE 1-1").await, "OK");
    drop(claim);
    assert!(h.exec_command("LIST_DEVICES").await.contains("state=shared"));

    let _claim2 = h.state.registry.try_claim_device("1-1", "peer-2").await.unwrap();
    daemon.shutdown().await;
}
