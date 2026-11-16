use portico_proto::token::{generate_claim_token, redact_token, verify_claim_token};

#[test]
fn generated_token_is_24_chars_alnum_and_unique() {
    let t1 = generate_claim_token();
    let t2 = generate_claim_token();
    assert_eq!(t1.len(), 24);
    assert_eq!(t2.len(), 24);
    assert_ne!(t1, t2);
    assert!(t1.chars().all(|c| c.is_ascii_alphanumeric()));
    assert!(t2.chars().all(|c| c.is_ascii_alphanumeric()));
}

#[test]
fn verify_claim_token_matches_exact_and_rejects_wrong_or_empty() {
    let expected = "ABCD2345EFGH6789JKLMNPQR";
    assert!(verify_claim_token(expected, expected.as_bytes()));
    assert!(!verify_claim_token(expected, b"ABCD2345EFGH6789JKLMNPQS"));
    assert!(!verify_claim_token(expected, b"ABCD2345"));
    assert!(!verify_claim_token(expected, b"ABCD2345EFGH6789JKLMNPQRX"));
    assert!(!verify_claim_token(expected, b""));
    assert!(!verify_claim_token("", b""));
}

#[test]
fn redact_token_masks_secret() {
    let token = "ABCD2345EFGH6789JKLMNPQR";
    let redacted = redact_token(token);
    assert!(!redacted.contains("EFGH"));
    assert!(!redacted.contains("6789"));
    assert!(redacted.contains("****"));
}
