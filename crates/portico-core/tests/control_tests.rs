use portico_core::daemon::PorticoDaemon;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tempfile::TempDir;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use usbip::cdc::{UsbCdcAcmHandler, CDC_ACM_SUBCLASS};
use usbip::{ClassCode, UsbDevice};

async fn send_cmd(sock: &mut BufReader<tokio::net::unix::OwnedReadHalf>, w: &mut tokio::net::unix::OwnedWriteHalf, cmd: &str) -> Vec<String> {
    w.write_all(cmd.as_bytes()).await.unwrap();
    w.write_all(b"\n").await.unwrap();
    w.flush().await.unwrap();

    let mut first = String::new();
    sock.read_line(&mut first).await.unwrap();
    let first = first.trim_end_matches(&['\r', '\n'][..]).to_string();
    let mut lines = vec![first.clone()];
    if let Some(rest) = first.strip_prefix("OK ") {
        if let Ok(_count) = rest.trim().parse::<usize>() {
            loop {
                let mut l = String::new();
                sock.read_line(&mut l).await.unwrap();
                let l = l.trim_end_matches(&['\r', '\n'][..]).to_string();
                let is_end = l == "END";
                lines.push(l);
                if is_end {
                    break;
                }
            }
        }
    }
    lines
}

#[tokio::test]
async fn control_channel_covers_all_commands_and_subscribe_events() {
    let dir = TempDir::new().unwrap();
    let sock_path = dir.path().join("portico-ctrl.sock");
    let daemon = PorticoDaemon::start_with_bind_overrides(
        dir.path().to_path_buf(),
        sock_path.clone(),
        Some(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0)),
        Some(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0)),
    )
    .await
    .unwrap();

    // Event subscriber stream
    let sub_stream = UnixStream::connect(&sock_path).await.unwrap();
    let (sub_r, mut sub_w) = sub_stream.into_split();
    let mut sub_reader = BufReader::new(sub_r);
    sub_w.write_all(b"SUBSCRIBE\n").await.unwrap();
    sub_w.flush().await.unwrap();
    let mut ack = String::new();
    sub_reader.read_line(&mut ack).await.unwrap();
    assert_eq!(ack.trim(), "OK");

    // Command stream
    let cmd_stream = UnixStream::connect(&sock_path).await.unwrap();
    let (cr, mut cw) = cmd_stream.into_split();
    let mut cr = BufReader::new(cr);

    let reg = send_cmd(&mut cr, &mut cw, "REGISTER_CLIENT android-ui").await;
    let client_id = reg[0].strip_prefix("OK ").unwrap().to_string();
    assert!(!client_id.is_empty());

    let state = send_cmd(&mut cr, &mut cw, "GET_STATE").await;
    assert!(state[0].starts_with("OK "));
    assert!(state[0].contains("tls=true"));
    assert!(state[0].contains("plaintext_lan=false"));
    assert!(state[0].contains("tier=free"));

    assert_eq!(
        send_cmd(&mut cr, &mut cw, "SET_IGNORED 0123/ae3c,046D:C52B").await[0],
        "OK"
    );
    let ign = send_cmd(&mut cr, &mut cw, "GET_IGNORED").await;
    assert_eq!(ign, vec!["OK 2", "ITEM 0123/AE3C", "ITEM 046D/C52B", "END"]);

    // Invalid SET_IGNORED preserves previous list
    assert!(send_cmd(&mut cr, &mut cw, "SET_IGNORED BAD")
        .await[0]
        .starts_with("ERR "));
    let ign2 = send_cmd(&mut cr, &mut cw, "GET_IGNORED").await;
    assert_eq!(ign2, vec!["OK 2", "ITEM 0123/AE3C", "ITEM 046D/C52B", "END"]);

    let fp = "a".repeat(64);
    assert_eq!(
        send_cmd(
            &mut cr,
            &mut cw,
            &format!("SET_REVERSE_CLIENTS 192.168.1.10:37425@{fp}@true")
        )
        .await[0],
        "OK"
    );
    assert!(
        send_cmd(&mut cr, &mut cw, "SET_REVERSE_CLIENTS bad_no_port@dead@true").await[0]
            .starts_with("ERR ")
    );
    let rev = send_cmd(&mut cr, &mut cw, "GET_REVERSE_CLIENTS").await;
    assert_eq!(rev.len(), 3);
    assert!(rev[1].contains("addr=192.168.1.10:37425"));

    assert_eq!(send_cmd(&mut cr, &mut cw, "SET_NET_LOCK true").await[0], "OK");
    assert_eq!(send_cmd(&mut cr, &mut cw, "SET_WAKE_LOCK true").await[0], "OK");
    assert_eq!(send_cmd(&mut cr, &mut cw, "SET_SPEED high").await[0], "OK");
    assert_eq!(send_cmd(&mut cr, &mut cw, "SET_RELAY false").await[0], "OK");
    assert!(send_cmd(&mut cr, &mut cw, "GET_RELAY").await[0].contains("status=disabled"));

    let tok1 = send_cmd(&mut cr, &mut cw, "GET_TOKEN").await[0]
        .strip_prefix("OK ")
        .unwrap()
        .to_string();
    let tok2 = send_cmd(&mut cr, &mut cw, "REGEN_TOKEN").await[0]
        .strip_prefix("OK ")
        .unwrap()
        .to_string();
    assert_ne!(tok1, tok2);

    assert_eq!(send_cmd(&mut cr, &mut cw, "START_SERVER").await[0], "OK");

    // Register a device via handle and check LIST_DEVICES + DETACH_DEVICE
    let mut sim = UsbDevice::new(1).with_interface(
        ClassCode::CDC as u8,
        CDC_ACM_SUBCLASS,
        0,
        Some("CDC"),
        UsbCdcAcmHandler::endpoints(),
        Arc::new(Mutex::new(Box::new(UsbCdcAcmHandler::new()))),
    );
    sim.bus_id = "1-9".into();
    sim.vendor_id = 0x0781;
    sim.product_id = 0x5583;
    daemon
        .handle()
        .state
        .registry
        .register_usb_device(sim, "SanDisk".into(), "Flash Drive".into(), "S9".into(), &[])
        .await
        .unwrap();

    let devs = send_cmd(&mut cr, &mut cw, "LIST_DEVICES").await;
    assert_eq!(devs[0], "OK 1");
    assert!(devs[1].contains("bus_id=1-9"));
    assert!(devs[1].contains("product=Flash%20Drive"));

    assert_eq!(send_cmd(&mut cr, &mut cw, "DETACH_DEVICE 1-9").await[0], "OK");
    assert_eq!(send_cmd(&mut cr, &mut cw, "LIST_DEVICES").await, vec!["OK 0", "END"]);

    assert_eq!(send_cmd(&mut cr, &mut cw, "STOP_SERVER").await[0], "OK");
    assert_eq!(
        send_cmd(&mut cr, &mut cw, &format!("UNREGISTER_CLIENT {client_id}")).await[0],
        "OK"
    );

    let logs = send_cmd(&mut cr, &mut cw, "GET_LOGS 0").await;
    assert!(logs.len() >= 3);

    // Check subscriber got SERVER_STATE event
    let mut saw_server_event = false;
    for _ in 0..20 {
        let mut line = String::new();
        if tokio::time::timeout(Duration::from_millis(200), sub_reader.read_line(&mut line))
            .await
            .is_ok()
        {
            if line.starts_with("EVENT SERVER_STATE running=true") {
                saw_server_event = true;
                break;
            }
        }
    }
    assert!(saw_server_event);

    assert_eq!(send_cmd(&mut cr, &mut cw, "SHUTDOWN").await[0], "OK");
    daemon.shutdown().await;
}
