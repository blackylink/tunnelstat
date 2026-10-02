# Contributing

Thanks for looking at `tunnelstat`. Issues and pull requests are welcome.

## Before you open a PR

The CI has to be green. On Windows:

```powershell
$env:RUSTUP_TOOLCHAIN = "stable-x86_64-pc-windows-gnu"
$env:PATH = "$env:CARGO_HOME\bin;<mingw>\bin;$env:PATH"
cargo fmt --check
cargo clippy --release
cargo build --release
```

`mingw` is required because `build.rs` shells out to `windres` to embed the icon and
the manifest. CI installs it via `choco install mingw`.

## Rules for changes

- **Keep it small.** The whole program is one `.exe` and a single source file. Adding a
  dependency for something Win32 already does is a regression.
- **Do not add network traffic.** Reading kernel counters (`GetIfTable2`,
  `GetIpForwardTable`) is what keeps this tool silent. Probes, pings and HTTP calls break
  the central promise, and they are also what makes the health states untrustworthy.
- **Do not make the health states optimistic.** A red that should be yellow is worse than a
  missing red. When you change a threshold, say in the PR why the old value lied.
- **Release the GDI objects you create.** Every `CreateSolidBrush` / `CreatePen` /
  `CreateFont` needs a matching `DeleteObject`. The smoke test in CI checks that the
  handle count stays flat, so a leak fails the build.
- **Do not read or write VPN client configuration.** The tool observes; it does not
  configure.

## Screenshots

State images in `docs/` are generated, not captured by hand:

```powershell
tunnelstat.exe --state 0   # stable
tunnelstat.exe --state 1   # unstable
...
```

Use `--state N` rather than `--demo` when you need one specific frame.

## Reporting bugs

Open an issue with:

- your Windows version and whether it is 24h or scaled,
- the VPN client and adapter name (`ipconfig /all` is enough),
- what you expected versus what the panel showed.

If auto-detection failed, try setting `interface=` in
`%APPDATA%\tunnelstat\config.txt` and say whether that fixed it — that distinction is
useful.

## License

By contributing you agree that your work is licensed under the MIT license in `LICENSE`.
