# Kernel command line

Linux kernel reference for Pertisk KOS. Some parameters are
**required** for the image to boot; others are optional. Dotted `pertisk.*`
tokens are parsed from `/proc/cmdline` by `pertiskd` (Linux does not promote
names containing `.` into init’s environment). Underscore `KEY=value` tokens
without dots are available as process env as usual.

Override the baked cmdline at image build with `PERTISK_CMDLINE`
([`image/cloud/populate-disk.sh`](../image/cloud/populate-disk.sh),
[`image/build-uki.sh`](../image/build-uki.sh)).

## Default / required parameters

| Arch | Default `options` / UKI cmdline |
|------|----------------------------------|
| **amd64** | `console=tty0 console=ttyS0 rdinit=/init` |
| **arm64** | `earlycon=pl011,0x09000000 console=tty0 console=ttyAMA0 console=ttyS0 arm64.nopauth rdinit=/init` |

- **`rdinit=/init`** — initramfs entry is `pertiskd` (PID 1).
- **`console=`** — last console becomes `/dev/console`. Lab scripts also set
  Proxmox `vga=serial0` so the web Console opens serial/xterm.js; see
  [PROXMOX.md](./PROXMOX.md).

QEMU smoke appends `-- --smoke` (and often `--state-dir=…`) after a `--`
separator so clap flags reach `pertiskd` without confusing kernel leftovers.

## Recommended (KSPP)

Not on the image default yet; safe to add via `PERTISK_CMDLINE` when hardening:

- `slab_nomerge` — Kernel Self Protection Project
- `pti=on` — Kernel Self Protection Project

See [HARDENING.md](./HARDENING.md).

## Dashboard parameters

Pertisk runs a fullscreen serial/console status dashboard by default (Proxmox /
ESXi Serial). Dashboard knobs:

### `pertisk.dashboard.disabled`

If set to a truthy value (`1`, `true`, `yes`, `on`), the console TUI is **not**
started and stderr is not silenced — kernel and userspace logs stay on the
active console.

```text
pertisk.dashboard.disabled=1
```

**Alias (env / undotted cmdline):** `PERTISK_DASHBOARD_DISABLED=1`

Also disabled by CLI `--no-dashboard` (after `--` on the append) or smoke mode.

### `pertisk.dashboard.console`

Device for dashboard I/O (and preferred stdio redirect). The name must start
with `tty` (with or without `/dev/` prefix).

```text
pertisk.dashboard.console=ttyS0
```

**Alias:** `PERTISK_DASHBOARD_CONSOLE=ttyS0`

When set, `pertiskd` prefers `/dev/<name>` for stdio redirect and TUI paint.
If that device is missing, it falls back to the arch default serial candidates.

> **Note:** If the dashboard console is the same device as a kernel
> `console=` (e.g. both `ttyS0`), output can interleave or fight. Prefer a
> dedicated tty when both dmesg and the TUI must stay readable.

### Theme / geometry (existing)

These undotted tokens become init env (and/or are set from
`machine.dashboard` YAML). See [PROXMOX.md](./PROXMOX.md) for details.

| Variable | Values |
|----------|--------|
| `PERTISK_DASHBOARD_THEME` | `catppuccin` (default), `dracula`, `nord`, `gruvbox`, `tokyo-night`, `solarized`, `cyberpunk`, `wild-cherry`, `mono` |
| `PERTISK_DASHBOARD_BORDER` | `line` (default, Serial-safe `-`), `ascii`, `auto`, `light`, `rounded`, `heavy`, `double` |
| `PERTISK_DASHBOARD_BACKGROUND` | `#RRGGBB` |
| `PERTISK_DASHBOARD_COLS` / `_ROWS` | pin geometry; else probe |
| `PERTISK_DASHBOARD_UTF8` | force Unicode borders |
| `PERTISK_DASHBOARD_PROBE` | `=1` enable CSI size probe |
| `MGMT_PUBLIC_URL` / `PERTISK_MGMT_URL` | mgmt URL on the node panel |

Priority: YAML `machine.dashboard` overwrites when a field is set; cmdline/env
fills gaps; then built-ins.

## Already handled elsewhere (not cmdline)

| Concern | Mechanism |
|---------|-----------|
| Hostname | `machine.network.hostname` (YAML) |
| Addressing / DHCP | `machine.network` + builtin DHCPv4 |
| Panic reboot delay | sysctl `kernel.panic` / `panic_on_oops` in `pertiskd` |
| IPv4-only vs dual-stack | `cluster.networkMode` (no hard `ipv6.disable=1` on cmdline) |
| Machine config path | STATE partition / `PERTISK_CONFIG` |

## Deferred cmdline parameters

**Not implemented** on the cmdline yet:

| Idea | Status |
|------------------|--------|
| `pertisk.platform=` (metal / aws / …) | Deferred — deploy matrix is docs/scripts only |
| `pertisk.config=` URL / metal-iso | Deferred — STATE YAML / apply path only |
| Early `ip=` / `bond=` / `vlan=` | Deferred — use machine config |
| `net.ifnames=0` | Not set by default |
| `pertisk.network.interface.ignore` | Deferred |
| `pertisk.hostname` pre-config | Deferred — use YAML |
| tty1 logs + tty2 dashboard VT switch | Deferred — single serial dashboard model |

Cloud provider images remain **paused** ([image/cloud/README.md](../image/cloud/README.md)).
