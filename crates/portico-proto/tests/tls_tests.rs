use portico_proto::identity::NodeIdentity;
use portico_proto::tls::{
    build_client_tls_config, build_server_tls_config, is_lan_or_loopback_ip, CertCheckPolicy,
    PinAllowlist,
};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::{TlsAcceptor, TlsConnector};

#[test]
fn lan_and_loopback_ip_guard() {
    let lan = [
        IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)),
        IpAddr::V4(Ipv4Addr::new(10, 1, 2, 3)),
        IpAddr::V4(Ipv4Addr::new(172, 16, 0, 1)),
        IpAddr::V4(Ipv4Addr::new(172, 31, 255, 254)),
        IpAddr::V4(Ipv4Addr::new(192, 168, 1, 50)),
        IpAddr::V4(Ipv4Addr::new(169, 254, 10, 20)),
        IpAddr::V6(Ipv6Addr::LOCALHOST),
        IpAddr::V6("fd12:3456::1".parse().unwrap()),
        IpAddr::V6("fe80::1".parse().unwrap()),
    ];
    for ip in lan {
        assert!(is_lan_or_loopback_ip(ip), "expected LAN: {ip}");
    }

    let public = [
        IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)),
        IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1)),
        IpAddr::V4(Ipv4Addr::new(172, 32, 0, 1)),
        IpAddr::V4(Ipv4Addr::new(192, 167, 1, 1)),
        IpAddr::V6("2001:4860:4860::8888".parse().unwrap()),
    ];
    for ip in public {
        assert!(!is_lan_or_loopback_ip(ip), "expected non-LAN: {ip}");
    }
}

#[tokio::test]
async fn tls13_tofu_pin_match_and_mismatch() {
    let server_id = NodeIdentity::generate("portico-server").unwrap();
    let server_fp = server_id.fingerprint_hex();
    let srv_cfg = build_server_tls_config(&server_id, None).unwrap();
    let acceptor = TlsAcceptor::from(srv_cfg);

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let srv_task = tokio::spawn(async move {
        for _ in 0..2 {
            if let Ok((tcp, _)) = listener.accept().await {
                let acc = acceptor.clone();
                tokio::spawn(async move {
                    if let Ok(mut tls) = acc.accept(tcp).await {
                        let mut b = [0u8; 4];
                        if tls.read_exact(&mut b).await.is_ok() {
                            let _ = tls.write_all(&b).await;
                        }
                    }
                });
            }
        }
    });

    // 1. Matching pin succeeds
    let (cli_cfg, observed) = build_client_tls_config(
        None,
        CertCheckPolicy::PinnedOrFirstUse {
            expected_fp: Some(server_fp.clone()),
            trust_first_use: false,
        },
    )
    .unwrap();
    let connector = TlsConnector::from(cli_cfg);
    let tcp = TcpStream::connect(addr).await.unwrap();
    let server_name = "localhost".try_into().unwrap();
    let mut tls = connector.connect(server_name, tcp).await.unwrap();
    assert_eq!(observed.get().as_deref(), Some(server_fp.as_str()));
    tls.write_all(b"ping").await.unwrap();
    let mut resp = [0u8; 4];
    tls.read_exact(&mut resp).await.unwrap();
    assert_eq!(&resp, b"ping");

    // 2. Wrong pin aborts TLS handshake
    let (bad_cfg, _) = build_client_tls_config(
        None,
        CertCheckPolicy::PinnedOrFirstUse {
            expected_fp: Some("0".repeat(64)),
            trust_first_use: false,
        },
    )
    .unwrap();
    let bad_connector = TlsConnector::from(bad_cfg);
    let tcp2 = TcpStream::connect(addr).await.unwrap();
    let res = bad_connector
        .connect("localhost".try_into().unwrap(), tcp2)
        .await;
    assert!(res.is_err(), "mismatched cert pin must abort TLS");

    srv_task.abort();
}

#[tokio::test]
async fn mtls_requires_pinned_client_certificate() {
    let server_id = NodeIdentity::generate("portico-server").unwrap();
    let allowed_client = NodeIdentity::generate("allowed-client").unwrap();
    let rogue_client = NodeIdentity::generate("rogue-client").unwrap();

    let allowlist = Arc::new(PinAllowlist::new(vec![allowed_client.fingerprint_hex()]));
    let srv_cfg = build_server_tls_config(&server_id, Some(allowlist)).unwrap();
    let acceptor = TlsAcceptor::from(srv_cfg);

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let srv_task = tokio::spawn(async move {
        for _ in 0..2 {
            if let Ok((tcp, _)) = listener.accept().await {
                let acc = acceptor.clone();
                tokio::spawn(async move {
                    if let Ok(mut tls) = acc.accept(tcp).await {
                        let _ = tls.write_all(b"ok").await;
                    }
                });
            }
        }
    });

    // Allowed client succeeds
    let (cli_ok, _) = build_client_tls_config(
        Some(&allowed_client),
        CertCheckPolicy::PinnedOrFirstUse {
            expected_fp: Some(server_id.fingerprint_hex()),
            trust_first_use: false,
        },
    )
    .unwrap();
    let tcp1 = TcpStream::connect(addr).await.unwrap();
    let mut tls1 = TlsConnector::from(cli_ok)
        .connect("localhost".try_into().unwrap(), tcp1)
        .await
        .unwrap();
    let mut buf = [0u8; 2];
    tls1.read_exact(&mut buf).await.unwrap();
    assert_eq!(&buf, b"ok");

    // Rogue client rejected
    let (cli_bad, _) = build_client_tls_config(
        Some(&rogue_client),
        CertCheckPolicy::PinnedOrFirstUse {
            expected_fp: Some(server_id.fingerprint_hex()),
            trust_first_use: false,
        },
    )
    .unwrap();
    let tcp2 = TcpStream::connect(addr).await.unwrap();
    let mut tls2 = TlsConnector::from(cli_bad)
        .connect("localhost".try_into().unwrap(), tcp2)
        .await
        .ok();
    if let Some(ref mut s) = tls2 {
        // In TLS 1.3, server verifies client CertificateVerify after ServerFinished; read fails with alert.
        let mut b = [0u8; 2];
        assert!(s.read_exact(&mut b).await.is_err());
    }

    srv_task.abort();
}
