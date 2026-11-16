use portico_core::config::{ConfigStore, LicenseTier, ReverseClientEntry, UsbSpeedSetting, VidPid};
use portico_proto::DEFAULT_PORT;
use tempfile::TempDir;

#[test]
fn first_run_initializes_defaults_and_persists_secrets() {
    let dir = TempDir::new().unwrap();
    let store1 = ConfigStore::load_or_init(dir.path()).unwrap();
    assert_eq!(store1.config.port, DEFAULT_PORT);
    assert!(store1.config.tls_required);
    assert!(!store1.config.plaintext_lan);
    assert_eq!(store1.config.claim_token.len(), 24);
    assert_eq!(store1.config.relay.slot_secret_hex.len(), 64);
    assert_eq!(store1.tier(), LicenseTier::Free);

    let store2 = ConfigStore::load_or_init(dir.path()).unwrap();
    assert_eq!(store2.config.claim_token, store1.config.claim_token);
    assert_eq!(
        store2.config.relay.slot_secret_hex,
        store1.config.relay.slot_secret_hex
    );
}

#[test]
fn vid_pid_parse_and_display() {
    assert_eq!(
        VidPid::parse("0123/ae3c").unwrap(),
        VidPid {
            vid: 0x0123,
            pid: 0xAE3C
        }
    );
    assert_eq!(
        VidPid::parse("0781:5583").unwrap().to_string(),
        "0781/5583"
    );
    assert_eq!(
        VidPid::parse("046dC52B").unwrap(),
        VidPid {
            vid: 0x046D,
            pid: 0xC52B
        }
    );

    assert!(VidPid::parse("123/AE3C").is_err());
    assert!(VidPid::parse("ZZZZ/1234").is_err());
    assert!(VidPid::parse("").is_err());
}

#[test]
fn reverse_client_and_speed_validation() {
    let valid = ReverseClientEntry {
        id: "desktop".into(),
        addr: "192.168.1.10:37425".into(),
        cert_sha256: "A".repeat(64),
        enabled: true,
    }
    .validate()
    .unwrap();
    assert_eq!(valid.cert_sha256, "a".repeat(64));

    let bad_fp = ReverseClientEntry {
        id: "desktop".into(),
        addr: "192.168.1.10:37425".into(),
        cert_sha256: "deadbeef".into(),
        enabled: true,
    };
    assert!(bad_fp.validate().is_err());

    let bad_addr = ReverseClientEntry {
        id: "desktop".into(),
        addr: "invalid-no-port".into(),
        cert_sha256: "a".repeat(64),
        enabled: true,
    };
    assert!(bad_addr.validate().is_err());

    assert_eq!(UsbSpeedSetting::parse("high").unwrap(), UsbSpeedSetting::High);
    assert!(UsbSpeedSetting::parse("warp").is_err());
}
