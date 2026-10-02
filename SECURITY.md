# Security

## What this tool reads

`vpnstat` opens no files, reads no configuration, and inspects no packet contents.

It calls exactly three Windows APIs:

| API | Data read |
|---|---|
| `GetIfTable2` | per-adapter byte, discard and error counters, adapter name/type/link speed |
| `GetIpForwardTable` | IPv4 routing table: which interface owns the default route |
| `GetCursorPos` / `GetWindowRect` | mouse position, own window geometry |

## What it does not do

- Sends no network traffic. No ICMP, no TCP probes, no HTTP. `stable` reflects packet
  loss counters, not an active latency measurement.
- Installs no driver, no service, no scheduled task.
- Requires no administrator rights.
- Modifies no VPN client configuration.
- Captures and inspects no packets.
- Collects no telemetry and phones home nowhere.

## Memory

The tool holds no secrets: no keys, no tokens, no hostnames beyond adapter names shown
in the `--debug` log. Adapter names are printed only to `vpnstat.log` next to the binary,
and only when `VPNSTAT_DEBUG=1` is set. Delete that file to remove it.

## Reporting

Please report vulnerabilities privately through GitHub's "Report a vulnerability" button
on the Security tab rather than opening a public issue.
