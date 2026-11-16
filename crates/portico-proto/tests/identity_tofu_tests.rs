use portico_proto::identity::{
    cert_fingerprint_hex, format_short_fingerprint, normalize_fingerprint, NodeIdentity,
};
use portico_proto::tofu::{KnownHosts, TofuDecision};
use portico_proto::ProtoError;
use tempfile::TempDir;

#[test]
fn load_or_generate_persists_and_reloads_identical_identity() {
    let dir = TempDir::new().unwrap();
    let id1 = NodeIdentity::load_or_generate(dir.path(), "identity", "portico-node").unwrap();
    let id2 = NodeIdentity::load_or_generate(dir.path(), "identity", "portico-node").unwrap();

    assert_eq!(id1, id2);
    assert_eq!(id1.fingerprint_hex().len(), 64);
    assert_eq!(id1.fingerprint_hex(), cert_fingerprint_hex(&id1.cert_der));
    assert_eq!(id1.node_id(), id2.node_id());

    let short = format_short_fingerprint(&id1.fingerprint_hex());
    assert_eq!(short.len(), 19); // XXXX-XXXX-XXXX-XXXX
    assert_eq!(short.matches('-').count(), 3);
}

#[test]
fn normalize_fingerprint_cleans_separators_and_rejects_invalid() {
    let raw = "A4:F2:9C:10:7B:3E:81:D4:00:11:22:33:44:55:66:77:88:99:AA:BB:CC:DD:EE:FF:01:23:45:67:89:AB:CD:EF";
    let norm = normalize_fingerprint(raw).unwrap();
    assert_eq!(
        norm,
        "a4f29c107b3e81d400112233445566778899aabbccddeeff0123456789abcdef"
    );

    let dashed = "A4F2-9C10-7B3E-81D4-0011-2233-4455-6677-8899-AABB-CCDD-EEFF-0123-4567-89AB-CDEF";
    assert_eq!(normalize_fingerprint(dashed).unwrap(), norm);

    assert!(matches!(
        normalize_fingerprint("abcd"),
        Err(ProtoError::InvalidFingerprint(_))
    ));
    assert!(matches!(
        normalize_fingerprint(&"z".repeat(64)),
        Err(ProtoError::InvalidFingerprint(_))
    ));
}

#[test]
fn known_hosts_tofu_lifecycle_and_atomic_save() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("known_hosts.json");
    let fp1 = "a".repeat(64);
    let fp2 = "b".repeat(64);

    let mut kh = KnownHosts::load(&path).unwrap();
    assert_eq!(
        kh.check("192.168.1.50:37420", &fp1),
        TofuDecision::FirstUse(fp1.clone())
    );

    kh.pin("192.168.1.50:37420", &fp1).unwrap();
    kh.save_atomic(&path).unwrap();

    let reloaded = KnownHosts::load(&path).unwrap();
    assert_eq!(
        reloaded.check("192.168.1.50:37420", &fp1),
        TofuDecision::Trusted
    );
    assert_eq!(
        reloaded.check("192.168.1.50:37420", &fp2),
        TofuDecision::Mismatch {
            expected: fp1,
            actual: fp2,
        }
    );
}
