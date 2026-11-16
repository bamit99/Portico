# Portico Progress

## Milestones
- [x] **M0 — Design & Plans**: `DESIGN.md` + 4 subsystem plans (`plans/protocol-security.md`, `plans/core-daemon.md`, `plans/desktop-cli.md`, `plans/android-app.md`)
- [x] **M1 — Core Daemon + Protocol/Security** (`portico-proto`, `portico-relay`, `portico-core`) — 25/25 workspace tests green + code review Critical/Important fixes verified
- [ ] **M2 — Desktop CLI** (`portico-cli`)
- [ ] **M3 — Android GUI + JNI** (`portico-jni`, `android/`)
- [ ] **M4 — Full E2E Polish & On-Device Acceptance** (`OnePlus 12R` + real USB device)

## Log
- **2026-11-16**: Initialized `DESIGN.md` (with builder-check resolution) and `plans/` for all 4 subsystems.
- **2026-11-16**: Completed M1 (`portico-proto`, `portico-relay`, `portico-core`) with TDD + code review fixes (`afa112f`). Note: `rusb` currently uses `vendored` feature on this host because system `libusb-1.0-dev` headers are absent; dynamic-link packaging should be revisited before binary release per README LGPL note.
