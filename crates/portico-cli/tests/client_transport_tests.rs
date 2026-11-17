use portico_cli::transport::{cmd_list, cmd_pair, cmd_trust, resolve_token, ConnectOptions, TargetEndpoint};
use portico_cli::CliError;
use portico_core::daemon::PorticoDaemon;
use portico_proto::tofu::KnownHosts;
use portico_relay::RelayServer;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tempfile::TempDir;
use tokio::net::TcpListener;
use usbip::cdc::{UsbCdcAcmHandler, CDC_ACM_SUBCLASS};
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
    d.vendor_id = 0x0781;
    d.product_id = 0x5583;
    d
}

#[tokio::test]
async fn tofu_pin_mismatch_public_plaintext_rejection_and_relay_list() {
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
            "SanDisk".into(),
            "Ultra".into(),
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

    // 1. Plaintext rejected for public IP before any network I/O
    let pub_res = cmd_list(ConnectOptions {
        config_dir: cli_dir.path(),
        target: "8.8.8.8:37420",
        token: &token,
        trust_fp: None,
        insecure_lan_plaintext: true,
        client_name: "cli",
    })
    .await;
    assert!(matches!(pub_res, Err(CliError::PlaintextNonLanRefused(_))));

    // 2. First contact without trust fails with UntrustedCert containing server fp
    let untrusted = cmd_list(ConnectOptions {
        config_dir: cli_dir.path(),
        target: &tcp_addr,
        token: &token,
        trust_fp: None,
        insecure_lan_plaintext: false,
        client_name: "cli",
    })
    .await;
    match untrusted {
        Err(CliError::UntrustedCert { fingerprint, .. }) => assert_eq!(fingerprint, fp),
        other => panic!("expected UntrustedCert, got {other:?}"),
    }

    // 3. Explicit trust pins cert and cmd_list returns device
    cmd_trust(cli_dir.path(), &tcp_addr, &fp).unwrap();
    let list = cmd_list(ConnectOptions {
        config_dir: cli_dir.path(),
        target: &tcp_addr,
        token: &token,
        trust_fp: None,
        insecure_lan_plaintext: false,
        client_name: "cli",
    })
    .await
    .unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].bus_id, "1-1");

    // 4. Server rotates identity -> client pin mismatch aborts
    let new_fp = h
        .exec_command("ROTATE_IDENTITY")
        .await
        .strip_prefix("OK ")
        .unwrap()
        .to_string();
    let mismatch = cmd_list(ConnectOptions {
        config_dir: cli_dir.path(),
        target: &tcp_addr,
        token: &token,
        trust_fp: None,
        insecure_lan_plaintext: false,
        client_name: "cli",
    })
    .await;
    assert!(matches!(
        mismatch,
        Err(CliError::CertPinMismatch { expected, actual, .. }) if expected == fp && actual == new_fp
    ));

    // 5. Connect via prly:// relay URI with --trust <new_fp>
    let relay_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let relay_addr = relay_listener.local_addr().unwrap();
    let relay_task = tokio::spawn(async move {
        let _ = RelayServer::run(relay_listener).await;
    });
    assert_eq!(
        h.exec_command(&format!("SET_RELAY true {relay_addr}")).await,
        "OK"
    );
    let mut relay_uri = None;
    for _ in 0..40 {
        let r = h.exec_command("GET_RELAY").await;
        if let Some(addr_part) = r.split_whitespace().find_map(|kv| kv.strip_prefix("address=")) {
            if addr_part != "-" && r.contains("status=ready") {
                relay_uri = Some(addr_part.replace("%3A", ":").replace("%2F", "/"));
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let relay_uri = relay_uri.expect("relay ready");
    assert!(matches!(
        TargetEndpoint::parse(&relay_uri),
        Ok(TargetEndpoint::Relay { .. })
    ));

    let relay_list = cmd_list(ConnectOptions {
        config_dir: cli_dir.path(),
        target: &relay_uri,
        token: &token,
        trust_fp: Some(&new_fp),
        insecure_lan_plaintext: false,
        client_name: "cli-relay",
    })
    .await
    .unwrap();
    assert_eq!(relay_list.len(), 1);
    assert_eq!(relay_list[0].bus_id, "1-1");

    // Malformed prly:// URIs and unbracketed IPv6 target parsing
    assert!(TargetEndpoint::parse("prly://127.0.0.1:37422/badhex").is_err());
    assert!(TargetEndpoint::parse("prly://127.0.0.1:37422/").is_err());
    assert_eq!(
        TargetEndpoint::parse("::1").unwrap(),
        TargetEndpoint::Direct("[::1]:37420".into())
    );
    assert!(cmd_list(ConnectOptions {
        config_dir: cli_dir.path(),
        target: "prly://127.0.0.1:37422/not-hex",
        token: &token,
        trust_fp: None,
        insecure_lan_plaintext: false,
        client_name: "cli",
    })
    .await
    .is_err());

    relay_task.abort();
    daemon.shutdown().await;
}

#[tokio::test]
async fn pair_confirm_declines_then_pins_stores_token_and_list_uses_store() {
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
            "SanDisk".into(),
            "Ultra".into(),
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

    // 1. Declined confirm: nothing is pinned
    cmd_pair(cli_dir.path(), &tcp_addr, Some(&token), false, |_| false)
        .await
        .unwrap();
    assert!(!cli_dir.path().join("known_hosts.json").exists());

    // 2. Confirmed pairing: pin + stored token
    cmd_pair(cli_dir.path(), &tcp_addr, Some(&token), false, |_| true)
        .await
        .unwrap();
    let kh = KnownHosts::load(&cli_dir.path().join("known_hosts.json")).unwrap();
    assert_eq!(kh.entries.get(&tcp_addr).map(|s| s.as_str()), Some(fp.as_str()));
    assert_eq!(
        resolve_token(cli_dir.path(), &tcp_addr, None).unwrap(),
        token
    );

    // 3. list works with the stored token (no --token flag)
    let stored = resolve_token(cli_dir.path(), &tcp_addr, None).unwrap();
    let list = cmd_list(ConnectOptions {
        config_dir: cli_dir.path(),
        target: &tcp_addr,
        token: &stored,
        trust_fp: None,
        insecure_lan_plaintext: false,
        client_name: "cli",
    })
    .await
    .unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].bus_id, "1-1");

    // 4. Re-pair on already-paired endpoint is a no-op (confirm not called)
    cmd_pair(cli_dir.path(), &tcp_addr, None, true, |_| {
        unreachable!("no prompt when already paired")
    })
    .await
    .unwrap();

    // 5. Unknown endpoint with no pin and no stored token: crisp error
    let err = resolve_token(cli_dir.path(), "192.168.99.99:37420", None).unwrap_err();
    assert!(format!("{err}").contains("no claim token"));

    daemon.shutdown().await;
}
