//! Background-attach session records and removable-media mount detection.
//!
//! Session records live in `$XDG_RUNTIME_DIR/portico/attachments` (a tmpfs that
//! vanishes on reboot). Each live background attach writes one JSON file keyed
//! by bus ID, so a later `portico attach` can detect an existing session by
//! checking the recorded PID — instead of double-importing (which the phone
//! rejects as `InUse`).

use crate::CliError;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attachment {
    pub bus_id: String,
    pub target: String,
    pub pid: u32,
    pub vhci_port: Option<u8>,
    pub started_at: u64,
    pub log_path: String,
    #[serde(default)]
    pub mount_points: Vec<String>,
}

/// Session directory. `PORTICO_SESSIONS_DIR` overrides for tests; otherwise
/// `$XDG_RUNTIME_DIR/portico/attachments`, falling back to the config dir.
pub fn sessions_dir(config_dir: &Path) -> PathBuf {
    if let Ok(dir) = std::env::var("PORTICO_SESSIONS_DIR") {
        if !dir.is_empty() {
            return PathBuf::from(dir);
        }
    }
    if let Ok(xdg) = std::env::var("XDG_RUNTIME_DIR") {
        if !xdg.is_empty() {
            return PathBuf::from(xdg).join("portico").join("attachments");
        }
    }
    config_dir.join("sessions")
}

fn record_path(sessions: &Path, bus_id: &str) -> PathBuf {
    let safe: String = bus_id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect();
    sessions.join(format!("{safe}.json"))
}

pub fn save_attachment(config_dir: &Path, att: &Attachment) -> Result<(), CliError> {
    let dir = sessions_dir(config_dir);
    fs::create_dir_all(&dir)?;
    let path = record_path(&dir, &att.bus_id);
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, serde_json::to_vec_pretty(att).map_err(|e| CliError::Remote(format!("session serialize: {e}")))?)?;
    fs::rename(&tmp, &path)?;
    Ok(())
}

pub fn load_attachments(config_dir: &Path) -> Vec<Attachment> {
    let dir = sessions_dir(config_dir);
    let Ok(entries) = fs::read_dir(dir) else { return vec![] };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        if entry.path().extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        if let Ok(data) = fs::read(entry.path()) {
            if let Ok(att) = serde_json::from_slice::<Attachment>(&data) {
                out.push(att);
            }
        }
    }
    out.sort_by(|a, b| a.bus_id.cmp(&b.bus_id));
    out
}

pub fn remove_attachment(config_dir: &Path, bus_id: &str) -> Result<(), CliError> {
    let path = record_path(&sessions_dir(config_dir), bus_id);
    if path.exists() {
        fs::remove_file(path)?;
    }
    Ok(())
}

/// POSIX liveness check — signal 0 succeeds if the process exists.
pub fn pid_alive(pid: u32) -> bool {
    // SAFETY: kill(pid, 0) only probes; it never delivers a signal.
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

/// Returns the live attachment for this bus ID (pruning stale records).
pub fn find_active(config_dir: &Path, bus_id: &str) -> Option<Attachment> {
    load_attachments(config_dir)
        .into_iter()
        .find(|a| a.bus_id == bus_id && pid_alive(a.pid))
}

/// Decode mountinfo octal escapes such as `External\0402` -> `External 2`.
pub fn decode_mountinfo_path(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 3 < bytes.len() {
            if let Ok(v) = u8::from_str_radix(&raw[i + 1..i + 4], 8) {
                out.push(v);
                i += 4;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Snapshot removable-media mount points from /proc/self/mountinfo.
pub fn snapshot_mounts() -> HashSet<String> {
    let mut out = HashSet::new();
    let Ok(data) = fs::read_to_string("/proc/self/mountinfo") else { return out };
    for line in data.lines() {
        // fields: ... "mount point" is field 5 (0-indexed 4) after the - separator handling
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 5 { continue; }
        let point = decode_mountinfo_path(fields[4]);
        if point.starts_with("/media/") || point.starts_with("/run/media/") || point.starts_with("/mnt/") {
            out.insert(point);
        }
    }
    out
}

/// Newly appeared mount points (in `after` but not `before`), sorted.
pub fn new_mounts(before: &HashSet<String>, after: &HashSet<String>) -> Vec<String> {
    let mut v: Vec<String> = after.difference(before).cloned().collect();
    v.sort();
    v
}

/// Log file for a background attachment.
pub fn log_path(config_dir: &Path, bus_id: &str) -> PathBuf {
    let dir = if let Ok(xdg) = std::env::var("XDG_RUNTIME_DIR") {
        if !xdg.is_empty() { PathBuf::from(xdg).join("portico").join("logs") } else { config_dir.join("logs") }
    } else {
        config_dir.join("logs")
    };
    let safe: String = bus_id.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
    dir.join(format!("{safe}.log"))
}

/// Parse `usbip port` output: return the vhci port whose block contains `bus_id`.
pub fn parse_vhci_port(output: &str, bus_id: &str) -> Option<u8> {
    let mut current: Option<u8> = None;
    for line in output.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("Port ") {
            let num = rest.split(':').next()?.trim();
            current = num.parse().ok();
        } else if let Some(port) = current {
            if trimmed.contains(bus_id) {
                return Some(port);
            }
        }
    }
    None
}
