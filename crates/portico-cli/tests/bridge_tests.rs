use portico_cli::bridge::{run_usbip_attach_command, start_local_usbip_bridge};
use portico_cli::transport::{connect_and_auth, ConnectOptions};
use portico_core::daemon::PorticoDaemon;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use usbip::cdc::{UsbCdcAcmHandler, CDC_ACM_SUBCLASS};
use usbip::usbip_protocol::{UsbIpCommand, UsbIpHeaderBasic, USBIP_CMD_SUBMIT};
use usbip::{ClassCode, UsbDevice};

fn make_sim_cdc(bus_id: &str) -> UsbDevice {
    let mut d = UsbDevice::new(1).with_interface(
        ClassCode::CDC as u8,
        CDC_ACM_SUBCLASS,
        0,
        Some("CDC"),
        UsbCdcAcmHandler::endpoints(),
        Arc::new(Mutex::new(Box::new(UsbCdcAcmHandler::new()))),
    );
    d.bus_id = bus_id.to_string();
    d.vendor_id = 0x1234;
    d.product_id = 0x5678;
    d
}

#[tokio::test]
async fn bridge_tunnels_usbip_over_tls_and_fails_on_missing_bus_or_failing_usbip_bin() {
    let daemon_dir = TempDir::new().unwrap();
    let cli_dir = TempDir::new().unwrap();
    let sock = daemon_dir.path().join("portico-ctrl.sock");

    let daemon = PorticoDaemon::start_with_bind_overrides(
        daemon_dir.path().to_path_buf(),
        sock,
        Some(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0)),
        Some(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0)),
    )
    .await
    .unwrap();
    let h = daemon.handle();
    h.state
        .registry
        .register_usb_device(
            make_sim_cdc("1-1"),
            "Acme".into(),
            "Widget".into(),
            "S1".into(),
            &[],
        )
        .await
        .unwrap();
    assert_eq!(h.exec_command("START_SERVER").await, "OK");

    let tcp_addr = h.network.bound_tcp_addr().await.unwrap().to_string();
    let token = h
        .exec_command("GET_TOKEN")
        .await
        .strip_prefix("OK ")
        .unwrap()
        .to_string();
    let fp = h.state.identity.read().await.fingerprint_hex();

    // Missing bus_id fails before opening bridge
    let s_missing = connect_and_auth(ConnectOptions {
        config_dir: cli_dir.path(),
        target: &tcp_addr,
        token: &token,
        trust_fp: Some(&fp),
        insecure_lan_plaintext: false,
        client_name: "cli",
    })
    .await
    .unwrap();
    assert!(start_local_usbip_bridge(s_missing, "9-9").await.is_err());

    // Valid bus_id opens local loopback bridge and passes USB/IP PDUs
    let s_ok = connect_and_auth(ConnectOptions {
        config_dir: cli_dir.path(),
        target: &tcp_addr,
        token: &token,
        trust_fp: Some(&fp),
        insecure_lan_plaintext: false,
        client_name: "cli",
    })
    .await
    .unwrap();
    let (local_addr, bridge_task) = start_local_usbip_bridge(s_ok, "1-1").await.unwrap();

    let mut local_usbip = TcpStream::connect(local_addr).await.unwrap();
    let mut busid = b"1-1".to_vec();
    busid.resize(32, 0);
    local_usbip
        .write_all(
            &UsbIpCommand::OpReqImport {
                status: 0,
                busid: busid.try_into().unwrap(),
            }
            .to_bytes(),
        )
        .await
        .unwrap();
    let _ver = local_usbip.read_u32().await.unwrap();
    assert_eq!(local_usbip.read_u32().await.unwrap(), 0);
    let mut info = vec![0u8; 0x138];
    local_usbip.read_exact(&mut info).await.unwrap();

    let submit = UsbIpCommand::UsbIpCmdSubmit {
        header: UsbIpHeaderBasic {
            command: USBIP_CMD_SUBMIT.into(),
            seqnum: 1,
            devid: 0,
            direction: 1,
            ep: 0,
        },
        transfer_flags: 0,
        transfer_buffer_length: 18,
        start_frame: 0,
        number_of_packets: 0,
        interval: 0,
        setup: [0x80, 0x06, 0x00, 0x01, 0x00, 0x00, 0x12, 0x00],
        data: vec![],
        iso_packet_descriptor: vec![],
    }
    .to_bytes();
    local_usbip.write_all(&submit).await.unwrap();
    let mut hdr = [0u8; 48];
    local_usbip.read_exact(&mut hdr).await.unwrap();
    let mut desc = [0u8; 18];
    local_usbip.read_exact(&mut desc).await.unwrap();
    assert_eq!(u16::from_le_bytes([desc[8], desc[9]]), 0x1234);

    drop(local_usbip);
    let _ = bridge_task.await;

    // Non-zero exit from usbip binary returns clean error
    assert!(run_usbip_attach_command("/bin/false", 12345, "1-1")
        .await
        .is_err());

    daemon.shutdown().await;
}
