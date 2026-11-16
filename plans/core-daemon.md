# Core Daemon (`portico-core`) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement `crates/portico-core`, the Rust USB sharing daemon managing persistent config, structured ring-buffer logging, ignored/reverse-client policy, `rusb` + `usbip` device registry, TLS/LAN/reverse-client/relay/discovery networking, and the line-based `portico-ctrl` Unix-socket control channel.

**Architecture:** `portico-core` wraps `jiegec/usbip`'s `UsbIpServer` and `handler` traits, adding a generic `PorticoRusbDeviceHandler<C: rusb::UsbContext>` / `PorticoRusbInterfaceHandler<C: rusb::UsbContext>` so both Android fd-wrapped handles (`Context::open_device_with_fd`) and Linux host handles work identically. All control-channel mutations go through `DaemonHandle`, which updates atomic config, enforces policy on live devices, and broadcasts `EVENT` notifications to subscribers.

**Tech Stack:** `portico-proto`, `usbip` (0.9.0 MIT), `rusb` (0.9.x MIT, `vendored` feature fallback if system libusb dev absent), `tokio`, `tokio-rustls`, `serde`, `serde_json`, `percent-encoding`.

**Spec:** `DESIGN.md` (§4, §5, §6, §7, §8), `FEATURES.md` (§Daemon feature surface, §Ignored devices, §Reverse clients, §Networking, §Security, §Persistence).

## Global Constraints

- Ignored devices stored/validated as `VVVV/PPPP` hex pairs (case-insensitive input normalized to uppercase `0123/AE3C`). Ignored devices are never auto-shared.
- Control channel uses line-based UTF-8 text (`\n`), percent-encoding arbitrary strings (`product`, `mfr`, `serial`, `msg`), with `SUBSCRIBE` on a dedicated stream so `EVENT` lines never interleave `OK`/`ERR` command replies.
- `START_SERVER` / `STOP_SERVER` must be idempotent.
- Auth failures and config logs must redact `claim_token`.
- Two or more devices must be shareable and usable concurrently.

## Review Focus

1. Invalid `SET_IGNORED` / `SET_REVERSE_CLIENTS` inputs (`ZZZZ/1234`, missing `@`, empty fingerprint, odd hex length) mutating partial state instead of returning `ERR` atomically. (Pinned in Task 1 & Task 6)
2. Unplugging (`DETACH_DEVICE`) or ignoring a device that is currently `in_use` by a USB/IP client — must cleanly handle/report state and not panic `usbip::UsbIpServer`. (Pinned in Task 3 & Task 6)
3. `ClientHello` with wrong claim token or plaintext connection from a non-LAN IP (`8.8.8.8`) — must return `AuthFailed` / `PolicyRejected` and close connection immediately before `ListDevices` or `OpenTunnel`. (Pinned in Task 4)
4. Two concurrent USB/IP clients attaching two different shared devices simultaneously — both transfers must succeed without cross-talk or deadlock. (Pinned in Task 7)
5. Control-channel client sending lines with embedded `%`, spaces, or exceeding line length limit (`64 KiB`) — must percent-decode safely or reject oversized lines without OOM. (Pinned in Task 6)

---

### Task 1: Persistent Config Store, `VidPid` / `ReverseClientEntry` Validation, & Licensing Hook

**Files:**
- Create: `crates/portico-core/Cargo.toml`
- Create: `crates/portico-core/src/lib.rs`
- Create: `crates/portico-core/src/config.rs`
- Test: `crates/portico-core/tests/config_tests.rs`

**Interfaces:**
- Consumes: `portico_proto::token::generate_claim_token`, `portico_proto::identity::normalize_fingerprint`
- Produces:
  - `VidPid { pub vid: u16, pub pid: u16 }` with `VidPid::parse(s: &str) -> Result<Self, CoreError>` (accepts `VVVV/PPPP`, `VVVV:PPPP`, `VVVVPPPP`) and `Display` -> `"{:04X}/{:04X}"`
  - `ReverseClientEntry { pub id: String, pub addr: String, pub cert_sha256: String, pub enabled: bool }`
  - `UsbSpeedSetting::{Auto, Low, Full, High, Super}`
  - `LicenseTier::{Free}` (`ConfigStore::tier(&self) -> LicenseTier`)
  - `DaemonConfig` + `ConfigStore::load_or_init(data_dir: &Path) -> Result<Self, CoreError>` + `save_atomic()`

- [ ] **Step 1: Create `crates/portico-core/Cargo.toml` and `src/lib.rs` stub**
- [ ] **Step 2: Write failing tests in `crates/portico-core/tests/config_tests.rs`**
  Test:
  - First-run `ConfigStore::load_or_init` creates `config.json` with `port == 37420`, `tls_required == true`, `plaintext_lan == false`, non-empty 24-char `claim_token`, non-empty 64-hex `slot_secret_hex`, `tier == Free`.
  - Second `load_or_init` on same dir reloads the exact same `claim_token` and `slot_secret_hex`.
  - `VidPid::parse`: `"0123/ae3c"`, `"0123:AE3C"`, `"0123AE3C"` -> `VidPid { vid: 0x0123, pid: 0xAE3C }`; `"123/AE3C"`, `"ZZZZ/1234"`, `""` -> `Err`.
  - `ReverseClientEntry` validation rejects empty `addr` or non-64-hex `cert_sha256`.

- [ ] **Step 3: Run tests to verify RED**
  Run: `cargo test -p portico-core --test config_tests`
  Expected: FAIL.

- [ ] **Step 4: Implement `crates/portico-core/src/config.rs`**
- [ ] **Step 5: Run tests to verify GREEN**
  Run: `cargo test -p portico-core --test config_tests`
  Expected: PASS.

- [ ] **Step 6: Commit**
  `git add crates/portico-core && git commit -m "feat(core): persistent config store, VidPid validation, and licensing hook"`

---

### Task 2: Bounded Ring-Buffer Logger & Event Bus

**Files:**
- Create: `crates/portico-core/src/logging.rs`
- Modify: `crates/portico-core/src/lib.rs`
- Test: `crates/portico-core/tests/logging_tests.rs`

**Interfaces:**
- Produces:
  - `LogEntry { pub seq: u64, pub timestamp_ms: u64, pub level: LogLevel, pub target: String, pub message: String }`
  - `LogRingBuffer::new(capacity: usize)` with `push(...) -> LogEntry`, `since(seq: u64) -> Vec<LogEntry>`, `clear()`
  - `DaemonEvent` enum (`DeviceAdded`, `DeviceRemoved`, `DeviceState`, `ServerState`, `RelayState`, `LockChanged`, `Log`) serialized via `to_control_line() -> String`

- [ ] **Step 1: Write failing tests in `crates/portico-core/tests/logging_tests.rs`**
  Test:
  - Ring buffer capped at capacity (e.g. 3): pushing 5 entries retains seq 3, 4, 5; `since(3)` returns 4, 5.
  - `DaemonEvent::to_control_line()` percent-encodes spaces/newlines in `product`/`message` so output is always a single `\n`-free line.

- [ ] **Step 2: Run tests to verify RED**
  Run: `cargo test -p portico-core --test logging_tests`
  Expected: FAIL.

- [ ] **Step 3: Implement `crates/portico-core/src/logging.rs`**
- [ ] **Step 4: Run tests to verify GREEN**
  Run: `cargo test -p portico-core --test logging_tests`
  Expected: PASS.

- [ ] **Step 5: Commit**
  `git add crates/portico-core && git commit -m "feat(core): bounded ring-buffer logger and control-channel event serializer"`

---

### Task 3: Generic `rusb` USB Handler + Device Registry + Ignored Policy

**Files:**
- Create: `crates/portico-core/src/usb_handler.rs`
- Create: `crates/portico-core/src/registry.rs`
- Modify: `crates/portico-core/src/lib.rs`
- Test: `crates/portico-core/tests/registry_tests.rs`

**Interfaces:**
- Consumes: `usbip::{UsbDevice, UsbInterface, UsbEndpoint, UsbIpServer, UsbInterfaceHandler, UsbDeviceHandler}`, `rusb::{DeviceHandle, UsbContext}`, `VidPid`
- Produces:
  - `PorticoRusbInterfaceHandler<C: UsbContext>` & `PorticoRusbDeviceHandler<C: UsbContext>`
  - `usb_device_from_rusb_handle<C: UsbContext>(handle: DeviceHandle<C>, bus_id: &str, bus_num: u32, dev_num: u32, speed_override: UsbSpeedSetting) -> Result<UsbDevice, CoreError>`
  - `DeviceState::{Shared, InUse(String), Ignored, PendingPermission}`
  - `DeviceRecord { pub bus_id: String, pub vid: u16, pub pid: u16, pub manufacturer: String, pub product: String, pub serial: String, pub state: DeviceState }`
  - `DeviceRegistry` wrapping `Arc<UsbIpServer>` + metadata table with `register_device(...)`, `register_pending_permission(...)`, `detach_device(bus_id)`, `apply_ignored_list(&[VidPid])`, `list_devices() -> Vec<DeviceRecord>`

- [ ] **Step 1: Write failing tests in `crates/portico-core/tests/registry_tests.rs`**
  Test using simulated `usbip::UsbDevice` instances (e.g., two CDC-ACM simulated devices with distinct `bus_id`s `1-1` and `1-2`, distinct `vid/pid`s):
  - Adding a device whose `vid/pid` is in `ignored` marks it `DeviceState::Ignored` and does NOT expose it in `UsbIpServer::available_devices` (cannot be imported via USB/IP).
  - Removing a `vid/pid` from `ignored` transitions the attached device to `DeviceState::Shared` and adds it to `UsbIpServer`.
  - Adding an already-shared idle device to `ignored` removes it from `UsbIpServer` and marks it `Ignored`.
  - `detach_device` removes device cleanly; unknown `bus_id` returns `CoreError::NotFound`.

- [ ] **Step 2: Run tests to verify RED**
  Run: `cargo test -p portico-core --test registry_tests`
  Expected: FAIL.

- [ ] **Step 3: Implement `usb_handler.rs` and `registry.rs`**
  Note: On `rusb::Context::open_device_with_fd`, `DeviceHandle<C>` implements `UsbContext`-generic USB transfers (`read_control`, `write_control`, `read_bulk`, `write_bulk`, `read_interrupt`, `write_interrupt`), and `usb_device_from_rusb_handle` reads `device_descriptor`, `active_config_descriptor`, strings (tolerating read errors just like `usbip`), and attaches `PorticoRusbInterfaceHandler<C>` / `PorticoRusbDeviceHandler<C>`.

- [ ] **Step 4: Run tests to verify GREEN**
  Run: `cargo test -p portico-core --test registry_tests`
  Expected: PASS.

- [ ] **Step 5: Commit**
  `git add crates/portico-core && git commit -m "feat(core): generic rusb URB handler and policy-aware USB device registry"`

---

### Task 4: Wire Protocol Session Handler (`PRTC` v0 Server Loop + `usbip::handler` Upgrade)

**Files:**
- Create: `crates/portico-core/src/session.rs`
- Modify: `crates/portico-core/src/lib.rs`
- Test: `crates/portico-core/tests/session_tests.rs`

**Interfaces:**
- Consumes: `portico_proto::frame::*`, `portico_proto::token::verify_claim_token`, `portico_proto::tls::is_lan_or_loopback_ip`, `DeviceRegistry`, `usbip::handler`
- Produces:
  - `serve_portico_stream<S: AsyncRead + AsyncWrite + Unpin>(stream: S, peer_addr: SocketAddr, is_tls: bool, state: Arc<DaemonInner>) -> Result<(), CoreError>`

- [ ] **Step 1: Write failing tests in `crates/portico-core/tests/session_tests.rs`**
  Test over in-memory `tokio::io::duplex` + loopback `TcpListener`:
  - `ClientHello` with valid token + `ListDevicesReq` returns shared devices (and omits ignored ones).
  - `ClientHello` with wrong token returns `ServerHello { status: AuthFailed }` and closes stream (subsequent `ListDevicesReq` fails with EOF).
  - Plaintext stream (`is_tls = false`) with public peer IP (`8.8.8.8:12345`) returns `PolicyRejected` even if `plaintext_lan = true`.
  - `OpenTunnelReq("1-1")` returns `OpenTunnelResp { status: Ok }`, then `OP_REQ_IMPORT("1-1")` succeeds and `GetDescriptor(Device)` returns the simulated device descriptor!
  - `OpenTunnelReq` for an ignored or nonexistent `bus_id` returns `OpenTunnelResp` with `Ignored`/`NotFound` without upgrading to `usbip::handler`.

- [ ] **Step 2: Run tests to verify RED**
  Run: `cargo test -p portico-core --test session_tests`
  Expected: FAIL.

- [ ] **Step 3: Implement `crates/portico-core/src/session.rs`**

- [ ] **Step 4: Run tests to verify GREEN**
  Run: `cargo test -p portico-core --test session_tests`
  Expected: PASS.

- [ ] **Step 5: Commit**
  `git add crates/portico-core && git commit -m "feat(core): PRTC v0 session handler and usbip tunnel handoff"`

---

### Task 5: Server Networking Engine (TCP TLS/Plaintext Listener, Reverse-Client mTLS Loop, Relay Client, UDP Discovery)

**Files:**
- Create: `crates/portico-core/src/network.rs`
- Modify: `crates/portico-core/src/lib.rs`
- Test: `crates/portico-core/tests/network_tests.rs`

**Interfaces:**
- Consumes: `serve_portico_stream`, `portico_proto::{tls, discovery, relay_proto}`
- Produces:
  - `NetworkSupervisor` managing cancelable Tokio tasks for:
    1. TCP server (`tls` default; optional LAN-only plaintext fallback check via TLS ClientHello peek `0x16` when `plaintext_lan == true` and peer is LAN IP!)
    2. Reverse-client outbound mTLS dialers (persistent control + on-demand `RequestReverseDataConn` data mTLS connections)
    3. Relay client (`ControlRegister` -> `IncomingClient` -> `ServerAccept` + TLS handshake) + `RelayStatus` state tracking (`Disabled`, `Initializing`, `Ready(uri)`, `Error(msg)`)
    4. UDP discovery responder (`37421`)

- [ ] **Step 1: Write failing tests in `crates/portico-core/tests/network_tests.rs`**
  Test:
  - TLS server on `127.0.0.1:0` accepts `tokio-rustls` client with TOFU pin, lists simulated device.
  - Plaintext LAN peek test: when `plaintext_lan == false`, sending `PRTC` plaintext bytes is rejected; when `plaintext_lan == true` on `127.0.0.1`, plaintext `PRTC` works AND TLS still works on the same port!
  - Relay E2E test: spin up `portico_relay::RelayServer` on `127.0.0.1:0`, enable relay on daemon, wait for `RelayStatus::Ready(uri)`, connect client via `uri`, complete TLS + `ClientHello` + `ListDevicesReq`.
  - Reverse-client mTLS test: spin up mock reverse TLS server pinning daemon's cert, add it to daemon `reverse_clients`, daemon dials out, completes mTLS, listener requests `ListDevicesReq` AND `RequestReverseDataConn` -> concurrent second connection attaches device!
  - UDP discovery responder responds to `DiscoveryPacket::Request` with matching `nonce`, `tcp_port`, and `cert_fp_prefix`.

- [ ] **Step 2: Run tests to verify RED**
  Run: `cargo test -p portico-core --test network_tests`
  Expected: FAIL.

- [ ] **Step 3: Implement `crates/portico-core/src/network.rs`**
  Note: Binding the TCP listener with `peek` of the first byte (`0x16` TLS ClientHello vs `b'P'` plaintext `PRTC`) allows seamless TLS+optional-LAN-plaintext on the same port (`37420`) without needing two ports, while unconditionally rejecting `b'P'` unless `plaintext_lan && is_lan_or_loopback_ip(peer.ip())`!

- [ ] **Step 4: Run tests to verify GREEN**
  Run: `cargo test -p portico-core --test network_tests`
  Expected: PASS.

- [ ] **Step 5: Commit**
  `git add crates/portico-core && git commit -m "feat(core): TLS/LAN TCP listener, reverse mTLS dialer, relay client, and UDP discovery"`

---

### Task 6: Daemon Facade (`PorticoDaemon`) & Line-Based `portico-ctrl` Unix-Socket Server

**Files:**
- Create: `crates/portico-core/src/daemon.rs`
- Create: `crates/portico-core/src/control.rs`
- Modify: `crates/portico-core/src/lib.rs`
- Test: `crates/portico-core/tests/control_tests.rs`

**Interfaces:**
- Produces:
  - `PorticoDaemon::start(data_dir: PathBuf, ctrl_socket_path: PathBuf) -> Result<Self, CoreError>`
  - `PorticoDaemon::handle() -> DaemonHandle`
  - `DaemonHandle::exec_command(&self, line: &str) -> String` (also called directly by control socket loop and JNI if needed)
  - All 22 commands from `DESIGN.md` §5 (`REGISTER_CLIENT`, `UNREGISTER_CLIENT`, `GET_STATE`, `SET_STATE`, `START_SERVER`, `STOP_SERVER`, `GET_IGNORED`, `SET_IGNORED`, `GET_REVERSE_CLIENTS`, `SET_REVERSE_CLIENTS`, `SET_RELAY`, `GET_RELAY`, `SET_NET_LOCK`, `SET_WAKE_LOCK`, `SET_SPEED`, `LIST_DEVICES`, `DETACH_DEVICE`, `GET_TOKEN`, `REGEN_TOKEN`, `GET_LOGS`, `SUBSCRIBE`, `SHUTDOWN`).

- [ ] **Step 1: Write failing tests in `crates/portico-core/tests/control_tests.rs`**
  Test over real `tokio::net::UnixStream` connected to `<tempdir>/portico-ctrl.sock`:
  - `REGISTER_CLIENT android-ui` -> `OK <id>`; `UNREGISTER_CLIENT <id>` -> `OK`.
  - `GET_STATE` reflects defaults (`tls=true plaintext_lan=false port=37420`).
  - `SET_IGNORED 0123/AE3C,046d/c52b` -> `OK`, `GET_IGNORED` -> `OK 2\nITEM 0123/AE3C\nITEM 046D/C52B\nEND`; invalid `SET_IGNORED BAD` -> `ERR` leaving previous list intact.
  - `SET_REVERSE_CLIENTS 192.168.1.10:37425@<64hex>@true` -> `OK`, `GET_REVERSE_CLIENTS` returns entry; invalid fingerprint -> `ERR`.
  - `SET_NET_LOCK true`, `SET_WAKE_LOCK true`, `SET_SPEED high`, `SET_RELAY false`, `GET_RELAY`, `GET_TOKEN`, `REGEN_TOKEN` (new token != old token), `GET_LOGS`.
  - `SUBSCRIBE` connection receives `EVENT DEVICE_ADDED` when device registered and `EVENT SERVER_STATE` on `START_SERVER`/`STOP_SERVER`.
  - `SHUTDOWN` returns `OK` and stops socket listener cleanly.

- [ ] **Step 2: Run tests to verify RED**
  Run: `cargo test -p portico-core --test control_tests`
  Expected: FAIL.

- [ ] **Step 3: Implement `daemon.rs` and `control.rs`**
  Set Unix socket file permissions to `0o600`. Bound line reads to `65_536` bytes.

- [ ] **Step 4: Run tests to verify GREEN**
  Run: `cargo test -p portico-core --test control_tests`
  Expected: PASS.

- [ ] **Step 5: Commit**
  `git add crates/portico-core && git commit -m "feat(core): PorticoDaemon lifecycle and line-based portico-ctrl socket server"`

---

### Task 7: Concurrent Multi-Device E2E Integration Verification (M1 Acceptance)

**Files:**
- Test: `crates/portico-core/tests/e2e_concurrent_tests.rs`

**Interfaces:**
- Consumes: `PorticoDaemon`, `portico_proto` TLS/TOFU client, `usbip` CDC-ACM simulated devices

- [ ] **Step 1: Write failing E2E test in `crates/portico-core/tests/e2e_concurrent_tests.rs`**
  Test:
  - Start `PorticoDaemon` in tempdir, register two simulated USB CDC-ACM devices (`1-1` and `1-2`), send `START_SERVER` over `portico-ctrl.sock`.
  - Connect two concurrent TLS 1.3 clients (using `claim_token` from `GET_TOKEN`, verifying TOFU cert fingerprint), list devices (both appear), open tunnels to `1-1` and `1-2` simultaneously, send `OP_REQ_IMPORT` + `USBIP_CMD_SUBMIT` (Device Descriptor + CDC bulk loopback) on BOTH concurrently, verify both receive expected USB payloads, then unplug `1-1` via `DETACH_DEVICE` and verify `SUBSCRIBE` receives `DEVICE_REMOVED`.

- [ ] **Step 2: Run test to verify it passes (or fails until any integration bug is fixed)**
  Run: `cargo test -p portico-core --test e2e_concurrent_tests`
  Expected: PASS (all assertions green).

- [ ] **Step 3: Commit**
  `git add crates/portico-core && git commit -m "test(core): E2E concurrent multi-device TLS + control-channel verification"`
