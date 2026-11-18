# Portico — clean-room build briefing (archived AI-agent instructions used during development)

## Working name

**Working name:** Portico (chosen 2026-11-16; alternates if we rename later: Tetherio, NetLanyard, Bridgely — folder renamed to `Portico/`, docs updated)

---

---

## The prompt to execute

> Use the `brainstorming` skill first — this is new creative design work. Then follow §Process below; use the `writing-plans`, `subagent-driven-development` (or `executing-plans`), `test-driven-development`, `requesting-code-review`/`receiving-code-review`, and `verification-before-completion` skills at the marked points.

**Goal.** Build a from-scratch clone of a USB-over-the-network server with a brand-new UI, per `FEATURES.md`. Do **not** look at, reference, port, or "consult" the original application whose feature list this is — implement purely from this spec. Pick your own names, icons, layout, wording and wire protocol.

**Platform:** Android (phone/server side) + desktop clients that can consume the same server.

**Tech stack (decided, don't re-litigate in brainstorm unless blocked):**
- **Core daemon engine:** Rust — single crate, USB hotplug/auto-share, network service.
- **Android GUI:** Kotlin + Jetpack Compose (Material 3), app wraps the Rust core (via FFI or a thin local-socket control channel) or pure Kotlin + Rust binary; simplest build that works.
- **Desktop clients:** Rust core reused; CLI first (`portico` command-line client), GUI later (Tauri or GTK, decide at plan time) — desktop scope may be a thin client for now (list/claim devices over the wire).
- **Licensing: N/A — free app.** No license format, keys, billing, or purchase flow for now (no Play Billing). Leave a hook in the architecture so a paid tier can be added later without rework.
- **Protocol:** our own (versioned). Keep a compatibility note so the plan can decide later if we ever expose a compat mode — not in v1 scope.

**Deliverable of the whole run:** working Android APK on the phone + desktop client on this machine, each feature per `FEATURES.md` verified end-to-end (plug a device → see it shared → client uses it) — not just unit tests.

## §Process (step by step)

1. `$brainstorming` — shape the design: product naming, UI design system, wire protocol, storage, logging. Write `DESIGN.md`.
2. `$writing-plans` — one plan file per subsystem in `plans/`, each task ≤ ~20 min, each with its test plan.
3. For each plan task: `$test-driven-development` (RED → GREEN → `code-review`) → `$requesting-code-review` when the task's tests pass → fix `$receiving-code-review`.
4. Integrate per `$subagent-driven-development` (or `$executing-plans`) as we execute tasks in the current session; run `$verification-before-completion` before claiming any feature complete.
5. Milestones M1 core, M2 Android GUI, M3 desktop client, M4 UI polish; each with runnable acceptance on the real phone (`OnePlus 12R`) and a real device plugged in.

Definition of done for every feature: implemented → tests green → reviewed → demoed on device/desktop → logged in `PROGRESS.md`.

---

## Appendix: pointers to existing pieces (what's legal to use)

**Use directly (MIT/Apache)**
- https://github.com/jiegec/usbip — Rust USB/IP server lib (MIT) — the core plugs into this
- https://github.com/microsoft/usbipd-win — Windows USB/IP server + GUI v5 (MIT)
- https://github.com/beriberikix/usbipd-mac — macOS server (MIT)
- https://github.com/libusb/libusb — LGPL (dynamic-link only, as used from Rust wrapper)

**Protocol spec (documented — no new protocol work)**
- https://github.com/torvalds/linux/blob/master/Documentation/usb/usbip_protocol.rst

**GPL reference (look, don't link — clean-room boundary)**
- https://github.com/vadimgrn/usbip-win2 — Windows client, WHQL drivers
- Linux kernel `vhci-hcd` (in-kernel client) + `usbip`/`usbipd` tools
- https://github.com/mwmakeiyong/Network-USB · https://github.com/suifei/usbctl · https://github.com/Penderrin-Projects/ZeroHub · https://github.com/otti83/airusb-hub · https://github.com/matumi-software/koppel · https://github.com/yunsmall/usbipdcpp

**Android glue**
- libusb-on-Android (userspace claim, per-device intent grants — root-free path proven by the original app)
- Concrete recipe (verified vs libusb official Android README + rusb upstream): Kotlin `UsbManager.requestPermission` → `UsbDeviceConnection.getFileDescriptor()` → pass fd to Rust over JNI/JNA → `LIBUSB_OPTION_NO_DEVICE_DISCOVERY` + `libusb_wrap_sys_device` (= `rusb` `Context::open_device_with_fd`); Android persists device grants across reboots.
- **License sweep (2026-11-16): every public Android USB/IP server is GPL-3.0 or unlicensed — none MIT/Apache.** All are look-don't-link references: cgutman/USBIPServerForAndroid (GPL-3, Java, fd-injection USBIP over localhost:3240), lleokaganov/wsusb (GPL-3, same + encrypted WebSocket relay — design reference for the relay feature), yunsmall/Android-Usbipdcpp (GPL-3 Kotlin; core lib usbipdcpp is LGPLv3 — dynamic-linking LGPL ≠ GPL infection, kept out by choice at stricter-boundary), techphenom/USBIP-Server-Android (GPL-3, C, Android 11+), forensix/libusbip (no license, dead).
- USBIP protocol note: kernel `usbip_host` module does not exist on Android → the server must be fully userspace (libusb + USBIP protocol), same architecture as usbipd-win.

**License = ours (no pointer — write new)**
- Android app UI, thin Rust daemon glue, desktop CLI, LAN discovery
- macOS *client* driver (DriverKit) — only hard new landmine; v1 skip
