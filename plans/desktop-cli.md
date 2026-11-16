# Desktop CLI (`portico-cli`) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the `portico` desktop CLI binary (`crates/portico-cli`) reusing `portico-proto`, `portico-core`, and `portico-relay` to discover, trust (TOFU), list, attach (via loopback USB/IP bridge to Linux `vhci-hcd`), reverse-listen (mTLS), serve (Linux host mode), and relay USB devices.

**Architecture:** `portico-cli` uses `clap` for subcommands and a shared `ClientTransport` module that handles direct TLS 1.3 + TOFU pinning, `prly://` relay dialing + E2E TLS 1.3, and opt-in `--insecure-lan-plaintext` (gated to LAN IPs). `portico attach` spins up an ephemeral `127.0.0.1:<port>` loopback socket, opens the authenticated `PRTC` v0 tunnel, and invokes `/usr/bin/usbip --tcp-port <port> attach -r 127.0.0.1 -b <bus_id>` (or prints the exact command when `--no-vhci` / non-privileged test mode is requested) while pumping bytes bidirectionally.

**Tech Stack:** `portico-proto`, `portico-core`, `portico-relay`, `clap`, `tokio`, `tokio-rustls`, `rusb`, `serde_json`.

**Spec:** `DESIGN.md` (§2, §4, §6, §7, §8), `FEATURES.md` (§Networking, §Security), `README.md` (§Tech stack, §Appendix).

## Global Constraints

- Binary name: `portico`.
- Desktop trust store: `~/.config/portico/known_hosts.json` (or `--config-dir` override for isolated tests).
- TLS 1.3 + TOFU required by default; `--insecure-lan-plaintext` fails immediately if target IP is non-LAN.
- Never print or log claim tokens unless user explicitly runs `portico serve --show-token` or reads from config.
- Support attaching 2+ devices concurrently (multiple `portico attach` processes or multi-bus attach).

## Review Focus

1. `portico` connecting to a server whose certificate fingerprint changed since first trust — must abort with `CertPinMismatch` error showing expected vs received fingerprint before sending `--token`. (Pinned in Task 1)
2. `--insecure-lan-plaintext` passed for a public IP — must fail closed before opening TCP or sending `ClientHello`. (Pinned in Task 1)
3. `portico attach` when `usbip` exits non-zero (e.g. missing `vhci-hcd` permissions) — must surface actionable stderr + exit non-zero cleanly without hanging indefinitely. (Pinned in Task 3)
4. `portico listen` receiving a reverse mTLS connection from an untrusted server cert — must reject at TLS layer when `--allow-server` pin does not match. (Pinned in Task 4)
5. `prly://` URI missing node ID or containing non-hex chars — must return crisp CLI validation error before network I/O. (Pinned in Task 1)

---

### Task 1: CLI Target Parsing, TOFU Client Transport, & `list` / `trust` Commands

**Files:**
- Create: `crates/portico-cli/Cargo.toml`
- Create: `crates/portico-cli/src/lib.rs`
- Create: `crates/portico-cli/src/transport.rs`
- Create: `crates/portico-cli/src/main.rs`
- Test: `crates/portico-cli/tests/client_transport_tests.rs`

**Interfaces:**
- Consumes: `portico_proto::{tls, tofu, frame, relay_proto}`, `portico_core::PorticoDaemon`
- Produces:
  - `TargetEndpoint::{Direct(String), Relay { relay_addr: String, node_id: [u8; 16], pin_key: String }}`
  - `connect_and_auth(...) -> Result<AuthenticatedStream, CliError>`
  - `cmd_list(...) -> Result<Vec<DeviceSummary>, CliError>`
  - `cmd_trust(config_dir, target, fp_hex) -> Result<(), CliError>`

- [ ] **Step 1: Create `crates/portico-cli/Cargo.toml`, `src/lib.rs`, `src/main.rs` stub**
- [ ] **Step 2: Write failing tests in `crates/portico-cli/tests/client_transport_tests.rs`**
  Test against a real in-process `PorticoDaemon`:
  - Untrusted first connection without `--trust` fails with `UntrustedCert` displaying the server's SHA-256 fingerprint (and does NOT send claim token).
  - With `cmd_trust` or `--trust <fp>`, `cmd_list` pins cert in `<tempdir>/known_hosts.json`, authenticates with token, and returns shared devices.
  - Subsequent `cmd_list` against a restarted daemon with a rotated identity cert fails with `CertPinMismatch`.
  - `--insecure-lan-plaintext` against `8.8.8.8:37420` fails before TCP connect.
  - Connecting via `prly://127.0.0.1:<relay_port>/<node_id_hex>` through a live `RelayServer` succeeds end-to-end.

- [ ] **Step 3: Run tests to verify RED**
  Run: `cargo test -p portico-cli --test client_transport_tests`
  Expected: FAIL.

- [ ] **Step 4: Implement `crates/portico-cli/src/transport.rs` and `list`/`trust` handlers**
- [ ] **Step 5: Run tests to verify GREEN**
  Run: `cargo test -p portico-cli --test client_transport_tests`
  Expected: PASS.

- [ ] **Step 6: Commit**
  `git add crates/portico-cli && git commit -m "feat(cli): TOFU client transport, trust pinning, and list command"`

---

### Task 2: `portico discover` (UDP LAN Discovery) & `portico relay` Subcommand

**Files:**
- Create: `crates/portico-cli/src/discover.rs`
- Modify: `crates/portico-cli/src/lib.rs`
- Modify: `crates/portico-cli/src/main.rs`
- Test: `crates/portico-cli/tests/discover_tests.rs`

**Interfaces:**
- Consumes: `portico_proto::discovery::DiscoveryPacket`, `portico_relay::RelayServer`
- Produces:
  - `discover_servers(bind_addr: SocketAddr, target_addr: SocketAddr, timeout: Duration) -> Result<Vec<DiscoveredServer>, CliError>`

- [ ] **Step 1: Write failing test in `crates/portico-cli/tests/discover_tests.rs`**
  Test: bind daemon UDP discovery on `127.0.0.1:0`, call `discover_servers` targeting that loopback address, assert returned `DiscoveredServer` matches daemon `tcp_port` and `cert_fp_prefix`.

- [ ] **Step 2: Run test to verify RED**
- [ ] **Step 3: Implement `discover.rs` and wire `discover` + `relay` CLI subcommands**
- [ ] **Step 4: Run test to verify GREEN**
- [ ] **Step 5: Commit**
  `git add crates/portico-cli && git commit -m "feat(cli): UDP LAN discovery and standalone relay subcommand"`

---

### Task 3: `portico attach` Loopback USB/IP Bridge & `detach`

**Files:**
- Create: `crates/portico-cli/src/bridge.rs`
- Modify: `crates/portico-cli/src/lib.rs`
- Modify: `crates/portico-cli/src/main.rs`
- Test: `crates/portico-cli/tests/bridge_tests.rs`

**Interfaces:**
- Consumes: `connect_and_auth`, `Frame::OpenTunnelReq` / `OpenTunnelResp`
- Produces:
  - `start_local_usbip_bridge(remote_stream: AuthenticatedStream, bus_id: &str) -> Result<(SocketAddr, JoinHandle<Result<(), CliError>>), CliError>`
  - `cmd_attach(...)` (invokes `usbip --tcp-port <port> attach -r 127.0.0.1 -b <bus_id>` when `vhci` enabled, or runs local TCP proxy in `--proxy-only` mode)

- [ ] **Step 1: Write failing test in `crates/portico-cli/tests/bridge_tests.rs`**
  Test: connect to `PorticoDaemon` with simulated CDC-ACM device `1-1`, call `start_local_usbip_bridge`, connect a raw `TcpStream` to the returned `127.0.0.1:<port>`, send standard `OP_REQ_IMPORT("1-1")` + `GetDescriptor(Device)` USB/IP PDUs, and assert valid USB/IP `OP_REP_IMPORT` + 18-byte device descriptor are returned over the TLS tunnel! Also test non-existent `bus_id` fails before spawning bridge.

- [ ] **Step 2: Run test to verify RED**
- [ ] **Step 3: Implement `crates/portico-cli/src/bridge.rs` and wire `attach` / `detach`**
- [ ] **Step 4: Run test to verify GREEN**
- [ ] **Step 5: Commit**
  `git add crates/portico-cli && git commit -m "feat(cli): TLS-to-local-USBIP loopback bridge and attach/detach commands"`

---

### Task 4: `portico listen` (Reverse mTLS Listener), `portico identity`, & `portico serve` (Linux Host Daemon)

**Files:**
- Create: `crates/portico-cli/src/reverse.rs`
- Create: `crates/portico-cli/src/serve.rs`
- Modify: `crates/portico-cli/src/lib.rs`
- Modify: `crates/portico-cli/src/main.rs`
- Test: `crates/portico-cli/tests/reverse_serve_tests.rs`

**Interfaces:**
- Consumes: `portico_core::{PorticoDaemon, usb_device_from_rusb_handle}`, `portico_proto::tls`
- Produces:
  - `ReverseListener` accepting mTLS connections, completing `ClientHello`, exposing local control or attaching multiple devices concurrently via `RequestReverseDataConn`
  - `cmd_identity` printing desktop client's SHA-256 cert fingerprint
  - `cmd_serve` running `PorticoDaemon` on desktop Linux (optional `--scan-host` / `--simulate-cdc` for dev testing)

- [ ] **Step 1: Write failing test in `crates/portico-cli/tests/reverse_serve_tests.rs`**
  Test:
  - Start `ReverseListener` on `127.0.0.1:0` allowing `PorticoDaemon`'s cert fingerprint; configure daemon's `reverse_clients` with listener's `addr` and cert fingerprint; start daemon -> listener receives mTLS connection, lists 2 simulated devices, and opens 2 concurrent `RequestReverseDataConn` bridges to attach both devices simultaneously!
  - Untrusted server cert connecting to `ReverseListener` is rejected during TLS handshake.

- [ ] **Step 2: Run test to verify RED**
- [ ] **Step 3: Implement `reverse.rs` and `serve.rs`**
- [ ] **Step 4: Run test to verify GREEN**
- [ ] **Step 5: Commit**
  `git add crates/portico-cli && git commit -m "feat(cli): reverse mTLS listener, identity command, and serve host command"`
