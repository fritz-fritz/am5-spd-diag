use am5_spd_diag::analyze::{
    build_context, load_timeline, load_timeline_from_package, make_package, open_package,
    print_analyze, print_inventory, print_status, render_report, write_report, PackageSession,
};
use am5_spd_diag::capture::capture_main;
use am5_spd_diag::config::load_config;
use am5_spd_diag::dimm::{format_dimm_summary, summary_flags};
use am5_spd_diag::hub::{
    confirm_fix, format_probe_human, print_probe, print_recover_result, recover_run, recover_warn,
};
use am5_spd_diag::notify::{ensure_session_env, notify_bin_path};
use am5_spd_diag::paths::{
    helper_kind, pin_helper_paths, pkexec_helper_path, run_pkexec_helper, share_dir,
    user_purge_targets, HelperKind, SYSTEM_STATE_DIR,
};
use am5_spd_diag::purge::{purge_then_user, PurgeSystem};
use am5_spd_diag::safe_fs::set_privileged_umask;
use am5_spd_diag::smbios::{collect_memory_dump, parse_memory_devices};
use std::env;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{self, Command};

const USAGE: &str = "\
am5-spd-diag — AM5 DDR5 SPD hub diagnostics after sleep / warm reboot

Usage: am5-spd-diag <command> [options]
       am5-spd-diag help [command]

Look
  status              Is SPD identity healthy right now? Current DIMMs and units.
  analyze [--from FILE]
                      History: sleeps, warm reboot vs poweroff, hub evidence.
  open [status|analyze|report|probe] [--from FILE]
                      Open the GTK results window (same views as the notice).
  probe               Read MR11 on kernel spd5118 hubs (host SMBus; no password in a local session).

Capture
  snapshot            Capture DIMM/SPD evidence now (no password in a local session).

Report
  report [--from FILE] Snapshot, print ticket markdown, and save it.
  package [--all]     Snapshot then evidence tarball.

Fix
  fix                 Experimental in-band MR11 clear. Warns; does not reboot.

Logs
  purge [--yes]       Delete captured evidence. Does not remove the program.

Captures in /var/log/am5-spd-diag are world-readable and root-owned.
Corruption notifies logged-in sessions.
";

fn help_cmd(cmd: &str) -> i32 {
    let text = match cmd {
        "status" => Some(
            "\
am5-spd-diag status

Show whether firmware is publishing healthy DIMM identity right now:
  - SPD now: healthy | corrupted | unknown
  - populated DIMMs (locator, size, part, width)
  - firmware-published RAM size vs last healthy baseline
  - systemd units and the system-sleep hook

Does not dump sleep/reboot history. For that: am5-spd-diag analyze
",
        ),
        "analyze" => Some(
            "\
am5-spd-diag analyze [--from FILE]

Investigate captured history:
  - sleep cycles this package recorded vs kernel suspend_stats
  - warm reboot vs poweroff between healthy and corrupt snapshots
  - per-boot timeline and corruption transitions
  - SPD5118 hub / dmesg evidence

--from FILE reads a package tarball (or extracted directory) instead of
live logs. Implied --no-snapshot.

Does not show unit health. For that: am5-spd-diag status
Ticket markdown: am5-spd-diag report
",
        ),
        "snapshot" => Some(
            "\
am5-spd-diag snapshot

Take one capture now (event=manual). Writes under
/var/log/am5-spd-diag/events/ (world-readable, root-owned).
If not root, runs the snapshot helper via pkexec
(/usr/libexec/am5-spd-diag/pkexec-snapshot). A local active session does
not need a password. That helper cannot fix hubs or change sleep policy.
",
        ),
        "report" => Some(
            "\
am5-spd-diag report [--no-snapshot] [--from FILE] [--out FILE]

Capture a current snapshot (local session, no password), then fill the
ticket template. Prints the markdown (same as status/analyze print their
text), then the saved path on the last line. Writes under
/var/log/am5-spd-diag/reports/ (readable by any local user). If that
directory is not writable, uses $XDG_DATA_HOME/am5-spd-diag/reports/
(~/.local/share/am5-spd-diag/reports/ by default). --no-snapshot skips
the capture (tests / already-fresh logs).

--from FILE loads a package tarball (or extracted directory) instead of
live logs. Skips snapshot. Prints markdown only unless --out FILE is given.
",
        ),
        "package" => Some(
            "\
am5-spd-diag package [--all] [--no-snapshot]

Build an evidence tarball (report, timeline, alerts, baseline, event dirs).
Captures a snapshot first unless --no-snapshot.
Default: alerted boots plus the preceding sleep/reboot chain.
--all includes every captured event directory.
The GTK window Package button does the same without a snapshot, then
opens the tarball's folder.
",
        ),
        "open" => Some(
            "\
am5-spd-diag open [status|analyze|report|probe] [--from FILE]

Open the GTK results window. Prefixes the same commands the CLI already
prints to stdout:

  am5-spd-diag open
  am5-spd-diag open analyze
  am5-spd-diag open probe
  am5-spd-diag open report --from FILE.tar.gz

Does not capture a snapshot. Needs a display session and the installed
helper /usr/libexec/am5-spd-diag/am5-spd-diag-notify (or the sibling
binary next to am5-spd-diag in a cargo target dir).
",
        ),
        "probe" => Some(
            "\
am5-spd-diag probe [--json]

Read SPD5118 MR11 (register 0x0B) on kernel spd5118 hubs.
If not root, runs pkexec-probe (no password in a local active session).
Targets come from sysfs (and dmesg-stuck IDs),
not a scan of empty 0x50–0x53 slots. Stuck hubs report MR11=0x08.
",
        ),
        "fix" | "recover" => Some(
            "\
am5-spd-diag fix [--yes]

Experimental in-band clear of a stuck SPD5118 hub (MR11 0x08 → 0x0000).
Requires admin authentication (GTK Fix and CLI fix). Prompts for
YES unless --yes. Does not rewrite EEPROM. Does not reboot.
Records a `recover` timeline event (hub.json + recover.json) so analyze
can credit the in-band clear. A warm reboot is still required after a
successful clear.
",
        ),
        "purge" => Some(
            "\
am5-spd-diag purge [--yes]

Delete captured evidence (timeline, events, baseline, reports, packages).
Does not uninstall the program, systemd units, or /etc/am5-spd-diag.conf.

User reports under $XDG_DATA_HOME/am5-spd-diag (default
~/.local/share/am5-spd-diag) are removed as you only after
/var/log/am5-spd-diag is wiped with systemd-tmpfiles (needs root).
A failed sudo leaves your XDG reports in place.
Prompts for YES unless --yes.

To remove the software itself: rpm/dnf/zypper/apt, or
`sudo make PREFIX=/usr uninstall` from the source tree.
",
        ),
        "help" | "" => {
            print!("{USAGE}");
            return 0;
        }
        _ => None,
    };
    match text {
        Some(t) => {
            print!("{t}");
            0
        }
        None => {
            eprintln!("am5-spd-diag: no help for command: {cmd}");
            eprint!("{USAGE}");
            1
        }
    }
}

fn euid() -> u32 {
    unsafe { libc::geteuid() }
}

fn need_root(args: &[String]) -> i32 {
    if euid() == 0 {
        return 0;
    }
    let exe = env::current_exe().unwrap_or_else(|_| PathBuf::from("am5-spd-diag"));
    let mut cmd = Command::new("sudo");
    cmd.arg("--").arg(exe);
    cmd.args(args);
    cmd.status().unwrap_or_default().code().unwrap_or(1)
}

fn pkexec_snapshot_path() -> PathBuf {
    PathBuf::from("/usr/libexec/am5-spd-diag/pkexec-snapshot")
}

fn run_privileged_snapshot() -> i32 {
    if euid() == 0 {
        return capture_main(&["manual".into()]);
    }
    let helper = if pkexec_snapshot_path().is_file() {
        Some(pkexec_snapshot_path())
    } else {
        let local = am5_spd_diag::paths::libexec_dir().join("pkexec-snapshot");
        if local.is_file() {
            Some(local)
        } else {
            None
        }
    };
    if let Some(helper) = helper {
        if which("pkexec") {
            return Command::new("pkexec")
                .arg(helper)
                .status()
                .map(|s| s.code().unwrap_or(1))
                .unwrap_or(1);
        }
    }
    eprintln!(
        "am5-spd-diag: snapshot needs the polkit helper ({}).",
        pkexec_snapshot_path().display()
    );
    eprintln!("Install the package, or re-run as root. Continuing without a privileged capture.");
    1
}

fn which(name: &str) -> bool {
    Command::new("which")
        .arg(name)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn cmd_purge(args: &[String]) -> i32 {
    let mut yes = false;
    let mut system_only = false;
    for arg in args {
        match arg.as_str() {
            "--yes" | "-y" => yes = true,
            "--system" => system_only = true,
            _ => {
                eprintln!("am5-spd-diag: unknown option: {arg}");
                let _ = help_cmd("purge");
                return 1;
            }
        }
    }
    let user_targets = if system_only {
        Vec::new()
    } else {
        user_purge_targets()
    };
    if euid() != 0 {
        print_purge_plan(&user_targets, true);
        if !confirm_purge(yes) {
            return 1;
        }
        let rc = need_root(&["purge".into(), "--yes".into(), "--system".into()]);
        if rc != 0 {
            return rc;
        }
        remove_user_purge_targets(&user_targets);
        println!("Purged.");
        return 0;
    }
    if !system_only {
        print_purge_plan(&user_targets, false);
        if !confirm_purge(yes) {
            return 1;
        }
    }
    if let Err(err) = purge_then_user(&PurgeSystem::installed(), &user_targets) {
        eprintln!("am5-spd-diag: {err}");
        return 1;
    }
    if !system_only {
        println!("Purged.");
    }
    0
}

fn print_purge_plan(user_targets: &[PathBuf], needs_root: bool) {
    println!("This deletes captured evidence (program and units stay installed):");
    let root_note = if needs_root {
        "  (systemd-tmpfiles, needs root; wiped first)"
    } else {
        "  (systemd-tmpfiles; wiped first)"
    };
    println!("  {SYSTEM_STATE_DIR}{root_note}");
    for path in user_targets {
        println!("  {}  (after system logs)", path.display());
    }
}

fn confirm_purge(yes: bool) -> bool {
    if yes {
        return true;
    }
    print!("Type YES to purge: ");
    let _ = io::stdout().flush();
    let mut ans = String::new();
    let _ = io::stdin().read_line(&mut ans);
    if ans.trim() != "YES" {
        println!("Aborted.");
        return false;
    }
    true
}

fn remove_user_purge_targets(targets: &[PathBuf]) {
    for path in targets {
        let _ = std::fs::remove_dir_all(path);
    }
}

fn cmd_open(args: &[String]) -> i32 {
    ensure_session_env();
    let bin = notify_bin_path();
    if !bin.is_file() {
        eprintln!("am5-spd-diag: GTK helper not found ({})", bin.display());
        eprintln!("Install the package, or: sudo make PREFIX=/usr install");
        return 1;
    }
    let mut notify_args = Vec::new();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "status" | "analyze" | "report" | "probe" => {
                notify_args.push(format!("--{}", args[i]));
            }
            "--status" | "--analyze" | "--report" | "--probe" => {
                notify_args.push(args[i].clone());
            }
            "--from" => {
                let Some(path) = args.get(i + 1) else {
                    eprintln!("am5-spd-diag: --from needs a path");
                    return 1;
                };
                if path.starts_with('-') {
                    eprintln!("am5-spd-diag: --from needs a path");
                    return 1;
                }
                notify_args.push("--from".into());
                notify_args.push(path.clone());
                i += 1;
            }
            arg if arg.starts_with("--from=") => notify_args.push(arg.to_string()),
            "--no-snapshot" => {}
            "-h" | "--help" => return help_cmd("open"),
            other => {
                eprintln!(
                    "am5-spd-diag: open shows status, analyze, report, or probe (not {other})"
                );
                let _ = help_cmd("open");
                return 1;
            }
        }
        i += 1;
    }
    let status = Command::new(&bin).args(&notify_args).status();
    match status {
        Ok(s) => s.code().unwrap_or(1),
        Err(e) => {
            eprintln!("am5-spd-diag: failed to launch {}: {e}", bin.display());
            1
        }
    }
}

fn maybe_snapshot(skip: bool) -> i32 {
    if skip {
        return 0;
    }
    run_privileged_snapshot()
}

fn from_archive(args: &[String]) -> Result<Option<PathBuf>, String> {
    let mut found = None;
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--from" {
            let Some(p) = args.get(i + 1) else {
                return Err("--from needs a path".into());
            };
            if p.starts_with('-') {
                return Err("--from needs a path".into());
            }
            found = Some(PathBuf::from(p));
            i += 2;
            continue;
        }
        if let Some(rest) = args[i].strip_prefix("--from=") {
            if rest.is_empty() {
                return Err("--from needs a path".into());
            }
            found = Some(PathBuf::from(rest));
        }
        i += 1;
    }
    Ok(found)
}

fn load_from_package(
    path: &Path,
) -> Result<(PackageSession, Vec<am5_spd_diag::TimelineEvent>), String> {
    let pkg = open_package(path)?;
    let events = load_timeline_from_package(&pkg.root);
    Ok((pkg, events))
}

fn print_report_text(text: &str) {
    print!("{text}");
    if !text.ends_with('\n') {
        println!();
    }
}

fn run_user_command(cmd: &str, args: &[String]) -> i32 {
    let prefix = share_dir();
    env::set_var("AM5_SPD_DIAG_SHARE", &prefix);
    env::set_var("AM5_SPD_DIAG_PREFIX", &prefix);
    let cfg = load_config(&prefix);
    let state_dir = cfg.state_dir();
    let from = match from_archive(args) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("am5-spd-diag: {e}");
            return 1;
        }
    };
    if matches!(cmd, "status") && from.is_some() {
        eprintln!("am5-spd-diag: --from applies to analyze and report");
        return 1;
    }
    match cmd {
        "status" => {
            let events = load_timeline(&state_dir);
            let ctx = build_context(&cfg, &events, &state_dir);
            print_status(&events, &ctx);
            0
        }
        "analyze" | "summary" => {
            if let Some(archive) = from {
                let (pkg, events) = match load_from_package(&archive) {
                    Ok(v) => v,
                    Err(e) => {
                        eprintln!("am5-spd-diag: {e}");
                        return 1;
                    }
                };
                let ctx = build_context(&cfg, &events, &pkg.root);
                print_analyze(&events, &ctx);
                return 0;
            }
            let events = load_timeline(&state_dir);
            let ctx = build_context(&cfg, &events, &state_dir);
            print_analyze(&events, &ctx);
            0
        }
        "inventory" => {
            print_inventory();
            0
        }
        "report" => {
            let mut skip = env::var("AM5_SPD_DIAG_SKIP_SNAPSHOT").ok().as_deref() == Some("1");
            let mut out = None;
            for arg in args {
                if arg == "--no-snapshot" {
                    skip = true;
                } else if let Some(path) = arg.strip_prefix("--out=") {
                    out = Some(PathBuf::from(path));
                }
            }
            let mut i = 0;
            while i < args.len() {
                if args[i] == "--out" {
                    if let Some(p) = args.get(i + 1) {
                        out = Some(PathBuf::from(p));
                        i += 1;
                    }
                }
                i += 1;
            }
            if let Some(archive) = from {
                let (pkg, events) = match load_from_package(&archive) {
                    Ok(v) => v,
                    Err(e) => {
                        eprintln!("am5-spd-diag: {e}");
                        return 1;
                    }
                };
                let ctx = build_context(&cfg, &events, &pkg.root);
                let text = render_report(&prefix, &events, &ctx, &cfg);
                print_report_text(&text);
                if let Some(out) = out {
                    let path = write_report(&prefix, &pkg.root, &cfg, &events, &ctx, Some(out));
                    println!("{}", path.display());
                }
                return 0;
            }
            if maybe_snapshot(skip) != 0 {
                eprintln!("am5-spd-diag: snapshot failed; not writing a ticket from stale logs");
                return 1;
            }
            let events = load_timeline(&state_dir);
            let ctx = build_context(&cfg, &events, &state_dir);
            let path = write_report(&prefix, &state_dir, &cfg, &events, &ctx, out);
            let text = std::fs::read_to_string(&path).unwrap_or_default();
            print_report_text(&text);
            println!("{}", path.display());
            0
        }
        "package" => {
            let mut skip = env::var("AM5_SPD_DIAG_SKIP_SNAPSHOT").ok().as_deref() == Some("1");
            let mut include_all = false;
            let mut package_dir = None;
            let mut i = 0;
            while i < args.len() {
                match args[i].as_str() {
                    "--no-snapshot" => skip = true,
                    "--all" => include_all = true,
                    "--package-dir" => {
                        if let Some(p) = args.get(i + 1) {
                            package_dir = Some(PathBuf::from(p));
                            i += 1;
                        }
                    }
                    _ => {}
                }
                i += 1;
            }
            if maybe_snapshot(skip) != 0 {
                eprintln!("am5-spd-diag: snapshot failed; not writing a package from stale logs");
                return 1;
            }
            let events = load_timeline(&state_dir);
            let ctx = build_context(&cfg, &events, &state_dir);
            let dir = package_dir.unwrap_or_else(|| {
                env::var("AM5_SPD_DIAG_PACKAGE_DIR")
                    .map(PathBuf::from)
                    .unwrap_or_else(|_| PathBuf::from(cfg.get("STATE_DIR")).join("packages"))
            });
            match make_package(&prefix, &state_dir, &cfg, &events, &ctx, &dir, include_all) {
                Ok(path) => {
                    println!("{}", path.display());
                    0
                }
                Err(e) => {
                    eprintln!("am5-spd-diag: {e}");
                    1
                }
            }
        }
        _ => 1,
    }
}

fn cmd_fix(yes: bool) -> i32 {
    recover_warn();
    if !yes && !confirm_fix() {
        println!("Aborted.");
        return 2;
    }
    if euid() == 0 {
        let (rc, result) = recover_run();
        let _ = print_recover_result(&result);
        return rc;
    }
    if pkexec_helper_path(HelperKind::Recover).is_file() && which("pkexec") {
        match run_pkexec_helper(HelperKind::Recover) {
            Ok(out) => {
                if !out.status.success() {
                    eprint!("{}", String::from_utf8_lossy(&out.stderr));
                }
                match serde_json::from_slice::<serde_json::Value>(&out.stdout) {
                    Ok(result) => {
                        let printed = print_recover_result(&result);
                        if out.status.success() {
                            printed
                        } else {
                            out.status.code().unwrap_or(1)
                        }
                    }
                    Err(_) => {
                        print!("{}", String::from_utf8_lossy(&out.stdout));
                        out.status.code().unwrap_or(1)
                    }
                }
            }
            Err(e) => {
                eprintln!("am5-spd-diag: {e}");
                eprintln!("Install the package, or re-run as root: sudo am5-spd-diag fix");
                1
            }
        }
    } else {
        eprintln!(
            "am5-spd-diag: fix needs the polkit helper ({}).",
            pkexec_helper_path(HelperKind::Recover).display()
        );
        eprintln!("Install the package, or re-run as root: sudo am5-spd-diag fix");
        1
    }
}

fn flags_cmd(summary: &str) -> i32 {
    let text = if summary == "-" {
        use std::io::Read;
        let mut buf = String::new();
        let _ = io::stdin().read_to_string(&mut buf);
        buf
    } else {
        std::fs::read_to_string(summary).unwrap_or_default()
    };
    println!("{}", summary_flags(&text).join(","));
    0
}

fn cmd_probe(rest: &[String]) -> i32 {
    let json_out = rest.iter().any(|a| a == "--json");
    if euid() != 0 && pkexec_helper_path(HelperKind::Probe).is_file() && which("pkexec") {
        match run_pkexec_helper(HelperKind::Probe) {
            Ok(out) => {
                if !out.status.success() {
                    eprint!("{}", String::from_utf8_lossy(&out.stderr));
                    if json_out {
                        print!("{}", String::from_utf8_lossy(&out.stdout));
                    }
                    return out.status.code().unwrap_or(1);
                }
                if json_out {
                    print!("{}", String::from_utf8_lossy(&out.stdout));
                    return 0;
                }
                match serde_json::from_slice::<serde_json::Value>(&out.stdout) {
                    Ok(probe) => println!("{}", format_probe_human(&probe)),
                    Err(_) => print!("{}", String::from_utf8_lossy(&out.stdout)),
                }
                0
            }
            Err(e) => {
                eprintln!("am5-spd-diag: {e}");
                print_probe(json_out);
                0
            }
        }
    } else {
        print_probe(json_out);
        0
    }
}

fn summarize_cmd(path: &str) -> i32 {
    let data = if path == "-" {
        use std::io::Read;
        let mut buf = Vec::new();
        let _ = io::stdin().read_to_end(&mut buf);
        buf
    } else {
        std::fs::read(path).unwrap_or_default()
    };
    print!("{}", format_dimm_summary(&parse_memory_devices(&data)));
    0
}

fn main() {
    let mut args: Vec<String> = env::args().collect();
    if let Some(kind) = helper_kind() {
        // Each helper binary has one job. Extra argv (for example `snapshot`
        // after pkexec) is ignored so a symlink /usr/bin/am5-spd-diag ->
        // pkexec-snapshot cannot reject `am5-spd-diag snapshot`.
        pin_helper_paths();
        set_privileged_umask();
        let rc = match kind {
            HelperKind::Snapshot => capture_main(&["manual".into()]),
            HelperKind::Probe => {
                print_probe(true);
                0
            }
            HelperKind::Recover => {
                let (rc, result) = recover_run();
                println!(
                    "{}",
                    serde_json::to_string_pretty(&result).unwrap_or_else(|_| "{}".into())
                );
                rc
            }
        };
        process::exit(rc);
    }
    args.remove(0);
    if args.first().map(String::as_str) == Some("snapshot-helper") {
        if args.len() != 1 {
            eprintln!("am5-spd-diag snapshot helper accepts no arguments");
            process::exit(2);
        }
        pin_helper_paths();
        set_privileged_umask();
        process::exit(capture_main(&["manual".into()]));
    }
    let cmd = args.first().cloned().unwrap_or_default();
    if cmd == "help" {
        process::exit(help_cmd(args.get(1).map(String::as_str).unwrap_or("")));
    }
    if cmd != "capture"
        && (args.get(1).map(String::as_str) == Some("-h")
            || args.get(1).map(String::as_str) == Some("--help"))
    {
        process::exit(help_cmd(&cmd));
    }
    let rest = if args.is_empty() {
        Vec::new()
    } else {
        args[1..].to_vec()
    };
    let rc = match cmd.as_str() {
        "-h" | "--help" | "" => {
            print!("{USAGE}");
            0
        }
        "status" => run_user_command("status", &rest),
        "capture" => capture_main(&rest),
        "snapshot" => run_privileged_snapshot(),
        "analyze" => run_user_command("analyze", &rest),
        "open" => cmd_open(&rest),
        "report" => run_user_command("report", &rest),
        "package" => run_user_command("package", &rest),
        "probe" => cmd_probe(&rest),
        "fix" | "recover" => cmd_fix(rest.iter().any(|a| a == "--yes")),
        "purge" => cmd_purge(&rest),
        "inventory" => run_user_command("inventory", &rest),
        "flags" => flags_cmd(rest.first().map(String::as_str).unwrap_or("-")),
        "summarize" => summarize_cmd(rest.first().map(String::as_str).unwrap_or("-")),
        "dump-memory" => {
            let mut table = None;
            let mut sysfs_only = false;
            let mut i = 0;
            while i < rest.len() {
                match rest[i].as_str() {
                    "--table" => {
                        table = rest.get(i + 1).map(PathBuf::from);
                        i += 1;
                    }
                    "--sysfs-only" => sysfs_only = true,
                    _ => {}
                }
                i += 1;
            }
            let (text, source) = collect_memory_dump(table.as_deref(), !sysfs_only);
            print!(
                "{}",
                if text.ends_with('\n') {
                    text
                } else {
                    format!("{text}\n")
                }
            );
            if source == "none" {
                1
            } else {
                0
            }
        }
        _ => {
            eprintln!("am5-spd-diag: unknown command: {cmd}");
            eprint!("{USAGE}");
            1
        }
    };
    process::exit(rc);
}
