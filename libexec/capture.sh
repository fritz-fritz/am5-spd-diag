#!/bin/bash
# Event capture for systemd boot/shutdown and sleep.target units.
# Never changes sleep policy. Always exits 0 so suspend cannot fail.
set -u

PATH="/usr/sbin:/sbin:/usr/bin:/bin:$PATH"
HERE="$(cd "$(dirname "$0")" && pwd)"
if [[ -n "${AM5_SPD_DIAG_SHARE:-}" ]]; then
  SHARE="$AM5_SPD_DIAG_SHARE"
elif [[ "$HERE" == /usr/libexec/am5-spd-diag ]]; then
  SHARE=/usr/share/am5-spd-diag
elif [[ "$HERE" == /usr/local/libexec/am5-spd-diag ]]; then
  SHARE=/usr/local/share/am5-spd-diag
else
  SHARE="$(cd "$HERE/.." && pwd)"
fi
HUB_PY="$HERE/spd_hub.py"
ANALYZE_PY="$HERE/analyze.py"

if [[ -f "$SHARE/config/default.conf" ]]; then
  # shellcheck disable=SC1091
  . "$SHARE/config/default.conf"
fi
if [[ -f /etc/am5-spd-diag.conf ]]; then
  # shellcheck disable=SC1091
  . /etc/am5-spd-diag.conf
fi

STATE_DIR="${AM5_SPD_DIAG_STATE_DIR:-${STATE_DIR:-/var/log/am5-spd-diag}}"
EVENTS_DIR="$STATE_DIR/events"
TIMELINE="$STATE_DIR/timeline.jsonl"
ALERTS="$STATE_DIR/ALERTS.log"
LATEST="$STATE_DIR/latest"
KEEP_DAYS="${KEEP_DAYS:-60}"

EVENT="${1:-manual}"
SLEEP_TYPE="${2:-}"

log_err() { echo "am5-spd-diag capture: $*" >&2; }

ensure_dirs() {
  mkdir -p "$EVENTS_DIR" "$LATEST"
}

utc_now() { date -u +%Y%m%dT%H%M%S.%NZ; }
iso_now() { date -Is; }
boot_id() { cat /proc/sys/kernel/random/boot_id 2>/dev/null || echo unknown; }

suspend_success() {
  cat /sys/power/suspend_stats/success 2>/dev/null || echo unknown
}

detect_shutdown_mode() {
  local mode=""
  if [[ -r /run/systemd/shutdown/scheduled ]]; then
    mode="$(awk -F= '/^MODE=/ {print $2}' /run/systemd/shutdown/scheduled 2>/dev/null || true)"
  fi
  if [[ -z "$mode" ]] && command -v systemctl >/dev/null 2>&1; then
    if systemctl list-jobs 2>/dev/null | grep -q 'reboot.target.*start'; then
      mode=reboot
    elif systemctl list-jobs 2>/dev/null | grep -qE 'poweroff.target.*start|halt.target.*start'; then
      mode=poweroff
    fi
  fi
  echo "${mode:-shutdown}"
}

normalize_event() {
  case "$EVENT" in
    shutdown)
      case "$(detect_shutdown_mode)" in
        reboot) EVENT=reboot ;;
        poweroff|halt) EVENT=poweroff ;;
        *) EVENT=shutdown ;;
      esac
      ;;
    pre)
      case "$SLEEP_TYPE" in
        hibernate|hybrid-sleep|suspend-then-hibernate) EVENT=hibernate-pre ;;
        *) EVENT=suspend-pre ;;
      esac
      ;;
    post)
      case "$SLEEP_TYPE" in
        hibernate|hybrid-sleep|suspend-then-hibernate) EVENT=hibernate-post ;;
        *) EVENT=suspend-post ;;
      esac
      ;;
  esac
}

memtotal_kb() {
  awk '/^MemTotal:/ {print $2}' /proc/meminfo 2>/dev/null || echo 0
}

mem_sleep_state() {
  tr -d '\n' </sys/power/mem_sleep 2>/dev/null || echo unknown
}

write_dimm_raw() {
  local out="$1"
  if [[ -f "$HUB_PY" ]] && python3 "$HUB_PY" dump-memory >"$out" 2>/dev/null; then
    return
  fi
  if command -v dmidecode >/dev/null 2>&1; then
    dmidecode -t memory >"$out" 2>/dev/null || echo "dmidecode failed" >"$out"
  else
    echo "SMBIOS memory dump unavailable (no sysfs DMI table and no dmidecode)" >"$out"
  fi
}

write_dimm_summary() {
  local raw="$1" summary="$2"
  if [[ -f "$HUB_PY" ]]; then
    python3 "$HUB_PY" summarize "$raw" >"$summary" 2>/dev/null || : >"$summary"
    return
  fi
  : >"$summary"
}

write_system_files() {
  local dir="$1"
  local f
  for f in bios_vendor bios_version bios_date bios_release \
           board_vendor board_name board_version board_serial \
           sys_vendor product_name product_version product_family product_sku \
           chassis_vendor chassis_type chassis_version; do
    printf '%s=' "$f"
    cat "/sys/class/dmi/id/$f" 2>/dev/null || echo
  done >"$dir/dmi-sysfs.txt"
  if [[ -r /etc/os-release ]]; then
    cp /etc/os-release "$dir/os-release.txt" 2>/dev/null || true
  fi
  uname -a >"$dir/uname.txt" 2>/dev/null || true
  {
    echo "boot_mode=$( [[ -d /sys/firmware/efi ]] && echo UEFI || echo legacy )"
    echo "arch=$(uname -m 2>/dev/null || echo unknown)"
  } >"$dir/firmware.txt"
  grep -E '^(processor|vendor_id|cpu family|model|model name|stepping|microcode)[[:space:]]*:' \
    /proc/cpuinfo 2>/dev/null | awk '/^processor/ { if (n++) exit } { print }' \
    >"$dir/cpuinfo-head.txt" || true
  if [[ -f "$HUB_PY" ]]; then
    python3 "$HUB_PY" dump-system >"$dir/dmidecode-system.txt" 2>/dev/null || true
  fi
  if [[ -f "$ANALYZE_PY" ]]; then
    python3 "$ANALYZE_PY" inventory >"$dir/system.json" 2>/dev/null || true
  fi
}

corruption_flags() {
  local summary="$1"
  if [[ -f "$HUB_PY" ]]; then
    python3 "$HUB_PY" flags "$summary" 2>/dev/null || true
    return
  fi
  echo ""
}

json_escape() {
  python3 -c 'import json,sys; print(json.dumps(sys.stdin.read().rstrip("\n")))' <<<"$1" 2>/dev/null \
    || printf '"%s"' "${1//\"/\\\"}"
}

classify_boot_kind() {
  local current_boot="$1"
  if [[ ! -s "$TIMELINE" ]]; then
    echo unknown
    return
  fi
  python3 - "$TIMELINE" "$current_boot" <<'PY'
import json, sys
path, boot = sys.argv[1], sys.argv[2]
events = []
try:
    with open(path, encoding="utf-8") as fh:
        for line in fh:
            line = line.strip()
            if not line:
                continue
            try:
                events.append(json.loads(line))
            except json.JSONDecodeError:
                continue
except OSError:
    print("unknown")
    raise SystemExit
if not events:
    print("unknown")
    raise SystemExit
if str(events[-1].get("boot_id") or "") == boot:
    print("same_boot")
    raise SystemExit
prev = None
for ev in reversed(events):
    if str(ev.get("boot_id") or "") != boot:
        prev = ev
        break
if not prev:
    print("unknown")
    raise SystemExit
prev_event = str(prev.get("event") or "")
if prev_event == "reboot":
    print("warm_reboot")
elif prev_event in {"poweroff", "shutdown", "halt"}:
    print("shutdown_poweroff")
elif prev_event:
    print("unexpected_power_loss")
else:
    print("unknown")
PY
}

append_timeline() {
  local stamp="$1" event="$2" dir="$3" mem_kb="$4" sleep_state="$5" flags="$6" alert="$7"
  local boot_kind="$8" susp="$9" hub_stuck="${10}"
  local line
  line="$(printf '{"ts":%s,"event":%s,"boot_id":%s,"memtotal_kb":%s,"mem_sleep":%s,"sleep_type":%s,"flags":%s,"alert":%s,"dir":%s,"boot_kind":%s,"suspend_success":%s,"hub_stuck":%s}\n' \
    "$(json_escape "$stamp")" \
    "$(json_escape "$event")" \
    "$(json_escape "$(boot_id)")" \
    "$mem_kb" \
    "$(json_escape "$sleep_state")" \
    "$(json_escape "$SLEEP_TYPE")" \
    "$(json_escape "$flags")" \
    "$alert" \
    "$(json_escape "$dir")" \
    "$(json_escape "$boot_kind")" \
    "$(json_escape "$susp")" \
    "$(json_escape "$hub_stuck")")"
  mkdir -p "$STATE_DIR"
  (
    flock 9
    printf '%s\n' "$line" >>"$TIMELINE"
  ) 9>>"$STATE_DIR/timeline.lock"
}

prune_old_events() {
  find "$EVENTS_DIR" -mindepth 1 -maxdepth 1 -type d -mtime "+$KEEP_DAYS" -exec rm -rf {} + 2>/dev/null || true
  python3 - "$TIMELINE" <<'PY'
import json, os, sys
from pathlib import Path
path = Path(sys.argv[1])
if not path.is_file():
    raise SystemExit
text = path.read_text(errors="replace")
decoder = json.JSONDecoder()
kept = []
idx, n = 0, len(text)
while idx < n:
    while idx < n and text[idx] in " \t\r\n":
        idx += 1
    if idx >= n:
        break
    try:
        ev, end = decoder.raw_decode(text, idx)
    except json.JSONDecodeError:
        nxt = text.find("{", idx + 1)
        if nxt < 0:
            break
        idx = nxt
        continue
    idx = end
    if not isinstance(ev, dict):
        continue
    directory = ev.get("dir") or ""
    if directory and not Path(directory).is_dir():
        continue
    kept.append(json.dumps(ev, separators=(",", ":")))
tmp = path.with_suffix(".jsonl.tmp")
tmp.write_text(("\n".join(kept) + ("\n" if kept else "")), encoding="utf-8")
os.replace(tmp, path)
PY
}

write_healthy_baseline() {
  local dir="$1" mem_kb="$2"
  python3 - "$HUB_PY" "$STATE_DIR/baseline.json" "$dir" "$mem_kb" <<'PY'
import json, os, sys
from pathlib import Path
hub_py, out, event_dir, mem_kb = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4]
sys.path.insert(0, str(Path(hub_py).resolve().parent))
import spd_hub  # noqa: E402

base = Path(event_dir)
dmi = {}
dmi_path = base / "dmi-sysfs.txt"
if dmi_path.is_file():
    for line in dmi_path.read_text(errors="replace").splitlines():
        if "=" in line:
            k, v = line.split("=", 1)
            dmi[k.strip()] = v.strip()
dimms = spd_hub.parse_dimm_summary(
    (base / "dimm-summary.txt").read_text(errors="replace") if (base / "dimm-summary.txt").is_file() else ""
)
hub = {}
hub_path = base / "hub.json"
if hub_path.is_file():
    try:
        hub = json.loads(hub_path.read_text(encoding="utf-8"))
    except json.JSONDecodeError:
        hub = {}
cpu = ""
for line in Path("/proc/cpuinfo").read_text(errors="replace").splitlines():
    if line.lower().startswith("model name"):
        cpu = line.split(":", 1)[1].strip()
        break
os_name = ""
for line in Path("/etc/os-release").read_text(errors="replace").splitlines() if Path("/etc/os-release").is_file() else []:
    if line.startswith("PRETTY_NAME="):
        os_name = line.split("=", 1)[1].strip().strip('"')
        break
payload = {
    "ts": (base / "meta.txt").read_text(errors="replace").splitlines()[0].split("=", 1)[-1] if (base / "meta.txt").is_file() else "",
    "event_dir": str(base),
    "memtotal_kb": int(mem_kb or 0),
    "cpu": cpu,
    "os": os_name,
    "kernel": os.uname().release,
    "dmi": dmi,
    "dimms": dimms,
    "hubs": hub.get("hubs") or [],
}
spd_hub.write_baseline(Path(out), payload)
print(out)
PY
}

notify_corruption() {
  local flags="$1" mem_kb="$2"
  local msg="SPD corruption is current (flags=${flags}; firmware published ${mem_kb} kB). Click for status, or Analyze / Report."
  echo "$msg" >"$STATE_DIR/NOTICE"
  echo corrupt >"$STATE_DIR/SPD_NOW"
  if [[ -f "$HUB_PY" ]]; then
    python3 "$HUB_PY" notify "$msg" >/dev/null 2>&1 || true
  else
    logger -p user.alert -t am5-spd-diag "$msg" 2>/dev/null || true
    wall -n "$msg" >/dev/null 2>&1 || true
  fi
}

clear_notice() {
  echo healthy >"$STATE_DIR/SPD_NOW"
  rm -f "$STATE_DIR/NOTICE"
}

capture_event() {
  ensure_dirs
  normalize_event

  local stamp dir mem_kb sleep_state flags alert=false
  local boot_kind=unknown susp hub_stuck=no
  stamp="$(utc_now)"
  dir="$EVENTS_DIR/${stamp}-${EVENT}"
  mkdir -p "$dir"

  mem_kb="$(memtotal_kb)"
  sleep_state="$(mem_sleep_state)"
  susp="$(suspend_success)"
  if [[ "$EVENT" == boot ]]; then
    boot_kind="$(classify_boot_kind "$(boot_id)")"
  fi

  {
    echo "ts=$(iso_now)"
    echo "event=$EVENT"
    echo "sleep_type=${SLEEP_TYPE:-}"
    echo "boot_id=$(boot_id)"
    echo "uname=$(uname -r)"
    echo "memtotal_kb=$mem_kb"
    echo "mem_sleep=$sleep_state"
    echo "suspend_success=$susp"
    echo "boot_kind=$boot_kind"
    echo "cmdline=$(tr -d '\n' </proc/cmdline)"
  } >"$dir/meta.txt"

  grep -E '^(MemTotal|MemFree|MemAvailable):' /proc/meminfo >"$dir/meminfo-head.txt" 2>/dev/null || true
  free -h >"$dir/free.txt" 2>/dev/null || true
  cat /sys/power/mem_sleep >"$dir/mem_sleep.txt" 2>/dev/null || true

  write_system_files "$dir"
  write_dimm_raw "$dir/dmidecode-memory.txt"
  write_dimm_summary "$dir/dmidecode-memory.txt" "$dir/dimm-summary.txt"

  if [[ -f "$HUB_PY" ]]; then
    python3 "$HUB_PY" probe --json >"$dir/hub.json" 2>/dev/null || echo '{"hubs":[],"stuck":[],"dmesg":[]}' >"$dir/hub.json"
    python3 "$HUB_PY" write-spd-pages "$dir/hub.json" "$dir" 2>/dev/null || true
    dmesg 2>/dev/null | grep -i spd5118 | tail -n 50 >"$dir/dmesg-spd5118.txt" || true
    if python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); sys.exit(0 if d.get("stuck") or d.get("dmesg_stuck") else 1)' "$dir/hub.json" 2>/dev/null; then
      hub_stuck=yes
    fi
  fi

  flags="$(corruption_flags "$dir/dimm-summary.txt")"
  if [[ -z "$flags" && "$hub_stuck" == yes ]]; then
    flags="hub_mr11_stuck"
  elif [[ -n "$flags" && "$hub_stuck" == yes && ",$flags," != *",hub_mr11_stuck,"* ]]; then
    flags="${flags},hub_mr11_stuck"
  fi

  if [[ -n "$flags" ]]; then
    alert=true
    {
      echo "$(iso_now) ALERT event=$EVENT flags=$flags memtotal_kb=$mem_kb boot_kind=$boot_kind dir=$dir"
      cat "$dir/dimm-summary.txt"
      echo
    } >>"$ALERTS"
    echo "$flags" >"$dir/ALERT.flags"
    : >"$STATE_DIR/CORRUPTION_SEEN"
    notify_corruption "$flags" "$mem_kb"
  else
    clear_notice
    write_healthy_baseline "$dir" "$mem_kb" >/dev/null 2>&1 || true
  fi
  echo "alert=$alert" >>"$dir/meta.txt"
  echo "flags=$flags" >>"$dir/meta.txt"
  echo "hub_stuck=$hub_stuck" >>"$dir/meta.txt"

  case "$EVENT" in
    boot|manual|reboot|poweroff|shutdown)
      dmesg 2>/dev/null | grep -Ei 'BIOS-e820|Memory: [0-9]|PM: suspend|suspend entry|suspend exit' \
        | tail -n 400 >"$dir/dmesg-filtered.txt" || true
      dmesg 2>/dev/null | grep -E 'BIOS-e820:' >"$dir/e820.txt" || true
      ;;
  esac

  if [[ "$alert" == true || "$EVENT" == boot || "$EVENT" == manual ]]; then
    if [[ ! -s "$dir/e820.txt" ]]; then
      dmesg 2>/dev/null | grep -E 'BIOS-e820:' >"$dir/e820.txt" || true
    fi
    grep -E 'System RAM' "$dir/e820.txt" >"$dir/e820-system-ram.txt" 2>/dev/null || \
      dmesg 2>/dev/null | grep -E 'BIOS-e820: \[mem .*\]  System RAM' \
        >"$dir/e820-system-ram.txt" || true
  fi

  ln -sfn "$dir" "$LATEST/$EVENT"
  ln -sfn "$dir" "$LATEST/any"
  append_timeline "$(iso_now)" "$EVENT" "$dir" "$mem_kb" "$sleep_state" "$flags" "$alert" "$boot_kind" "$susp" "$hub_stuck"

  prune_old_events

  echo "$dir"
}

case "$EVENT" in
  pre|post|boot|shutdown|reboot|poweroff|suspend-pre|suspend-post|hibernate-pre|hibernate-post|manual)
    if [[ -z "${AM5_SPD_DIAG_IN_TIMEOUT:-}" ]] && command -v timeout >/dev/null 2>&1; then
      AM5_SPD_DIAG_IN_TIMEOUT=1 timeout --preserve-status \
        "${CAPTURE_TIMEOUT_SEC:-20}" env AM5_SPD_DIAG_IN_TIMEOUT=1 \
        bash "$HERE/capture.sh" "$EVENT" "${SLEEP_TYPE:-}" || true
      exit 0
    fi
    if ! capture_event; then
      log_err "capture failed for event=$EVENT (ignored)"
    fi
    exit 0
    ;;
  *)
    log_err "unknown event: $EVENT"
    exit 0
    ;;
esac
