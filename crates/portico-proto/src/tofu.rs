use crate::identity::{normalize_fingerprint, write_atomic_secure};
use crate::ProtoError;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TofuDecision {
    Trusted,
    FirstUse(String),
    Mismatch { expected: String, actual: String },
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct KnownHosts {
    pub entries: BTreeMap<String, String>,
}

impl KnownHosts {
    pub fn load(path: &Path) -> Result<Self, ProtoError> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let data = fs::read(path)?;
        serde_json::from_slice(&data)
            .map_err(|e| ProtoError::MalformedPayload(format!("known_hosts json: {e}")))
    }

    pub fn save_atomic(&self, path: &Path) -> Result<(), ProtoError> {
        let json = serde_json::to_vec_pretty(self)
            .map_err(|e| ProtoError::MalformedPayload(format!("known_hosts serialize: {e}")))?;
        write_atomic_secure(path, &json)
    }

    pub fn check(&self, endpoint: &str, actual_fp_hex: &str) -> TofuDecision {
        let actual = normalize_fingerprint(actual_fp_hex).unwrap_or_else(|_| actual_fp_hex.to_lowercase());
        match self.entries.get(endpoint) {
            Some(raw_expected) => {
                let expected = normalize_fingerprint(raw_expected)
                    .unwrap_or_else(|_| raw_expected.to_lowercase());
                if expected == actual {
                    TofuDecision::Trusted
                } else {
                    TofuDecision::Mismatch { expected, actual }
                }
            }
            None => TofuDecision::FirstUse(actual),
        }
    }

    pub fn pin(&mut self, endpoint: &str, fp_hex: &str) -> Result<(), ProtoError> {
        let norm = normalize_fingerprint(fp_hex)?;
        self.entries.insert(endpoint.to_string(), norm);
        Ok(())
    }
}
