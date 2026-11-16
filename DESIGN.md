# Portico — Architecture & Design Document

**Status:** Draft v0.1 (2026-11-16)  
**Spec authority:** `README.md`, `FEATURES.md`  
**Clean-room boundary:** Zero code copied from GPL/unlicensed references in `README.md` appendix. Core uses MIT `usbip` (`jiegec/usbip`) and MIT/Apache Rust ecosystem crates (`rusb`, `tokio`, `rustls`, `rcgen`).

---

## 1. Intent, Goals, Non-Goals, Constraints

### Intent
Portico lets an Android phone (without root) or a Linux host share locally attached USB devices over a network so desktop clients can list, claim, and use them concurrently as if plugged in locally.

### Goals
- **Security by default:** TLS 1.3 everywhere by default; offline TOFU certificate fingerprint pinning; protocol-level claim token inside the versioned handshake; mandatory mutual TLS for reverse clients; end-to-end encrypted NAT relay (relay sees ciphertext only).
- **Clean-room & brand-new UX:** Fresh product name (**Portico**), fresh Material 3 UI, clear non-technical wording, custom TCP port (`37420`), custom versioned wire protocol (`v0`).
- **Root-free Android USB access:** Kotlin `UsbManager.requestPermission` → `UsbDeviceConnection.getFileDescriptor()` → JNI fd pass → `rusb::disable_device_discovery()` + `Context::open_device_with_fd(fd)` → `usbip::UsbIpServer`.
- **Desktop host mode (`portico serve`) & testability:** Same `portico-core` engine supports Linux host enumeration via `rusb::Context` (generic `UsbContext` handler) so M1 can be verified end-to-end on Linux as well as via JNI fd-injection on Android.
- **Concurrent multi-device sharing:** Multiple devices shared and used simultaneously with no seat or count limits.
- **Extensibility hook:** Internal `LicenseTier::Free` capability trait/config slot so a future paid tier can be added without protocol or architecture rework.

### Non-Goals (v1)
- Compatibility mode with legacy unencrypted USB/IP wire protocol on port 3240.
- Multi-user accounts / passwords / Play Billing / license keys.
- macOS DriverKit client driver.
- Cross-compilation matrix beyond the local Linux x86_64 host and `OnePlus 12R` Android arm64-v8a target.

### Anti-Goals
- Never send USB traffic or claim tokens in plaintext over non-LAN routes.
- Never trust a dialed reverse-client endpoint by IP/hostname alone (mTLS + pinned fingerprint required).
- Never copy GPL code from reference repositories.

---

## 2. Approaches Considered

### Daemon ↔ Android GUI Integration
1. **Approach A (Selected): In-process Rust `cdylib` (`portico-jni`) inside Android `ForegroundService` + Unix-socket line-based control channel**
   - `PorticoService` loads `libportico_jni.so`, starts the Rust Tokio runtime on a dedicated daemon thread, and binds a private Unix domain socket (`<filesDir>/portico-ctrl.sock`, mode `0600`) speaking the line-based text control protocol.
   - Kotlin `UsbManager` grants permission, holds `UsbDeviceConnection` open, and hands the raw `fd` + device metadata to Rust via JNI (`nativeAttachUsbFd`).
   - **Tradeoffs:** Avoids Android 10+ W^X `execve` restrictions on running binaries from app data, avoids `SCM_RIGHTS` fd-passing complexity across processes, and keeps the documented text control channel clean and testable on both Linux and Android. A daemon crash shares the service process (mitigated by `START_STICKY` and panic-catching at the FFI boundary).
2. **Approach B: Standalone Rust binary spawned as subprocess + `SCM_RIGHTS`**
   - **Rejected:** Requires packaging an executable disguised as `lib*.so`, passing `ParcelFileDescriptor` over `LocalSocket` ancillary data, and synchronizing `UsbDeviceConnection` lifetime across two OS processes.
3. **Approach C: Pure Kotlin USB/IP implementation**
   - **Rejected:** Violates decided Rust core stack and prevents reusing the core engine on desktop.

### Desktop Client ↔ Linux Kernel `vhci-hcd` Integration
1. **Approach A (Selected): `portico` CLI Localhost Loopback Bridge**
   - `portico attach <target> <bus_id>` connects to the Portico server over TLS 1.3 (verifying TOFU fingerprint), completes the Portico v0 handshake with the claim token, opens an authenticated USB/IP tunnel (`OpenUsbIpTunnel`), binds an ephemeral `127.0.0.1:<local_port>` TCP socket, invokes the system's `usbip --tcp-port <local_port> attach -r 127.0.0.1 -b <bus_id>` tool, and bidirectionally pumps bytes between `vhci-hcd`'s local socket and the remote TLS tunnel.
   - **Tradeoffs:** Zero GPL code in our repo, works with standard upstream Linux `vhci-hcd`, and keeps all network traffic TLS-encrypted and token-authenticated.

---

## 3. Product Naming, UI Look & Wording

### Brand & Palette
- **Name:** Portico
- **Tagline:** *"Share USB devices across your network, privately."*
- **Android Application ID:** `io.portico.app`
- **Design System (Material 3):**
  - Primary: Deep Slate Blue (`#1E3A5F`), Accent: Electric Cobalt (`#2563EB`), Secondary: Teal (`#0D9488`), Warning: Amber (`#D97706`), Danger: Crimson (`#DC2626`).
  - Typography: Sans-serif UI labels (`MaterialTheme.typography`), monospace tabular numerals (`FontFamily.Monospace`) for `VID/PID`, `bus_id`, ports, tokens, and certificate fingerprints.

### Main Screen Layout & Copy
1. **Top App Bar:** `Portico` · Status chip (`Sharing` / `Paused` / `Starting`) · Start/Stop toggle button · Overflow menu (`⋮`).
2. **Primary Status Card:**
   - Active copy: *"USB devices plugged into this phone are shared over your network once you grant USB access."*
   - Paused copy: *"Sharing is paused. Start the service to share attached USB devices."*
   - Action link: `"Desktop client setup"` (opens sheet showing `portico list <ip>:37420`, claim token copy, and fingerprint copy, plus link to project docs).
3. **Security & Connection Card:**
   - Endpoint: `<lan-ip>:37420`
   - Mode badge: `TLS 1.3 · TOFU Pinned` (green/teal) OR prominent warning banner when LAN plaintext opt-in is active: `"Plaintext LAN Mode Active — USB traffic (keystrokes, storage, serial) is unencrypted. Use on trusted private LANs only."`
   - Server Fingerprint: `A4F2-9C10-7B3E-81D4` (short 16-hex preview + expandable full SHA-256 + Copy button).
   - Claim Token: masked (`••••-••••-••••`) with Reveal/Hide toggle, Copy, and Regenerate button.
4. **Relay Status Row** (always reflects live state):
   - `"Currently disabled"` | `"Please wait, initializing…"` | `"Available — enter <relay-addr> in any client to connect!"`
5. **Attached USB Devices List:**
   - Empty state: *"No USB devices attached. Plug in a device via USB-C or an OTG adapter."*
   - Row: Device product name, `VID/PID` (`0781/5583`), bus ID (`1-2`), status pill (`Shared`, `In Use`, `Permission Required`, `Ignored`), and row action (`Grant Access` / `Ignore` / `Release`).

### Overflow Menu (11 Items — Feature Parity)
1. **System Log** — Live scrollable log stream from daemon, level filter (`INFO`/`WARN`/`ERROR`), Copy All / Clear.
2. **Start on Boot** — Switch (persisted; enables/disables `BootReceiver`).
3. **Keep Network Awake** — Switch (persisted; acquires `WifiManager.WifiLock`).
4. **Keep Device Awake** — Switch (persisted; acquires `PowerManager.PARTIAL_WAKE_LOCK`).
5. **NAT Relay Service** — Dialog/screen with enable toggle, relay server address field (default configurable), and live status (`"currently disabled"` / `"please wait, initializing"` / `"available, put the address above into any client to connect!"`).
6. **Ignored Devices…** — Persistent `VID/PID` list (`0123/AE3C`), validated hex input (`VVVV/PPPP`), `"Select from attached"` picker displaying `<product name>(<vvpp>)` (e.g. `SanDisk Ultra(0781/5583)`), and checkbox multi-select remove with validation toast if none selected or invalid input.
7. **Reverse Clients…** — Persistent list of outbound mTLS targets (`host:port` + required peer cert SHA-256 fingerprint), validated input, checkbox remove flow with validation toasts, enable/disable per entry.
8. **TLS & Security…** — TLS enforcement info, `"Allow plaintext on LAN"` opt-in switch (off by default, shows explicit risk warning dialog on enable), Server Fingerprint view + Rotate Identity Keypair, Claim Token view + Regenerate.
9. **Help** — Opens project documentation / client quickstart URL.
10. **About** — Description, version (`0.1.0`), license & open-source notices (`usbip` MIT, `rusb` MIT, `rustls` Apache-2.0/MIT).
11. **Exit** — Sends `SHUTDOWN` to daemon, removes foreground notification, finishes task.

---

## 4. Workspace & Crate Architecture

```
Portico/
├── Cargo.toml                  # Workspace: portico-proto, portico-core, portico-cli, portico-jni, portico-relay
├── crates/
│   ├── portico-proto/          # Wire protocol v0, framing, TLS/TOFU/mTLS helpers, claim token, relay framing
│   ├── portico-core/           # Daemon engine, config store, log ring buffer, control-channel server, USB registry + usbip integration
│   ├── portico-cli/            # `portico` CLI client + `porticod` / `portico relay` subcommands
│   ├── portico-jni/            # Android `cdylib` (`libportico_jni.so`) JNI bridge
│   └── portico-relay/          # Standalone ciphertext-only rendezvous/relay binary/lib for NAT traversal & E2E tests
├── android/                    # Gradle Kotlin + Jetpack Compose (Material 3) project (`io.portico.app`)
├── plans/                      # Per-subsystem implementation plans
├── DESIGN.md
└── PROGRESS.md
```

---

## 5. Control-Channel Design (Local Line-Based Text Protocol)

- **Channel Name:** `portico-ctrl`
- **Transport:** Unix domain stream socket at `<data_dir>/portico-ctrl.sock` (`0600` permissions on Linux/Android; on platforms/tests where specified, TCP `127.0.0.1` loopback in test harness only).
- **Encoding:** UTF-8 lines terminated by `\n`. Values that may contain spaces or control characters (product names, log messages, error messages) use URL percent-encoding (`%20`, `%0A`, `%25`).
- **Two connection modes:**
  1. **Command mode (default):** Client sends one request line, server replies synchronously with one response (`OK ...` or `ERR <code> <msg>`) or a multi-line block (`OK <count>\nITEM ...\n...\nEND\n`).
  2. **Event stream mode (`SUBSCRIBE`):** Dedicated socket connection sends `SUBSCRIBE\n`, receives `OK\n`, then receives asynchronous `EVENT <TYPE> <k=v ...>\n` lines until disconnect.

### Control-Channel Command Reference

| Command | Arguments | Response | Notes |
|---|---|---|---|
| `REGISTER_CLIENT` | `<name>` | `OK <client_id>` | Registers GUI/controller instance |
| `UNREGISTER_CLIENT` | `<client_id>` | `OK` | Removes registration |
| `GET_STATE` | — | `OK running=<bool> server_active=<bool> tls=<bool> plaintext_lan=<bool> port=<u16> speed=<mode> net_lock=<bool> wake_lock=<bool> boot=<bool> relay_status=<st> relay_addr=<pct> fp=<hex> active_peers=<n> tier=<free>` | Full daemon snapshot |
| `SET_STATE` | `<key> <val>` | `OK` | Keys: `port`, `plaintext_lan`, `start_on_boot`, `tls_required` |
| `START_SERVER` | — | `OK` | Starts TCP listener + reverse-client loop + relay (if enabled) |
| `STOP_SERVER` | — | `OK` | Stops TCP listener + disconnects active sessions |
| `GET_IGNORED` | — | `OK <n>` / `ITEM <VVVV/PPPP>` / `END` | Lists ignored VID/PID pairs |
| `SET_IGNORED` | `<list\|->` | `OK` | Comma-separated `VVVV/PPPP` pairs (`-` clears); validates 4-hex/4-hex; unshares newly ignored idle devices |
| `GET_REVERSE_CLIENTS` | — | `OK <n>` / `ITEM id=<id> addr=<host:port> fp=<hex> enabled=<bool>` / `END` | Lists reverse-client allowlist |
| `SET_REVERSE_CLIENTS` | `<entries\|->` | `OK` | Comma-separated `<addr>@<sha256_hex>@<true\|false>` (`-` clears); validates non-empty fingerprint |
| `SET_RELAY` | `<true\|false> [endpoint]` | `OK` | Persists & starts/stops relay client |
| `GET_RELAY` | — | `OK enabled=<bool> status=<disabled\|initializing\|ready\|error> address=<pct> endpoint=<pct>` | Status label state |
| `SET_NET_LOCK` | `<true\|false>` | `OK` | Updates desired state + emits `EVENT LOCK_CHANGED` for Android `WifiLock` |
| `SET_WAKE_LOCK` | `<true\|false>` | `OK` | Updates desired state + emits `EVENT LOCK_CHANGED` for Android `WakeLock` |
| `SET_SPEED` | `<auto\|low\|full\|high\|super>` | `OK` | USB speed cap/hint persisted & applied |
| `LIST_DEVICES` | — | `OK <n>` / `ITEM bus_id=<id> vid=<hhhh> pid=<hhhh> product=<pct> mfr=<pct> serial=<pct> state=<st> peer=<pct>` / `END` | `state`: `shared`, `in_use`, `ignored`, `pending_perm` |
| `DETACH_DEVICE` | `<bus_id>` | `OK` | Force-unregisters a device (e.g. on USB unplug) |
| `GET_TOKEN` | — | `OK <token>` | Returns current claim token |
| `REGEN_TOKEN` | — | `OK <new_token>` | Rotates claim token and persists |
| `GET_LOGS` | `[since_seq]` | `OK <n>` / `ITEM seq=<u64> ts=<u64> level=<lvl> target=<pct> msg=<pct>` / `END` | Reads ring buffer |
| `SUBSCRIBE` | — | `OK` + stream `EVENT ...` | Events: `DEVICE_ADDED`, `DEVICE_REMOVED`, `DEVICE_STATE`, `SERVER_STATE`, `RELAY_STATE`, `LOCK_CHANGED`, `LOG` |
| `SHUTDOWN` | — | `OK` | Graceful shutdown of daemon & socket |

---

## 6. Wire Protocol v0 Definition (`PRTC` v0)

- **Default TCP Port:** `37420`
- **Transport Security:**
  - **TLS 1.3** (`rustls`) is mandatory by default.
  - **Plaintext LAN opt-in:** Allowed ONLY if server has `plaintext_lan == true`, client explicitly opts in, AND the TCP peer IP is loopback or private/link-local (`127.0.0.0/8`, `10.0.0.0/8`, `172.16.0.0/12`, `192.168.0.0/16`, `169.254.0.0/16`, `::1`, `fc00::/7`, `fe80::/10`). Public/routable IPs are **never** served or connected over plaintext.
  - **TOFU + Pinning:** Server auto-generates an ECDSA P-256 self-signed X.509 cert (`CN=portico-node`). Fingerprint = lowercase hex `SHA-256(leaf_cert_der)`. Client checks `known_hosts.json`; unknown certs prompt/require explicit `--trust <fp>` or interactive confirmation; mismatched pins abort immediately before sending `ClientHello`.
  - **Reverse Clients (mTLS):**
    - Desktop runs `portico listen --bind 0.0.0.0:37425` (plus `portico identity` to display its listener cert SHA-256 fingerprint for pasting into the phone's Reverse Clients list).
    - Server dials each enabled reverse client `host:port` with exponential backoff (`1s`..`30s`) as TLS client presenting its node cert, and verifies the listener's cert SHA-256 matches the pinned `cert_sha256` in `reverse_clients` (mandatory mTLS). The listener verifies the server's cert SHA-256 against its TOFU/allowlist store.
    - Over this persistent mTLS control stream, the listener sends `ClientHello` (with claim token), `ListDevicesReq`, `Ping`, and for each device attach sends `RequestReverseDataConn { bus_id, conn_nonce }`.
    - Upon `RequestReverseDataConn`, the server opens a fresh outbound mTLS connection to the same `host:port`, sends `ReverseDataHello { conn_nonce }` (inside mTLS), receives `OpenTunnelResp(Ok)`, and immediately transitions that data stream to `usbip::handler`. This keeps the control channel alive and allows **2+ devices to be attached concurrently** over reverse connections.

### Binary Frame Format (Big-Endian)
Used for control/handshake phase before switching to raw USB/IP stream:
```
+-------------------+---------+----------+--------------------+----------------------+
| Magic "PRTC" (4B) | Ver (1B)| Type (1B)| Payload Len (4B)   | Payload (N bytes)    |
+-------------------+---------+----------+--------------------+----------------------+
```
- `Ver`: `0x00` (v0). Max `Payload Len`: `4_194_304` (4 MiB).
- Strings: `u16` byte length + UTF-8 bytes.

### Message Types
| Type | Name | Payload Layout |
|---|---|---|
| `0x01` | `ClientHello` | `min_ver: u8`, `max_ver: u8`, `auth_kind: u8` (`1=ClaimToken`), `credentials: [u8]` (`u16` len + bytes), `client_name: str`, `capabilities: u32` (`bit 0 = USBIP_V1`) |
| `0x02` | `ServerHello` | `chosen_ver: u8`, `status: u8` (`0=Ok`, `1=AuthFailed`, `2=UnsupportedVersion`, `3=PolicyRejected`), `server_name: str`, `capabilities: u32`, `tier: u8` (`0=Free`), `message: str` |
| `0x10` | `ListDevicesReq` | *(empty)* |
| `0x11` | `ListDevicesResp` | `count: u16`, repeated `DeviceSummary`: `bus_id: str`, `vid: u16`, `pid: u16`, `device_class: u8`, `device_subclass: u8`, `device_protocol: u8`, `speed: u32`, `manufacturer: str`, `product: str`, `serial: str`, `in_use: u8` |
| `0x20` | `OpenTunnelReq` | `bus_id: str` (non-empty: claims specific shared device; server checks not ignored & available) |
| `0x21` | `OpenTunnelResp` | `status: u8` (`0=Ok`, `1=NotFound`, `2=InUse`, `3=Ignored`), `message: str`. **On `Ok`, the connection immediately transitions to raw USB/IP PDUs (`jiegec/usbip::handler`).** |
| `0x22` | `RequestReverseDataConn` | `conn_nonce: [u8; 16]`, `bus_id: str` (sent by reverse listener over mTLS control stream to request a dedicated data mTLS connection for `bus_id`) |
| `0x23` | `ReverseDataHello` | `conn_nonce: [u8; 16]` (sent by server as first frame on the newly opened outbound mTLS data connection, followed by `OpenTunnelResp` and raw USB/IP PDUs) |
| `0x30` | `Ping` | `nonce: u64` |
| `0x31` | `Pong` | `nonce: u64` |
| `0x7F` | `Error` | `code: u8`, `message: str` |

*Auth note:* `credentials` is an opaque byte string keyed by `auth_kind`, so v2 per-user accounts can introduce `auth_kind = 2` without altering frame headers or handshake flow.

---

## 7. NAT Relay (`PRLY` v0) & LAN UDP Discovery (`PRDS` v0)

### NAT Relay (`PRLY` v0 — Ciphertext-Only)
- **Default Relay Port:** `37422`
- **Trust Model:** The relay server is untrusted for USB contents, descriptors, tokens, and certificates. It sees only `node_id` (16 bytes) and connection nonces, then pipes raw TLS 1.3 ciphertext via `copy_bidirectional`.
- **Node ID & Address Format:**
  - `node_id` (16 bytes) = first 16 bytes of `SHA-256(b"portico-node-v0" || identity_cert_der)`, so the relay address stays stable across restarts and rotates only if the server identity certificate is rotated.
  - `slot_secret` (32 random bytes) is generated once and persisted in `config.json` (`relay.slot_secret_hex`).
  - Shareable relay URI displayed in UI and accepted by `portico`: `prly://<relay_host>:<relay_port>/<node_id_32hex>`.
  - No hardcoded third-party relay server is baked in: toggling Relay ON requires a configured `relay.endpoint` (`host:port`, e.g. self-hosted `portico relay --listen 0.0.0.0:37422`).
- **Flow:**
  1. Server connects to relay `<host:port>` and sends `PRLY\x00\x01` + `node_id(16B)` + `SHA-256(slot_secret)(32B)` (`ControlRegister`). Relay registers the slot and replies `0x00` (`Registered`).
  2. When a desktop client targets `prly://<relay_host>:<relay_port>/<node_id_hex>`, client opens TCP to `<relay_host>:<relay_port>` and sends `PRLY\x00\x02` + `node_id(16B)` + `conn_nonce(16B)` (`ClientConnect`).
  3. Relay forwards `0x03` + `conn_nonce(16B)` (`IncomingClient`) down the server's relay control stream.
  4. Server opens a fresh TCP connection to relay sending `PRLY\x00\x04` + `node_id(16B)` + `SHA-256(slot_secret)(32B)` + `conn_nonce(16B)` (`ServerAccept`), then immediately begins a **TLS 1.3 server handshake** on that connection while Relay bridges `client_tcp <-> server_data_tcp`.
  5. Client performs normal **TLS 1.3 + TOFU fingerprint verification + Portico v0 `ClientHello`** end-to-end through the relay pipe.

### LAN UDP Discovery (`PRDS` v0)
- **UDP Port:** `37421`
- **Discovery Request (`portico discover`):** Client broadcasts/multicasts `PRDS\x00\x01` + `nonce(8B)` to `255.255.255.255:37421` (and/or `224.0.0.251:37421` / subnet broadcast).
- **Discovery Response:** Running Portico server replies unicast with `PRDS\x00\x02` + `nonce(8B)` + `tcp_port(u16 BE)` + `flags(u8: bit0=tls_required, bit1=plaintext_lan)` + `cert_fp_prefix(8B)` + `node_name(u8 len + UTF-8)`. Zero secrets (no claim token, no device list) are broadcast over UDP.

---

## 8. Storage, Logging, & Security Defaults

### Persistent Storage (`<data_dir>/config.json`)
Written atomically (`config.json.tmp` → `fsync` → `rename`), mode `0600` on Unix:
- `port`: `37420`
- `tls_required`: `true`
- `plaintext_lan`: `false`
- `claim_token`: 24-char cryptographically random Crockford/alphanumeric secret generated on first boot
- `start_on_boot`: `false`
- `keep_network_awake`: `false`
- `keep_device_awake`: `false`
- `usb_speed`: `"auto"`
- `ignored_devices`: `[]` (`"VVVV/PPPP"`)
- `reverse_clients`: `[]` (`{ id, addr, cert_sha256, enabled }`)
- `relay`: `{ enabled: false, endpoint: "", slot_secret_hex: "<64-hex random>" }`
- `tier`: `"free"` (licensing hook)

### Identity & Trust Material
- Server keypair + X.509 cert: `<data_dir>/identity_cert.der`, `<data_dir>/identity_key.der` (mode `0600`).
- Desktop client trust store: `~/.config/portico/known_hosts.json` (`host_or_node_id -> sha256_hex`) and client mTLS identity (`client_cert.der`, `client_key.der`) for reverse-client mode.

### Logging
- Bounded in-memory ring buffer (`capacity = 1000` entries) with monotonic `seq: u64`, `timestamp_ms: u64`, `level`, `target`, `message`.
- Redaction rule: `claim_token` and private key bytes are **never** written to logs; auth failures log peer IP and reason only.
- On Android, logs also mirror to `logcat` (`PorticoCore` tag).

### Security Defaults Summary
- TLS 1.3 ON by default; plaintext disabled by default and restricted to RFC1918/link-local/loopback even when toggled on.
- Claim token auto-generated on first launch, verified via constant-time comparison (`subtle` / constant-time slice eq) inside TLS `ClientHello`.
- Reverse clients require mTLS + pinned SHA-256 fingerprint on both sides.
- Relay carries TLS 1.3 ciphertext only.

---

## 9. Left to the Builder
- Exact Rust crate versions compatible with `Cargo.lock` and offline/online resolution (`rusb` `vendored` vs system libusb).
- Exact Jetpack Compose screen layout details and Android resource XML/drawables within the Material 3 palette defined above.
- Linux desktop `usbip` invocation details (`usbip --tcp-port <port> attach -r 127.0.0.1 -b <bus_id>` and privilege handling/messaging when `vhci-hcd` requires `CAP_NET_ADMIN`/root).
