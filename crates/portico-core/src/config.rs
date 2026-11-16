use crate::CoreError;
use portico_proto::identity::{normalize_fingerprint, write_atomic_secure};
use portico_proto::token::generate_claim_token;
use portico_proto::DEFAULT_PORT;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct VidPid {
    pub vid: u16,
    pub pid: u16,
}

impl VidPid {
    pub fn parse(s: &str) -> Result<Self, CoreError> {
        let trimmed = s.trim();
        let (v_str, p_str) = if let Some((v, p)) = trimmed.split_once('/') {
            (v, p)
        } else if let Some((v, p)) = trimmed.split_once(':') {
            (v, p)
        } else if trimmed.len() == 8 && trimmed.chars().all(|c| c.is_ascii_hexdigit()) {
            (&trimmed[..4], &trimmed[4..])
        } else {
            return Err(CoreError::InvalidInput(format!(
                "invalid VID/PID '{s}' (expected VVVV/PPPP)"
            )));
        };
        if v_str.len() != 4 || p_str.len() != 4 {
            return Err(CoreError::InvalidInput(format!(
                "invalid VID/PID '{s}' (expected 4 hex digits each)"
            )));
        }
        let vid = u16::from_str_radix(v_str, 16)
            .map_err(|_| CoreError::InvalidInput(format!("invalid VID hex '{v_str}'")))?;
        let pid = u16::from_str_radix(p_str, 16)
            .map_err(|_| CoreError::InvalidInput(format!("invalid PID hex '{p_str}'")))?;
        Ok(Self { vid, pid })
    }
}

impl fmt::Display for VidPid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:04X}/{:04X}", self.vid, self.pid)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReverseClientEntry {
    pub id: String,
    pub addr: String,
    pub cert_sha256: String,
    pub enabled: bool,
}

impl ReverseClientEntry {
    pub fn validate(&self) -> Result<Self, CoreError> {
        let addr = self.addr.trim();
        let Some((host, port_str)) = addr.rsplit_once(':') else {
            return Err(CoreError::InvalidInput(format!(
                "reverse client addr '{addr}' missing :port"
            )));
        };
        if host.is_empty() || port_str.parse::<u16>().is_err() {
            return Err(CoreError::InvalidInput(format!(
                "invalid reverse client addr '{addr}'"
            )));
        }
        let fp = normalize_fingerprint(&self.cert_sha256)?;
        Ok(Self {
            id: if self.id.trim().is_empty() {
                addr.to_string()
            } else {
                self.id.trim().to_string()
            },
            addr: addr.to_string(),
            cert_sha256: fp,
            enabled: self.enabled,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UsbSpeedSetting {
    #[default]
    Auto,
    Low,
    Full,
    High,
    Super,
}

impl UsbSpeedSetting {
    pub fn parse(s: &str) -> Result<Self, CoreError> {
        match s.trim().to_ascii_lowercase().as_str() {
            "auto" => Ok(Self::Auto),
            "low" => Ok(Self::Low),
            "full" => Ok(Self::Full),
            "high" => Ok(Self::High),
            "super" => Ok(Self::Super),
            other => Err(CoreError::InvalidInput(format!(
                "invalid usb speed '{other}'"
            ))),
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Low => "low",
            Self::Full => "full",
            Self::High => "high",
            Self::Super => "super",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LicenseTier {
    #[default]
    Free,
}

impl LicenseTier {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Free => "free",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct RelayConfig {
    pub enabled: bool,
    pub endpoint: String,
    pub slot_secret_hex: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DaemonConfig {
    pub port: u16,
    pub tls_required: bool,
    pub plaintext_lan: bool,
    pub claim_token: String,
    pub start_on_boot: bool,
    pub keep_network_awake: bool,
    pub keep_device_awake: bool,
    pub usb_speed: UsbSpeedSetting,
    pub ignored_devices: Vec<VidPid>,
    pub reverse_clients: Vec<ReverseClientEntry>,
    pub relay: RelayConfig,
    pub tier: LicenseTier,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        let mut slot_secret = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut slot_secret);
        Self {
            port: DEFAULT_PORT,
            tls_required: true,
            plaintext_lan: false,
            claim_token: generate_claim_token(),
            start_on_boot: false,
            keep_network_awake: false,
            keep_device_awake: false,
            usb_speed: UsbSpeedSetting::Auto,
            ignored_devices: Vec::new(),
            reverse_clients: Vec::new(),
            relay: RelayConfig {
                enabled: false,
                endpoint: String::new(),
                slot_secret_hex: hex::encode(slot_secret),
            },
            tier: LicenseTier::Free,
        }
    }
}

pub struct ConfigStore {
    pub path: PathBuf,
    pub config: DaemonConfig,
}

impl ConfigStore {
    pub fn load_or_init(data_dir: &Path) -> Result<Self, CoreError> {
        fs::create_dir_all(data_dir)?;
        let path = data_dir.join("config.json");
        if path.exists() {
            let bytes = fs::read(&path)?;
            let mut config: DaemonConfig = serde_json::from_slice(&bytes)
                .map_err(|e| CoreError::InvalidInput(format!("corrupt config.json: {e}")))?;
            let mut modified = false;
            if config.claim_token.is_empty() {
                config.claim_token = generate_claim_token();
                modified = true;
            }
            if config.relay.slot_secret_hex.len() != 64 {
                let mut slot_secret = [0u8; 32];
                rand::rngs::OsRng.fill_bytes(&mut slot_secret);
                config.relay.slot_secret_hex = hex::encode(slot_secret);
                modified = true;
            }
            let store = Self { path, config };
            if modified {
                store.save_atomic()?;
            }
            return Ok(store);
        }

        let store = Self {
            path,
            config: DaemonConfig::default(),
        };
        store.save_atomic()?;
        Ok(store)
    }

    pub fn save_atomic(&self) -> Result<(), CoreError> {
        let json = serde_json::to_vec_pretty(&self.config)
            .map_err(|e| CoreError::InvalidInput(format!("serialize config: {e}")))?;
        write_atomic_secure(&self.path, &json)?;
        Ok(())
    }

    pub fn tier(&self) -> LicenseTier {
        self.config.tier
    }
}
