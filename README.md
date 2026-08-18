# am5-spd-diag

Capture AMD AM5 DDR5 SPD/DIMM identity across boot, shutdown, and systemd
sleep/resume. Detects firmware-visible SPD hub corruption (Unknown/missing
part numbers, 8-bit width, optional SPD5118 MR11=0x08) after sleep and warm
reboot. Installed capacity is remembered from the last **healthy** capture,
not assumed.

Sleep policy (`deep` / `s2idle`) is never changed.

## Install (this tree)

```bash
sudo make PREFIX=/usr install
am5-spd-diag status
am5-spd-diag report    # /var/log/am5-spd-diag/reports/
```

That enables:

1. **Boot / shutdown** — `am5-spd-diag.service`
2. **Before sleep** — `am5-spd-diag-pre-sleep.service`
3. **After resume** — `am5-spd-diag-post-sleep.service`

Logs are `2775 root:users` via systemd-tmpfiles. A capture that finds **current**
corruption writes a notice and notifies logged-in sessions.

Remove the software with `sudo make PREFIX=/usr uninstall` (or the distro
package manager). That keeps logs. Delete captured evidence with
`am5-spd-diag purge` (does not uninstall the program).

## Commands

| Command | Purpose |
|---|---|
| `am5-spd-diag status` | Is SPD identity healthy **right now**? DIMMs + units |
| `am5-spd-diag analyze` | Sleep/reboot history, corruption chain, hub evidence |
| `am5-spd-diag snapshot` | Capture now (polkit; snapshot helper only) |
| `am5-spd-diag report` | Polkit snapshot, then ticket markdown |
| `am5-spd-diag package` | Polkit snapshot, then tarball |
| `am5-spd-diag probe` | Read SPD5118 MR11 on 0x50–0x53 (root / i2c) |
| `am5-spd-diag recover` | Experimental in-band MR11 clear; **does not reboot** |
| `am5-spd-diag purge` | Delete captured logs/reports; does **not** uninstall |

`am5-spd-diag help` and `am5-spd-diag help <command>` print the same text as `-h`.
`report` / `snapshot` elevate only `/usr/libexec/am5-spd-diag/pkexec-snapshot` via polkit
(action `org.opensuse.am5-spd-diag.snapshot`). That helper cannot run `recover`.
From a git checkout, polkit only allows that **installed** path — install the
package (or `sudo make PREFIX=/usr install`) before expecting a graphical prompt
from the tree binary.

Probe and recover talk only to host SMBus adapters (PIIX4 / I801 / FCH / AMD
SMBus), not GPU or DesignWare buses.

Motherboard and DIMM serials are included in tickets. System UUID and asset
tags are omitted.

`recover` is manual only. It warns, writes MR11 only when it reads `0x08`, then
asks you to warm reboot.

A current-corruption capture posts a **persistent urgent** desktop notice.
Click it for `am5-spd-diag status`. **Analyze** and **Report** buttons open a
terminal on those commands. Acting on the notice dismisses the banner but
leaves it in the desktop notification history. GNOME uses `org.gtk.Notifications` so actions can
outlive the sender. Plasma, XFCE, Cinnamon, MATE, LXQt, and dunst use the
freedesktop Notifications API, which only delivers button clicks to the
connection that called `Notify` — so the helper stays connected until you
click or dismiss. On Wayland the click also carries an activation token;
without it the compositor often refuses to map the terminal. SSH/console
sessions still get journal + `wall`. Report is shown in `glow` when installed,
otherwise `bat` / `mdcat` / `python3-rich`, and `less` as the common fallback.

## Packaging

- RPM: `am5-spd-diag.spec` (OBS project `home:fritz-fritz`)
- Debian: OBS `debian.*` + `am5-spd-diag.dsc` (debtransform); local `debian/`
  (`dh` + the same Makefile)
- Changelog source of truth is `am5-spd-diag.changes`. After editing it, run
  `python3 scripts/gen_changelogs.py` to refresh `debian.changelog`,
  `debian/changelog`, and the spec `%changelog`.
- `dmidecode` is recommended. Capture reads SMBIOS from sysfs first and
  falls back to the `dmidecode` binary if the kernel table is unavailable.

## Workaround (operational)

Warm reboot does not clear a stuck hub. AC power cut does. Optional:
`sudo am5-spd-diag recover` then reboot.
