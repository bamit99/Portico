use portico_cli::session::{
    decode_mountinfo_path, find_active, load_attachments, new_mounts, parse_vhci_port, pid_alive,
    remove_attachment, save_attachment, Attachment,
};
use std::collections::HashSet;
use std::path::Path;

fn att(bus_id: &str, pid: u32) -> Attachment {
    Attachment {
        bus_id: bus_id.to_string(),
        target: "192.168.0.181:37420".to_string(),
        pid,
        vhci_port: Some(0),
        started_at: 1,
        log_path: "/tmp/x.log".to_string(),
        mount_points: vec![],
    }
}

#[test]
fn session_records_roundtrip_and_find_active_checks_pid() {
    let tmp = tempfile::TempDir::new().unwrap();
    std::env::set_var("PORTICO_SESSIONS_DIR", tmp.path().join("sessions"));

    let self_pid = std::process::id();
    save_attachment(Path::new("/cfg"), &att("1-2", self_pid)).unwrap();
    // dead pid record — must be ignored by find_active
    save_attachment(Path::new("/cfg"), &att("3-4", 999_999_999)).unwrap();

    assert_eq!(load_attachments(Path::new("/cfg")).len(), 2);
    assert!(pid_alive(self_pid));
    assert!(!pid_alive(999_999_999));
    assert_eq!(find_active(Path::new("/cfg"), "1-2").map(|a| a.pid), Some(self_pid));
    assert!(find_active(Path::new("/cfg"), "3-4").is_none());
    assert!(find_active(Path::new("/cfg"), "9-9").is_none());

    remove_attachment(Path::new("/cfg"), "1-2").unwrap();
    assert!(find_active(Path::new("/cfg"), "1-2").is_none());

    std::env::remove_var("PORTICO_SESSIONS_DIR");
}

#[test]
fn mount_helpers_decode_and_diff() {
    assert_eq!(decode_mountinfo_path("External\\0402"), "External 2");
    let before: HashSet<String> = ["/run/media/amit/2TBNVME".to_string()].into_iter().collect();
    let after: HashSet<String> = [
        "/run/media/amit/2TBNVME".to_string(),
        "/run/media/amit/Ultra".to_string(),
        "/media/other".to_string(),
    ]
    .into_iter()
    .collect();
    assert_eq!(
        new_mounts(&before, &after),
        vec!["/media/other".to_string(), "/run/media/amit/Ultra".to_string()]
    );
    // real /proc is readable on Linux; just assert it doesn't panic
    let _ = portico_cli::session::snapshot_mounts();
}

#[test]
fn parse_vhci_port_finds_matching_bus() {
    let out = "Imported USB devices\n====================\nPort 00: <Port in Use> at High Speed(480Mbps)\n           0781:5583 SanDisk\n           1-2 -> usbip://127.0.0.1:34981/1-2\nPort 01: <Port in Use>\n           1-3 -> usbip://127.0.0.1:34982/1-3\n";
    assert_eq!(parse_vhci_port(out, "1-2"), Some(0));
    assert_eq!(parse_vhci_port(out, "1-3"), Some(1));
    assert_eq!(parse_vhci_port(out, "9-9"), None);
}
