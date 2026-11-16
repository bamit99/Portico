use portico_cli::reverse::ReverseListener;
use portico_cli::serve::{start_host_daemon, ServeOptions};
use portico_core::config::ReverseClientEntry;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use usbip::usbip_protocol::{UsbIpCommand, UsbIpHeaderBasic, USBIP_CMD_SUBMIT};

#[tokio::test]
async fn reverse_listener_mtls_and_concurrent_two_device_attach_via_serve() {
    let cli_dir = TempDir::new().unwrap();
    let srv_dir = TempDir::new().unwrap();

    let rev_id = ReverseListener::load_or_create_identity(cli_dir.path()).unwrap();
    assert_eq!(rev_id.fingerprint_hex().len(), 64);

    // Start host daemon with simulated CDC device, plus add a second simulated device
    let daemon = start_host_daemon(ServeOptions {
        data_dir: srv_dir.path().to_path_buf(),
        ctrl_socket: srv_dir.path().join("portico-ctrl.sock"),
        tcp_bind: Some(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0)),
        udp_bind: Some(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0)),
        scan_host: false,
        simulate_cdc: true,
    })
    .await
    .unwrap();
    let h = daemon.handle();

    // Add a second simulated device
    {
        let mut d2 = usbip::UsbDevice::new(2).with_interface(
            usbip::ClassCode::CDC as u8,
            usbip::cdc::CDC_ACM_SUBCLASS,
            0,
            Some("CDC2"),
            usbip::cdc::UsbCdcAcmHandler::endpoints(),
            std::sync::Arc::new(std::sync::Mutex::new(Box::new(
                usbip::cdc::UsbCdcAcmHandler::new(),
            ))),
        );
        d2.bus_id = "2-2".into();
        d2.vendor_id = 0x2222;
        d2.product_id = 0x3333;
        h.state
            .registry
            .register_usb_device(d2, "M2".into(), "Sim2".into(), "S2".into(), &[])
            .await
            .unwrap();
    }

    let server_fp = h.state.identity.read().await.fingerprint_hex();
    let token = h
        .exec_command("GET_TOKEN")
        .await
        .strip_prefix("OK ")
        .unwrap()
        .to_string();

    let listener = ReverseListener::bind(
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
        rev_id.clone(),
        vec![server_fp.clone()],
        token,
    )
    .await
    .unwrap();
    let rev_addr = listener.local_addr();

    // Configure daemon to dial listener
    {
        let mut cfg = h.state.config.lock().await;
        cfg.config.reverse_clients = vec![ReverseClientEntry {
            id: "desk".into(),
            addr: rev_addr.to_string(),
            cert_sha256: rev_id.fingerprint_hex(),
            enabled: true,
        }];
    }
    h.network.sync_reverse_clients(h.state.clone()).await;

    let ctrl = tokio::time::timeout(Duration::from_secs(4), listener.wait_for_server_ready())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(ctrl.server_fp, server_fp);
    assert_eq!(ctrl.devices.len(), 2);

    let (b1, b2) = tokio::join!(
        listener.open_reverse_tunnel(&server_fp, &ctrl.devices[0].bus_id),
        listener.open_reverse_tunnel(&server_fp, &ctrl.devices[1].bus_id),
    );
    let (addr1, t1) = b1.unwrap();
    let (addr2, t2) = b2.unwrap();

    // Exercise USB/IP import + GetDescriptor on BOTH reverse tunnels concurrently
    async fn fetch_vid(addr: SocketAddr, bus_id: &str) -> u16 {
        let mut s = TcpStream::connect(addr).await.unwrap();
        let mut bus = bus_id.as_bytes().to_vec();
        bus.resize(32, 0);
        s.write_all(
            &UsbIpCommand::OpReqImport {
                status: 0,
                busid: bus.try_into().unwrap(),
            }
            .to_bytes(),
        )
        .await
        .unwrap();
        let _v = s.read_u32().await.unwrap();
        assert_eq!(s.read_u32().await.unwrap(), 0);
        let mut info = vec![0u8; 0x138];
        s.read_exact(&mut info).await.unwrap();

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
        s.write_all(&submit).await.unwrap();
        let mut hdr = [0u8; 48];
        s.read_exact(&mut hdr).await.unwrap();
        let mut desc = [0u8; 18];
        s.read_exact(&mut desc).await.unwrap();
        u16::from_le_bytes([desc[8], desc[9]])
    }

    let (v1, v2) = tokio::join!(
        fetch_vid(addr1, &ctrl.devices[0].bus_id),
        fetch_vid(addr2, &ctrl.devices[1].bus_id),
    );
    assert_ne!(v1, v2);

    t1.abort();
    t2.abort();
    daemon.shutdown().await;
}
