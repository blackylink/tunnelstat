# Changelog

All notable changes to this project are documented here.
Format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), versioning
follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.0] - 2026-10-02

First public release.

### Added
- Always-on-top click-through overlay showing live tunnel speed and connection state.
- Five states: stable / unstable / dropped / leak / VPN down.
- Tunnel auto-detection by adapter name or `IF_TYPE_TUNNEL`, preferring the interface
  that holds the default route.
- Leak detection based on routing evidence (no heuristics on throughput ratio, which
  produced false positives on healthy VPNs).
- Stability from kernel packet-loss counters (`discards`/`errors`).
- Drag to reposition; position persists in `%APPDATA%\tunnelstat\pos.txt`.
- Close button that appears on hover; click-through is lifted only while hovered.
- Hotkeys: `Ctrl+Alt+V` toggle, `Ctrl+Alt+Q` quit.
- Per-monitor DPI awareness v2, so the panel stays sharp on scaled displays.
- Generated multi-resolution icon embedded into the binary; authored app manifest.
- Single-instance guard via named mutex.
- `--zone`, `--interval` flags and `TUNNELSTAT_DEBUG=1` diagnostics.

### Notes
- Zero network traffic. No driver, no admin rights, no VPN client config is read.
- 343 KB single binary, ~1.7 MB private memory, one thread.
