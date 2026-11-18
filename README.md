# Portico

**Share the USB devices plugged into your phone with computers on your network — and let machines borrow each other's USB devices too.**

Plug a pendrive into your Android phone, and it shows up as a local disk on your Linux desktop over Wi-Fi. No cloud, no cable, no app on the receiving machine beyond one small CLI. Portico is a from-scratch, open-source USB-over-network tool: an Android app acts as the server, and lightweight desktop clients attach to it using the standard Linux USB/IP stack.

## How it works

- The **Android app** runs a userspace USB/IP server (libusb-based — no root needed). Devices plugged into the phone are shared over TCP once you grant permission, shown in a Material 3 GUI with a claim token and fingerprint badge.
- The **desktop client** (`portico` CLI, Linux) discovers the phone, verifies its TLS certificate on first contact (TOFU pinning), and uses a per-endpoint **claim token** shown in the app to attach devices. On attach, the kernel's `vhci-hcd` module mounts the remote device as if it were plugged in locally — the OS then treats it like any USB disk.

Traffic runs on your LAN over TLS; discovery uses a UDP broadcast (TCP `37420`, UDP `37421` by default).

## Features

- **Auto-share on plug-in** — every attached device is shared by default; ignore-list keeps exclusions persistent (`vid/pid` based)
- **Per-device permission grants** — remembered across reboots
- **Start on boot**, keep-network-awake and keep-device-awake toggles
- **NAT relay/discovery service** for phones behind a different network segment
- **Foreground/background attach** on desktop; `status`/`stop` to manage detached sessions
- Full feature checklist in [FEATURES.md](FEATURES.md); architecture decisions in [DESIGN.md](DESIGN.md)

## Requirements

- **Server:** an Android phone running the Portico app
- **Client:** Linux with the `vhci-hcd` kernel module (macOS/Windows clients are future work — see roadmap in [CLONE-PROMPT.md](CLONE-PROMPT.md))

## Quick start — phone side

1. Install the app and open it.
2. Allow the permission prompt for each device you plug in (remembered afterwards).
3. Note the **claim token** and **fingerprint badge** shown in the app.

## Quick start — desktop (Linux)

Install the `portico` binary (e.g. `cargo build --release`, then copy `target/release/portico` to a PATH directory).

**One-time setup — passwordless sudo for USB/IP.** `usbip attach`/`detach` write to `/sys/devices/platform/vhci_hcd.*/`, which needs root. For background mode, install a NOPASSWD rule once:

```
sudo visudo -f /etc/sudoers.d/portico
```

```
<your-username> ALL=(root) NOPASSWD: /usr/bin/usbip *
```

Load the kernel module once per boot:

```
sudo modprobe vhci-hcd
echo vhci_hcd | sudo tee /etc/modules-load.d/portico.conf   # persist across reboots
```

**Attach a device:**

```
portico discover --target <phone-ip>:37421     # optional — find the TCP port
portico pair <phone-ip>                        # TOFU: compare badge, answer y
portico list <phone-ip>:37420 --token <your-claim-token>
portico attach <phone-ip>:37420 <bus-id> --background
portico status                                 # active background attachments + mount paths
portico stop <bus-id>                          # detach one device
```

Notes:

- The **bus ID** is the first column of `portico list` — it changes whenever the disk is replugged or the phone reboots, so copy it fresh. `in_use` means another client holds the device.
- After attach, `udisksd` auto-mounts the disk under `/run/media/<user>/<label>`; the CLI prints the mount path.
- Fingerprints are case-insensitive dash-stripped hex; claim tokens are uppercase Crockford base32 — copy-paste, never retype.
- The store lives in `~/.config/portico/` (`known_hosts.json` for TOFU pins, `tokens.json` for claim tokens); background session records live in `$XDG_RUNTIME_DIR/portico/` (tmpfs).

## Security

TLS with trust-on-first-use certificate pinning, per-endpoint claim tokens, and per-device grants on the phone side. Regenerate a claim token from the app at any time to invalidate the old one.

## Project docs

- [FEATURES.md](FEATURES.md) — feature specification
- [DESIGN.md](DESIGN.md) — design notes
- [PROGRESS.md](PROGRESS.md) — development log
- [CLONE-PROMPT.md](CLONE-PROMPT.md) — the clean-room build briefing used during development (archival)

## Status

v1 scope: Android server + Linux desktop CLI. Windows (usbipd-style) and macOS (DriverKit) clients and a desktop GUI are future directions.
