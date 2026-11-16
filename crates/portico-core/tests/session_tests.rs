use portico_core::config::VidPid;
use portico_core::session::serve_portico_stream;
use portico_core::state::DaemonState;
use portico_proto::frame::{
    read_frame, write_frame, AuthKind, Frame, HelloStatus, TunnelStatus, CAP_USBIP_V1,
    PROTOCOL_VERSION_V0,
};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use usbip::cdc::{UsbCdcAcmHandler, CDC_ACM_SUBCLASS};
use usbip::usbip_protocol::{UsbIpCommand, USBIP_CMD_SUBMIT};
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
async fn valid_token_lists_and_upgrades_to_usbip_while_wrong_token_and_public_plaintext_fail() {
    let dir = TempDir::new().unwrap();
    let state = DaemonState::new(dir.path().to_path_buf()).unwrap();
    let token = state.config.lock().await.config.claim_token.clone();
    state.config.lock().await.config.plaintext_lan = true;
    state.config.lock().await.config.ignored_devices = vec![VidPid {
        vid: 0x0123,
        pid: 0xAE3C,
    }];

    let ignored = state.config.lock().await.config.ignored_devices.clone();
    state
        .registry
        .register_usb_device(
            make_sim_cdc("1-1", 0x0781, 0x5583),
            "SanDisk".into(),
            "Ultra".into(),
            "S1".into(),
            &ignored,
        )
        .await
        .unwrap();
    state
        .registry
        .register_usb_device(
            make_sim_cdc("1-2", 0x0123, 0xAE3C),
            "Blocked".into(),
            "Hidden".into(),
            "S2".into(),
            &ignored,
        )
        .await
        .unwrap();

    // 1. Public IP over plaintext rejected even with plaintext_lan = true
    let (mut cli, srv) = tokio::io::duplex(8192);
    let st = state.clone();
    let public_peer = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)), 5000);
    let h = tokio::spawn(async move { serve_portico_stream(srv, public_peer, false, st).await });
    write_frame(
        &mut cli,
        &Frame::ClientHello {
            min_ver: 0,
            max_ver: 0,
            auth_kind: AuthKind::ClaimToken,
            credentials: token.as_bytes().to_vec(),
            client_name: "evil".into(),
            capabilities: CAP_USBIP_V1,
        },
    )
    .await
    .unwrap();
    let resp = read_frame(&mut cli).await.unwrap();
    assert!(matches!(
        resp,
        Frame::ServerHello {
            status: HelloStatus::PolicyRejected,
            ..
        }
    ));
    let _ = h.await;

    // 2. Wrong claim token rejected over TLS
    let (mut cli2, srv2) = tokio::io::duplex(8192);
    let st2 = state.clone();
    let lan_peer = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 20)), 5001);
    let h2 = tokio::spawn(async move { serve_portico_stream(srv2, lan_peer, true, st2).await });
    write_frame(
        &mut cli2,
        &Frame::ClientHello {
            min_ver: 0,
            max_ver: 0,
            auth_kind: AuthKind::ClaimToken,
            credentials: b"WRONG".to_vec(),
            client_name: "cli".into(),
            capabilities: CAP_USBIP_V1,
        },
    )
    .await
    .unwrap();
    let resp2 = read_frame(&mut cli2).await.unwrap();
    assert!(matches!(
        resp2,
        Frame::ServerHello {
            status: HelloStatus::AuthFailed,
            ..
        }
    ));
    assert!(read_frame(&mut cli2).await.is_err());
    let _ = h2.await;

    // 3. Valid token lists only shared 1-1, rejects 1-2 tunnel, upgrades 1-1 to usbip
    let (mut cli3, srv3) = tokio::io::duplex(65536);
    let st3 = state.clone();
    let h3 = tokio::spawn(async move { serve_portico_stream(srv3, lan_peer, true, st3).await });
    write_frame(
        &mut cli3,
        &Frame::ClientHello {
            min_ver: PROTOCOL_VERSION_V0,
            max_ver: PROTOCOL_VERSION_V0,
            auth_kind: AuthKind::ClaimToken,
            credentials: token.as_bytes().to_vec(),
            client_name: "desktop".into(),
            capabilities: CAP_USBIP_V1,
        },
    )
    .await
    .unwrap();
    assert!(matches!(
        read_frame(&mut cli3).await.unwrap(),
        Frame::ServerHello {
            status: HelloStatus::Ok,
            ..
        }
    ));

    write_frame(&mut cli3, &Frame::ListDevicesReq).await.unwrap();
    match read_frame(&mut cli3).await.unwrap() {
        Frame::ListDevicesResp { devices } => {
            assert_eq!(devices.len(), 1);
            assert_eq!(devices[0].bus_id, "1-1");
        }
        other => panic!("unexpected: {other:?}"),
    }

    // Tunnel to ignored 1-2 fails
    write_frame(
        &mut cli3,
        &Frame::OpenTunnelReq {
            bus_id: "1-2".into(),
        },
    )
    .await
    .unwrap();
    assert!(matches!(
        read_frame(&mut cli3).await.unwrap(),
        Frame::OpenTunnelResp {
            status: TunnelStatus::Ignored,
            ..
        }
    ));

    // Tunnel to 1-1 succeeds and speaks USB/IP
    write_frame(
        &mut cli3,
        &Frame::OpenTunnelReq {
            bus_id: "1-1".into(),
        },
    )
    .await
    .unwrap();
    assert!(matches!(
        read_frame(&mut cli3).await.unwrap(),
        Frame::OpenTunnelResp {
            status: TunnelStatus::Ok,
            ..
        }
    ));

    let mut busid = b"1-1".to_vec();
    busid.resize(32, 0);
    let import_req = UsbIpCommand::OpReqImport {
        status: 0,
        busid: busid.try_into().unwrap(),
    }
    .to_bytes();
    cli3.write_all(&import_req).await.unwrap();
    let _op_rep_ver = cli3.read_u32().await.unwrap();
    let import_status = cli3.read_u32().await.unwrap();
    assert_eq!(import_status, 0);
    let mut dev_info = vec![0u8; 0x138];
    cli3.read_exact(&mut dev_info).await.unwrap();

    // Verify a second shared device (1-3) CANNOT be imported through 1-1's tunnel
    state
        .registry
        .register_usb_device(
            make_sim_cdc("1-3", 0x9999, 0x8888),
            "Other".into(),
            "Dev".into(),
            "S3".into(),
            &ignored,
        )
        .await
        .unwrap();
    let (mut cli4, srv4) = tokio::io::duplex(65536);
    let st4 = state.clone();
    let h4 = tokio::spawn(async move { serve_portico_stream(srv4, lan_peer, true, st4).await });
    write_frame(
        &mut cli4,
        &Frame::ClientHello {
            min_ver: PROTOCOL_VERSION_V0,
            max_ver: PROTOCOL_VERSION_V0,
            auth_kind: AuthKind::ClaimToken,
            credentials: token.as_bytes().to_vec(),
            client_name: "attacker".into(),
            capabilities: CAP_USBIP_V1,
        },
    )
    .await
    .unwrap();
    let _ = read_frame(&mut cli4).await.unwrap();
    write_frame(
        &mut cli4,
        &Frame::OpenTunnelReq {
            bus_id: "1-3".into(),
        },
    )
    .await
    .unwrap();
    assert!(matches!(
        read_frame(&mut cli4).await.unwrap(),
        Frame::OpenTunnelResp {
            status: TunnelStatus::Ok,
            ..
        }
    ));
    let mut try_1_1 = b"1-1".to_vec();
    try_1_1.resize(32, 0);
    cli4.write_all(
        &UsbIpCommand::OpReqImport {
            status: 0,
            busid: try_1_1.try_into().unwrap(),
        }
        .to_bytes(),
    )
    .await
    .unwrap();
    let _v = cli4.read_u32().await.unwrap();
    assert_ne!(
        cli4.read_u32().await.unwrap(),
        0,
        "tunnel for 1-3 must NOT be able to import 1-1"
    );
    drop(cli4);
    let _ = h4.await;

    // Send GetDescriptor(Device)
    let submit = UsbIpCommand::UsbIpCmdSubmit {
        header: usbip::usbip_protocol::UsbIpHeaderBasic {
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
    cli3.write_all(&submit).await.unwrap();
    let mut ret_hdr = [0u8; 48];
    cli3.read_exact(&mut ret_hdr).await.unwrap();
    let mut desc = [0u8; 18];
    cli3.read_exact(&mut desc).await.unwrap();
    assert_eq!(desc[0], 0x12);
    assert_eq!(desc[1], 0x01);

    drop(cli3);
    let _ = h3.await;
}
