use portico_core::config::ReverseClientEntry;
use portico_core::network::NetworkSupervisor;
use portico_core::state::{DaemonState, RelayStatus};
use portico_proto::discovery::DiscoveryPacket;
use portico_proto::frame::{
    read_frame, write_frame, AuthKind, Frame, HelloStatus, TunnelStatus, CAP_USBIP_V1,
};
use portico_proto::identity::NodeIdentity;
use portico_proto::relay_proto::{parse_relay_uri, RelayFrame};
use portico_proto::tls::{
    build_client_tls_config, build_server_tls_config, CertCheckPolicy, PinAllowlist,
};
use portico_relay::RelayServer;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio_rustls::{TlsAcceptor, TlsConnector};
use usbip::cdc::{UsbCdcAcmHandler, CDC_ACM_SUBCLASS};
use usbip::usbip_protocol::UsbIpCommand;
use usbip::{ClassCode, UsbDevice};

fn make_sim_cdc(bus_id: &str) -> UsbDevice {
    let mut d = UsbDevice::new(1).with_interface(
        ClassCode::CDC as u8,
        CDC_ACM_SUBCLASS,
        0x00,
        Some("CDC"),
        UsbCdcAcmHandler::endpoints(),
        Arc::new(Mutex::new(Box::new(UsbCdcAcmHandler::new()))),
    );
    d.bus_id = bus_id.to_string();
    d.vendor_id = 0x0781;
    d.product_id = 0x5583;
    d
}

#[tokio::test]
async fn tls_plaintext_peek_discovery_relay_and_reverse_mtls() {
    let dir = TempDir::new().unwrap();
    let state = DaemonState::new(dir.path().to_path_buf()).unwrap();
    let token = state.config.lock().await.config.claim_token.clone();
    let server_fp = state.identity.read().await.fingerprint_hex();

    state
        .registry
        .register_usb_device(
            make_sim_cdc("1-1"),
            "SanDisk".into(),
            "Ultra".into(),
            "S1".into(),
            &[],
        )
        .await
        .unwrap();

    let net = NetworkSupervisor::new();
    let (tcp_addr, udp_addr) = net
        .start_server(
            state.clone(),
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
            Some(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0)),
        )
        .await
        .unwrap();
    let udp_addr = udp_addr.unwrap();

    // 1. Plaintext rejected when plaintext_lan == false
    {
        let mut plain = TcpStream::connect(tcp_addr).await.unwrap();
        write_frame(
            &mut plain,
            &Frame::ClientHello {
                min_ver: 0,
                max_ver: 0,
                auth_kind: AuthKind::ClaimToken,
                credentials: token.as_bytes().to_vec(),
                client_name: "plain".into(),
                capabilities: CAP_USBIP_V1,
            },
        )
        .await
        .unwrap();
        assert!(matches!(
            read_frame(&mut plain).await.unwrap(),
            Frame::ServerHello {
                status: HelloStatus::PolicyRejected,
                ..
            }
        ));
    }

    // 2. Enable plaintext_lan -> loopback plaintext works AND TLS still works on same port
    state.config.lock().await.config.plaintext_lan = true;
    {
        let mut plain = TcpStream::connect(tcp_addr).await.unwrap();
        write_frame(
            &mut plain,
            &Frame::ClientHello {
                min_ver: 0,
                max_ver: 0,
                auth_kind: AuthKind::ClaimToken,
                credentials: token.as_bytes().to_vec(),
                client_name: "plain-ok".into(),
                capabilities: CAP_USBIP_V1,
            },
        )
        .await
        .unwrap();
        assert!(matches!(
            read_frame(&mut plain).await.unwrap(),
            Frame::ServerHello {
                status: HelloStatus::Ok,
                ..
            }
        ));
    }

    // 3. UDP Discovery responds with matching nonce & tcp_port
    {
        let sock = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let nonce = [1, 2, 3, 4, 5, 6, 7, 8];
        sock.send_to(&DiscoveryPacket::Request { nonce }.encode(), udp_addr)
            .await
            .unwrap();
        let mut buf = [0u8; 512];
        let (n, _) = tokio::time::timeout(Duration::from_secs(2), sock.recv_from(&mut buf))
            .await
            .unwrap()
            .unwrap();
        match DiscoveryPacket::decode(&buf[..n]).unwrap() {
            DiscoveryPacket::Response {
                nonce: rnonce,
                tcp_port,
                ..
            } => {
                assert_eq!(rnonce, nonce);
                assert_eq!(tcp_port, tcp_addr.port());
            }
            other => panic!("unexpected packet: {other:?}"),
        }
    }

    // 4. Relay integration
    let relay_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let relay_addr = relay_listener.local_addr().unwrap();
    let relay_srv_task = tokio::spawn(async move {
        let _ = RelayServer::run(relay_listener).await;
    });
    {
        let mut cfg = state.config.lock().await;
        cfg.config.relay.enabled = true;
        cfg.config.relay.endpoint = relay_addr.to_string();
    }
    net.sync_relay(state.clone()).await;

    let mut ready_uri = None;
    for _ in 0..40 {
        if let RelayStatus::Ready(uri) = state.relay_status.read().await.clone() {
            ready_uri = Some(uri);
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let ready_uri = ready_uri.expect("relay should reach Ready");
    let (ep, node_id) = parse_relay_uri(&ready_uri).unwrap();
    assert_eq!(ep, relay_addr.to_string());

    {
        let mut cli_tcp = TcpStream::connect(relay_addr).await.unwrap();
        RelayFrame::ClientConnect {
            node_id,
            conn_nonce: [0x55; 16],
        }
        .write_to(&mut cli_tcp)
        .await
        .unwrap();

        let (cli_tls_cfg, _) = build_client_tls_config(
            None,
            CertCheckPolicy::PinnedOrFirstUse {
                expected_fp: Some(server_fp.clone()),
                trust_first_use: false,
            },
        )
        .unwrap();
        let mut tls = TlsConnector::from(cli_tls_cfg)
            .connect("localhost".try_into().unwrap(), cli_tcp)
            .await
            .unwrap();
        write_frame(
            &mut tls,
            &Frame::ClientHello {
                min_ver: 0,
                max_ver: 0,
                auth_kind: AuthKind::ClaimToken,
                credentials: token.as_bytes().to_vec(),
                client_name: "relayed".into(),
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
        write_frame(&mut tls, &Frame::ListDevicesReq).await.unwrap();
        match read_frame(&mut tls).await.unwrap() {
            Frame::ListDevicesResp { devices } => assert_eq!(devices.len(), 1),
            other => panic!("unexpected: {other:?}"),
        }
    }

    // 5. Reverse-client mTLS dial + RequestReverseDataConn
    let rev_identity = NodeIdentity::generate("reverse-listener").unwrap();
    let rev_allow = Arc::new(PinAllowlist::new(vec![server_fp.clone()]));
    let rev_tls_cfg = build_server_tls_config(&rev_identity, Some(rev_allow)).unwrap();
    let rev_acceptor = TlsAcceptor::from(rev_tls_cfg);
    let rev_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let rev_addr = rev_listener.local_addr().unwrap();

    {
        let mut cfg = state.config.lock().await;
        cfg.config.reverse_clients = vec![ReverseClientEntry {
            id: "desk".into(),
            addr: rev_addr.to_string(),
            cert_sha256: rev_identity.fingerprint_hex(),
            enabled: true,
        }];
    }
    net.sync_reverse_clients(state.clone()).await;

    // Accept reverse control connection
    let (tcp_ctrl, _) = tokio::time::timeout(Duration::from_secs(3), rev_listener.accept())
        .await
        .unwrap()
        .unwrap();
    let mut ctrl_tls = rev_acceptor.accept(tcp_ctrl).await.unwrap();
    write_frame(
        &mut ctrl_tls,
        &Frame::ClientHello {
            min_ver: 0,
            max_ver: 0,
            auth_kind: AuthKind::ClaimToken,
            credentials: token.as_bytes().to_vec(),
            client_name: "rev-desk".into(),
            capabilities: CAP_USBIP_V1,
        },
    )
    .await
    .unwrap();
    assert!(matches!(
        read_frame(&mut ctrl_tls).await.unwrap(),
        Frame::ServerHello {
            status: HelloStatus::Ok,
            ..
        }
    ));

    let nonce = [0x99u8; 16];
    write_frame(
        &mut ctrl_tls,
        &Frame::RequestReverseDataConn {
            conn_nonce: nonce,
            bus_id: "1-1".into(),
        },
    )
    .await
    .unwrap();

    let (tcp_data, _) = tokio::time::timeout(Duration::from_secs(3), rev_listener.accept())
        .await
        .unwrap()
        .unwrap();
    let mut data_tls = rev_acceptor.accept(tcp_data).await.unwrap();
    assert_eq!(
        read_frame(&mut data_tls).await.unwrap(),
        Frame::ReverseDataHello { conn_nonce: nonce }
    );
    assert!(matches!(
        read_frame(&mut data_tls).await.unwrap(),
        Frame::OpenTunnelResp {
            status: TunnelStatus::Ok,
            ..
        }
    ));

    // Confirm USB/IP works over the reverse data tunnel
    let mut busid = b"1-1".to_vec();
    busid.resize(32, 0);
    let import_req = UsbIpCommand::OpReqImport {
        status: 0,
        busid: busid.try_into().unwrap(),
    }
    .to_bytes();
    data_tls.write_all(&import_req).await.unwrap();
    let _ver = data_tls.read_u32().await.unwrap();
    let status = data_tls.read_u32().await.unwrap();
    assert_eq!(status, 0);

    net.stop_server(&state).await;
    relay_srv_task.abort();
}
