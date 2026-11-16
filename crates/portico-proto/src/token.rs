use rand::RngCore;
use subtle::ConstantTimeEq;

const TOKEN_ALPHABET: &[u8; 32] = b"23456789ABCDEFGHJKLMNPQRSTUVWXYZ";
const TOKEN_LEN: usize = 24;

pub fn generate_claim_token() -> String {
    let mut bytes = [0u8; TOKEN_LEN];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    bytes
        .iter()
        .map(|b| TOKEN_ALPHABET[(b & 0x1F) as usize] as char)
        .collect()
}

pub fn verify_claim_token(expected: &str, presented: &[u8]) -> bool {
    if expected.is_empty() || presented.is_empty() {
        return false;
    }
    let exp = expected.as_bytes();
    if exp.len() != presented.len() {
        return false;
    }
    bool::from(exp.ct_eq(presented))
}

pub fn redact_token(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= 6 {
        return "****".to_string();
    }
    let prefix: String = chars[..2].iter().collect();
    let suffix: String = chars[chars.len() - 2..].iter().collect();
    format!("{prefix}****{suffix}")
}
