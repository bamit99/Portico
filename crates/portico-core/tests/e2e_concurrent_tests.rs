use portico_core::daemon::PorticoDaemon;
use portico_proto::frame::{
    read_frame, write_frame, AuthKind, Frame, HelloStatus, TunnelStatus, CAP_USBIP_V1,
};
use portico_proto::tls::{build_client_tls_config, CertCheckPolicy};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tempfile::TempDir;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpStream, UnixStream};
use tokio_rustls::TlsConnector;
use usbip::cdc::{UsbCdcAcmHandler, CDC_ACM_SUBCLASS};
use usbip::usbip_protocol::{UsbIpCommand, UsbIpHeaderBasic, USBIP_CMD_SUBMIT};
use usbip::{ClassCode, UsbDevice};

fn make_sim_cdc(bus_id: &str, vid: u16, pid: u16) -> UsbDevice {
    let mut d = UsbDevice::new(1).with_interface(
        ClassCode::CDC as u8,
        CDC_ACM_SUBCLASS,
        0,
        Some("CDC"),
        UsbCdcAcmHandler::endpoints(),
        Arc::new(Mutex::new(Box::new(UsbCdcAcmHandler::new()))),
    );
    d.bus_id = bus_id.to_string();
    d.vendor_id = vid;
    d.product_id = pid;
    d
}

async fn connect_and_open_tunnel(
    addr: SocketAddr,
    fp: &str,
    token: &str,
    bus_id: &str,
) -> tokio_rustls::client::TlsStream<TcpStream> {
    let (cfg, _) = build_client_tls_config(
        None,
        CertCheckPolicy::PinnedOrFirstUse {
            expected_fp: Some(fp.to_string()),
            trust_first_use: false,
        },
    )
    .unwrap();
    let tcp = TcpStream::connect(addr).await.unwrap();
    let mut tls = TlsConnector::from(cfg)
        .connect("localhost".try_into().unwrap(), tcp)
        .await
        .unwrap();
    write_frame(
        &mut tls,
        &Frame::ClientHello {
            min_ver: 0,
            max_ver: 0,
            auth_kind: AuthKind::ClaimToken,
            credentials: token.as_bytes().to_vec(),
            client_name: format!("cli-{bus_id}"),
            capabilities: CAP_USBIP_V1,
        },
    )
    .await
    .unwrap();
    assert!(matches!(
        read_frame(&mut tls).await.unwrap(),
        Frame::ServerHello {
            status: HelloStatus::Ok,
            ..
        }
    ));
    write_frame(
        &mut tls,
        &Frame::OpenTunnelReq {
            bus_id: bus_id.to_string(),
        },
    )
    .await
    .unwrap();
    assert!(matches!(
        read_frame(&mut tls).await.unwrap(),
        Frame::OpenTunnelResp {
            status: TunnelStatus::Ok,
            ..
        }
    ));
    tls
}

async fn usbip_import_and_get_dev_desc(
    tls: &mut tokio_rustls::client::TlsStream<TcpStream>,
    bus_id: &str,
) -> [u8; 18] {
    let mut busid = bus_id.as_bytes().to_vec();
    busid.resize(32, 0);
    let import = UsbIpCommand::OpReqImport {
        status: 0,
        busid: busid.try_into().unwrap(),
    }
    .to_bytes();
    tls.write_all(&import).await.unwrap();
    let _ver = tls.read_u32().await.unwrap();
    assert_eq!(tls.read_u32().await.unwrap(), 0);
    let mut info = vec![0u8; 0x138];
    tls.read_exact(&mut info).await.unwrap();

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
    tls.write_all(&submit).await.unwrap();
    let mut ret_hdr = [0u8; 48];
    tls.read_exact(&mut ret_hdr).await.unwrap();
    let mut desc = [0u8; 18];
    tls.read_exact(&mut desc).await.unwrap();
    desc
}

#[tokio::test]
async fn concurrent_two_device_usbip_sessions_and_detach_event() {
    let dir = TempDir::new().unwrap();
    let sock = dir.path().join("portico-ctrl.sock");
    let daemon = PorticoDaemon::start_with_bind_overrides(
        dir.path().to_path_buf(),
        sock.clone(),
        Some(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0)),
        Some(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0)),
    )
    .await
    .unwrap();
    let h = daemon.handle();

    h.state
        .registry
        .register_usb_device(
            make_sim_cdc("1-1", 0x1111, 0x2222),
            "M1".into(),
            "Dev1".into(),
            "S1".into(),
            &[],
        )
        .await
        .unwrap();
    h.state
        .registry
        .register_usb_device(
            make_sim_cdc("1-2", 0x3333, 0x4444),
            "M2".into(),
            "Dev2".into(),
            "S2".into(),
            &[],
        )
        .await
        .unwrap();

    assert_eq!(h.exec_command("START_SERVER").await, "OK");
    let tcp_addr = h.network.bound_tcp_addr().await.unwrap();
    let token = h
        .exec_command("GET_TOKEN")
        .await
        .strip_prefix("OK ")
        .unwrap()
        .to_string();
    let fp = h.state.identity.read().await.fingerprint_hex();

    // Subscribe to events
    let sub = UnixStream::connect(&sock).await.unwrap();
    let (sr, mut sw) = sub.into_split();
    let mut sr = BufReader::new(sr);
    sw.write_all(b"SUBSCRIBE\n").await.unwrap();
    sw.flush().await.unwrap();
    let mut ok = String::new();
    sr.read_line(&mut ok).await.unwrap();
    assert_eq!(ok.trim(), "OK");

    // Two concurrent TLS + USB/IP sessions
    let (mut t1, mut t2) = tokio::join!(
        connect_and_open_tunnel(tcp_addr, &fp, &token, "1-1"),
        connect_and_open_tunnel(tcp_addr, &fp, &token, "1-2"),
    );

    let (d1, d2) = tokio::join!(
        usbip_import_and_get_dev_desc(&mut t1, "1-1"),
        usbip_import_and_get_dev_desc(&mut t2, "1-2"),
    );
    assert_eq!(u16::from_le_bytes([d1[8], d1[9]]), 0x1111);
    assert_eq!(u16::from_le_bytes([d1[10], d1[11]]), 0x2222);
    assert_eq!(u16::from_le_bytes([d2[8], d2[9]]), 0x3333);
    assert_eq!(u16::from_le_bytes([d2[10], d2[11]]), 0x4444);

    drop(t1);
    drop(t2);
    tokio::time::sleep(Duration::from_millis(50)).await;

    assert_eq!(h.exec_command("DETACH_DEVICE 1-1").await, "OK");
    let mut saw_removed = false;
    for _ in 0..20 {
        let mut line = String::new();
        if tokio::time::timeout(Duration::from_millis(200), sr.read_line(&mut line))
            .await
            .is_ok()
        {
            if line.trim() == "EVENT DEVICE_REMOVED bus_id=1-1" {
                saw_removed = true;
                break;
            }
        }
    }
    assert!(saw_removed);

    daemon.shutdown().await;
}
