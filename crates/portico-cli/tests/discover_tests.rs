use portico_cli::discover::discover_servers;
use portico_core::daemon::PorticoDaemon;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;
use tempfile::TempDir;

#[tokio::test]
async fn discovers_running_daemon_over_udp() {
    let dir = TempDir::new().unwrap();
    let sock = dir.path().join("portico-ctrl.sock");
    let daemon = PorticoDaemon::start_with_bind_overrides(
        dir.path().to_path_buf(),
        sock,
        Some(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0)),
        Some(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0)),
    )
    .await
    .unwrap();
    let h = daemon.handle();
    assert_eq!(h.exec_command("START_SERVER").await, "OK");

    let tcp_addr = h.network.bound_tcp_addr().await.unwrap();
    let udp_addr = h.network.bound_udp_addr().await.unwrap();
    let expected_prefix = h.state.identity.read().await.fingerprint_hex()[..16].to_string();

    let found = discover_servers(
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
        udp_addr,
        Duration::from_millis(400),
    )
    .await
    .unwrap();

    assert_eq!(found.len(), 1);
    assert_eq!(found[0].tcp_port, tcp_addr.port());
    assert_eq!(found[0].cert_fp_prefix, expected_prefix);
    assert!(found[0].tls_required);

    daemon.shutdown().await;
}
