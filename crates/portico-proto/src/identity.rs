use crate::ProtoError;
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Write;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeIdentity {
    pub cert_der: Vec<u8>,
    pub key_der: Vec<u8>,
}

impl NodeIdentity {
    pub fn generate(common_name: &str) -> Result<Self, ProtoError> {
        let mut params = rcgen::CertificateParams::new(vec![
            common_name.to_string(),
            "localhost".to_string(),
        ])
        .map_err(|e| ProtoError::Tls(format!("cert params: {e}")))?;
        params
            .distinguished_name
            .push(rcgen::DnType::CommonName, common_name);
        let key_pair =
            rcgen::KeyPair::generate().map_err(|e| ProtoError::Tls(format!("key gen: {e}")))?;
        let cert = params
            .self_signed(&key_pair)
            .map_err(|e| ProtoError::Tls(format!("self sign: {e}")))?;
        Ok(Self {
            cert_der: cert.der().to_vec(),
            key_der: key_pair.serialize_der(),
        })
    }

    pub fn load_or_generate(
        dir: &Path,
        prefix: &str,
        common_name: &str,
    ) -> Result<Self, ProtoError> {
        fs::create_dir_all(dir)?;
        let cert_path = dir.join(format!("{prefix}_cert.der"));
        let key_path = dir.join(format!("{prefix}_key.der"));
        if cert_path.exists() && key_path.exists() {
            let cert_der = fs::read(&cert_path)?;
            let key_der = fs::read(&key_path)?;
            if !cert_der.is_empty() && !key_der.is_empty() {
                return Ok(Self { cert_der, key_der });
            }
        }
        let id = Self::generate(common_name)?;
        write_atomic_secure(&cert_path, &id.cert_der)?;
        write_atomic_secure(&key_path, &id.key_der)?;
        Ok(id)
    }

    pub fn fingerprint_hex(&self) -> String {
        cert_fingerprint_hex(&self.cert_der)
    }

    pub fn node_id(&self) -> [u8; 16] {
        let mut hasher = Sha256::new();
        hasher.update(b"portico-node-v0");
        hasher.update(&self.cert_der);
        let digest = hasher.finalize();
        let mut out = [0u8; 16];
        out.copy_from_slice(&digest[..16]);
        out
    }
}

pub fn cert_fingerprint_hex(der: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(der);
    hex::encode(hasher.finalize())
}

pub fn format_short_fingerprint(fp_hex: &str) -> String {
    let upper: String = fp_hex
        .chars()
        .filter(|c| c.is_ascii_hexdigit())
        .take(16)
        .map(|c| c.to_ascii_uppercase())
        .collect();
    upper
        .as_bytes()
        .chunks(4)
        .map(|c| std::str::from_utf8(c).unwrap_or(""))
        .collect::<Vec<_>>()
        .join("-")
}

pub fn normalize_fingerprint(input: &str) -> Result<String, ProtoError> {
    let cleaned: String = input
        .chars()
        .filter(|c| !matches!(c, ':' | '-' | ' ' | '\t' | '\r' | '\n'))
        .map(|c| c.to_ascii_lowercase())
        .collect();
    if cleaned.len() != 64 || !cleaned.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(ProtoError::InvalidFingerprint(input.to_string()));
    }
    Ok(cleaned)
}

pub fn write_atomic_secure(path: &Path, data: &[u8]) -> Result<(), ProtoError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    {
        let mut f = fs::File::create(&tmp)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            f.set_permissions(fs::Permissions::from_mode(0o600))?;
        }
        f.write_all(data)?;
        f.sync_all()?;
    }
    fs::rename(&tmp, path)?;
    Ok(())
}
