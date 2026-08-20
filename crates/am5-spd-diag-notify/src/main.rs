//! Persistent FDO/gtk notification waiter and GTK results window.
//!
//! Capture never links this crate. A click opens our GTK window, not a terminal.

mod markdown;

use am5_spd_diag::analyze::{
    build_context, format_analyze, format_status, load_timeline, load_timeline_from_package,
    make_package, open_package, recover_cleared, recover_this_boot, render_report, PackageSession,
};
use am5_spd_diag::config::{load_config, Config};
use am5_spd_diag::format_probe_human;
use am5_spd_diag::notify::{
    ensure_session_env, normalize_action, ANALYZE_ACTION, APP_ICON, APP_ID, APP_NAME,
    CLOSE_GRACE_MS, DEFAULT_ACTION, FDO_DEST, FDO_IFACE, FDO_PATH, GTK_NOTIFY_DEST, NOTIFY_ACTIONS,
    NOTIFY_ID, NOTIFY_IMAGE, OBJECT_PATH, REPORT_ACTION,
};
use am5_spd_diag::paths::{run_pkexec_helper, share_dir, HelperKind};
use am5_spd_diag::FORUM_URL;
use gtk4::gio::{self, ApplicationFlags};
use gtk4::{
    Align, Application, ApplicationWindow, Box as GtkBox, Button, HeaderBar, Image, Label,
    Orientation, PolicyType, ScrolledWindow, Separator,
};
use std::cell::RefCell;
use std::collections::HashMap;
use std::env;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};
use zbus::blocking::{Connection, MessageIterator, Proxy};
use zbus::fdo::RequestNameFlags;
use zbus::message::Type as MsgType;
use zbus::zvariant::{OwnedValue, Value};
use zbus::{interface, MatchRule};

fn main() {
    ensure_session_env();
    let args: Vec<String> = env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--notify") => {
            if args.len() < 3 {
                eprintln!("usage: am5-spd-diag-notify --notify TITLE BODY");
                std::process::exit(2);
            }
            send_notification(&args[1], &args[2]);
        }
        Some("--activated") => {
            if !serve_application() {
                show_window("status", None, None);
            }
        }
        _ => {
            let (action, from) = parse_window_args(&args);
            let package = match from {
                Some(path) => match open_package(&path) {
                    Ok(pkg) => Some(pkg),
                    Err(e) => {
                        eprintln!("am5-spd-diag-notify: {e}");
                        std::process::exit(1);
                    }
                },
                None => None,
            };
            show_window(&action, None, package);
        }
    }
}

fn parse_window_args(args: &[String]) -> (String, Option<PathBuf>) {
    let mut action = String::from("status");
    let mut from = None;
    let mut saw_action = false;
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        if a == "--from" {
            if let Some(p) = args.get(i + 1) {
                from = Some(PathBuf::from(p));
                i += 2;
                continue;
            }
            eprintln!("am5-spd-diag-notify: --from needs a path");
            std::process::exit(2);
        } else if let Some(rest) = a.strip_prefix("--from=") {
            from = Some(PathBuf::from(rest));
        } else if let Some(rest) = a.strip_prefix("--") {
            action = normalize_action(rest).to_string();
            saw_action = true;
        } else if !a.starts_with('-') {
            action = normalize_action(a).to_string();
            saw_action = true;
        }
        i += 1;
    }
    if from.is_some() && !saw_action {
        action = "report".into();
    }
    (action, from)
}

fn name_has_owner(name: &str) -> bool {
    let Ok(conn) = Connection::session() else {
        return false;
    };
    conn.call_method(
        Some("org.freedesktop.DBus"),
        "/org/freedesktop/DBus",
        Some("org.freedesktop.DBus"),
        "NameHasOwner",
        &name,
    )
    .ok()
    .and_then(|r| r.body().deserialize::<bool>().ok())
    .unwrap_or(false)
}

fn send_notification(title: &str, body: &str) {
    if name_has_owner(GTK_NOTIFY_DEST) && gtk_add_notification(title, body) {
        return;
    }
    send_fdo_and_wait(title, body);
}

fn gtk_themed_icon(name: &'static str) -> Value<'static> {
    // GThemedIcon serialize: ('themed', <(['name'], use_default_fallbacks)>)
    Value::from(("themed", Value::from((vec![name], true))))
}

fn gtk_notification_dict<'a>(title: &'a str, body: &'a str) -> HashMap<String, Value<'a>> {
    let mut notification = HashMap::new();
    notification.insert("title".into(), Value::from(title));
    notification.insert("body".into(), Value::from(body));
    notification.insert("icon".into(), gtk_themed_icon(NOTIFY_IMAGE));
    notification.insert("priority".into(), Value::from("urgent"));
    notification.insert("default-action".into(), Value::from(DEFAULT_ACTION));
    let analyze = HashMap::from([
        ("label".to_string(), Value::from("Analyze")),
        ("action".to_string(), Value::from(ANALYZE_ACTION)),
    ]);
    let report = HashMap::from([
        ("label".to_string(), Value::from("Report")),
        ("action".to_string(), Value::from(REPORT_ACTION)),
    ]);
    notification.insert("buttons".into(), Value::from(vec![analyze, report]));
    notification
}

fn gtk_add_notification(title: &str, body: &str) -> bool {
    let Ok(conn) = Connection::session() else {
        return false;
    };
    let notification = gtk_notification_dict(title, body);
    conn.call_method(
        Some(GTK_NOTIFY_DEST),
        "/org/gtk/Notifications",
        Some(GTK_NOTIFY_DEST),
        "AddNotification",
        &(APP_ID, NOTIFY_ID, notification),
    )
    .is_ok()
}

fn gtk_remove_notification() {
    if !name_has_owner(GTK_NOTIFY_DEST) {
        return;
    }
    if let Ok(conn) = Connection::session() {
        let _ = conn.call_method(
            Some(GTK_NOTIFY_DEST),
            "/org/gtk/Notifications",
            Some(GTK_NOTIFY_DEST),
            "RemoveNotification",
            &(APP_ID, NOTIFY_ID),
        );
    }
}

fn fdo_hints(resident: bool) -> HashMap<String, Value<'static>> {
    let mut hints = HashMap::new();
    hints.insert("urgency".into(), Value::U8(2));
    // App logo is Icon= in the desktop file; Notify app_icon stays the warning.
    hints.insert("desktop-entry".into(), Value::from(APP_ID));
    if resident {
        hints.insert("resident".into(), Value::Bool(true));
    }
    hints
}

fn send_fdo_and_wait(title: &str, body: &str) {
    let Ok(conn) = Connection::session() else {
        return;
    };
    let Ok(proxy) = Proxy::new(&conn, FDO_DEST, FDO_PATH, FDO_IFACE) else {
        return;
    };
    let hints = fdo_hints(true);
    let nid: u32 = match proxy.call(
        "Notify",
        &(
            APP_NAME,
            0u32,
            NOTIFY_IMAGE,
            title,
            body,
            NOTIFY_ACTIONS,
            hints,
            0i32,
        ),
    ) {
        Ok(nid) => nid,
        Err(_) => return,
    };
    let Ok(builder) = MatchRule::builder()
        .msg_type(MsgType::Signal)
        .interface(FDO_IFACE)
    else {
        return;
    };
    let rule = builder.build();
    let (tx, rx) = mpsc::channel();
    let conn_iter = conn.clone();
    std::thread::spawn(move || {
        let Ok(iter) = MessageIterator::for_match_rule(rule, &conn_iter, Some(64)) else {
            return;
        };
        for msg in iter {
            if tx.send(msg).is_err() {
                break;
            }
        }
    });
    let mut token = String::new();
    let mut closing_until: Option<Instant> = None;
    loop {
        let wait = match closing_until {
            Some(deadline) => {
                let now = Instant::now();
                if now >= deadline {
                    return;
                }
                deadline.saturating_duration_since(now)
            }
            None => Duration::from_secs(24 * 60 * 60),
        };
        let msg = match rx.recv_timeout(wait) {
            Ok(msg) => msg,
            Err(RecvTimeoutError::Timeout) => {
                if closing_until.is_some() {
                    return;
                }
                continue;
            }
            Err(RecvTimeoutError::Disconnected) => return,
        };
        let Ok(msg) = msg else {
            continue;
        };
        let header = msg.header();
        if header.interface().map(|i| i.as_str()) != Some(FDO_IFACE) {
            continue;
        }
        match header.member().map(|m| m.as_str()) {
            Some("ActivationToken") => {
                if let Ok((id, value)) = msg.body().deserialize::<(u32, String)>() {
                    if id == nid {
                        token = value;
                    }
                }
            }
            Some("ActionInvoked") => {
                if let Ok((id, action)) = msg.body().deserialize::<(u32, String)>() {
                    if id == nid {
                        close_fdo(&proxy, nid);
                        drop(rx);
                        show_window(normalize_action(&action), Some(token), None);
                        return;
                    }
                }
            }
            Some("NotificationClosed") => {
                if let Ok((id, _reason)) = msg.body().deserialize::<(u32, u32)>() {
                    if id == nid {
                        closing_until =
                            Some(Instant::now() + Duration::from_millis(CLOSE_GRACE_MS));
                    }
                }
            }
            _ => {}
        }
    }
}

fn close_fdo(proxy: &Proxy<'_>, nid: u32) {
    let _ = proxy.call_method("CloseNotification", &nid);
}

struct AppIface {
    tx: Sender<(String, Option<String>)>,
}

fn activation_token(platform_data: &HashMap<String, OwnedValue>) -> Option<String> {
    for key in ["activation-token", "desktop-startup-id"] {
        let Some(value) = platform_data.get(key) else {
            continue;
        };
        let Ok(text) = String::try_from(value.clone()) else {
            continue;
        };
        let text = text.trim();
        if !text.is_empty() {
            return Some(text.to_string());
        }
    }
    None
}

fn queue_activation(tx: &Sender<(String, Option<String>)>, action: &str, token: Option<String>) {
    let tx = tx.clone();
    let action = action.to_string();
    // Plasma waits for the Activate reply. Sending on this channel unblocks
    // serve_application, which used to drop the connection before zbus could
    // write that reply ("Launching … (Failed)" / NoReply). Defer until after
    // this method returns.
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(50));
        let _ = tx.send((action, token));
    });
}

#[interface(name = "org.freedesktop.Application")]
impl AppIface {
    fn activate(&self, platform_data: HashMap<String, OwnedValue>) {
        queue_activation(&self.tx, "status", activation_token(&platform_data));
    }

    fn open(&self, _uris: Vec<String>, platform_data: HashMap<String, OwnedValue>) {
        queue_activation(&self.tx, "status", activation_token(&platform_data));
    }

    fn activate_action(
        &self,
        action_name: &str,
        _parameter: Vec<OwnedValue>,
        platform_data: HashMap<String, OwnedValue>,
    ) {
        queue_activation(
            &self.tx,
            normalize_action(action_name),
            activation_token(&platform_data),
        );
    }
}

fn serve_application() -> bool {
    let Ok(conn) = Connection::session() else {
        return false;
    };
    let (tx, rx) = mpsc::channel();
    if conn
        .object_server()
        .at(OBJECT_PATH, AppIface { tx })
        .is_err()
    {
        return false;
    }
    match conn.request_name_with_flags(
        APP_ID,
        RequestNameFlags::ReplaceExisting | RequestNameFlags::DoNotQueue,
    ) {
        Ok(zbus::fdo::RequestNameReply::PrimaryOwner) => {}
        Ok(_) => return true,
        Err(_) => return true,
    }
    match rx.recv_timeout(Duration::from_secs(25)) {
        Ok((action, token)) => {
            drop(conn);
            show_window(&action, token, None);
            true
        }
        Err(_) => false,
    }
}

fn window_title(action: &str) -> String {
    let page = match normalize_action(action) {
        "analyze" => "Analyze",
        "report" => "Report",
        "probe" => "Probe",
        _ => "Status",
    };
    format!("Ghost DIMM · AM5 SPD {page}")
}

fn results_text(action: &str, package_root: Option<&Path>) -> (String, String) {
    let prefix = share_dir();
    env::set_var("AM5_SPD_DIAG_SHARE", &prefix);
    let cfg = load_config(&prefix);
    let (events, state) = if let Some(root) = package_root {
        (load_timeline_from_package(root), root.to_path_buf())
    } else {
        let state = cfg.state_dir();
        (load_timeline(&state), state)
    };
    let ctx = build_context(&cfg, &events, &state);
    match action {
        "analyze" => (window_title("analyze"), format_analyze(&events, &ctx)),
        "report" => (
            window_title("report"),
            render_report(&prefix, &events, &ctx, &cfg),
        ),
        "probe" => {
            let hub = events
                .last()
                .map(|e| e.hub.clone())
                .unwrap_or_else(|| serde_json::json!({}));
            let mut body = format_probe_human(&hub);
            if package_root.is_some() {
                body = format!("Captured hub.json (package, not a live probe)\n\n{body}");
            }
            (window_title("probe"), body)
        }
        _ => (window_title("status"), format_status(&events, &ctx)),
    }
}

fn open_containing_folder(parent: &ApplicationWindow, file_path: &Path) {
    use gtk4::gio::prelude::FileExt;
    let dir = file_path.parent().unwrap_or(file_path);
    let uri = gio::File::for_path(dir).uri();
    gtk4::show_uri(Some(parent), uri.as_str(), 0);
}

fn show_message(parent: &ApplicationWindow, kind: gtk4::MessageType, message: &str, detail: &str) {
    use gtk4::prelude::{DialogExt, GtkWindowExt};
    let dialog = gtk4::MessageDialog::builder()
        .transient_for(parent)
        .modal(true)
        .message_type(kind)
        .buttons(gtk4::ButtonsType::Close)
        .text(message)
        .secondary_text(detail)
        .build();
    dialog.connect_response(|d, _| d.close());
    dialog.present();
}

fn package_output_dir(cfg: &Config) -> PathBuf {
    env::var("AM5_SPD_DIAG_PACKAGE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| cfg.state_dir().join("packages"))
}

fn show_package_error(parent: &ApplicationWindow, copied: &Label, err: &str) {
    use gtk4::prelude::WidgetExt;
    copied.set_text("Package failed");
    copied.set_tooltip_text(Some(err));
    eprintln!("am5-spd-diag-notify: {err}");
    show_message(
        parent,
        gtk4::MessageType::Error,
        "Could not write the evidence tarball",
        err,
    );
}

fn live_probe_text() -> (String, String) {
    match run_pkexec_helper(HelperKind::Probe) {
        Ok(out) => {
            if !out.status.success() {
                let err = String::from_utf8_lossy(&out.stderr);
                let stdout = String::from_utf8_lossy(&out.stdout);
                let detail = if err.trim().is_empty() {
                    stdout.to_string()
                } else {
                    err.to_string()
                };
                return (
                    window_title("probe"),
                    format!("Could not read SPD hubs (polkit/i2c).\n\n{}", detail.trim()),
                );
            }
            match serde_json::from_slice::<serde_json::Value>(&out.stdout) {
                Ok(probe) => (window_title("probe"), format_probe_human(&probe)),
                Err(_) => (
                    window_title("probe"),
                    String::from_utf8_lossy(&out.stdout).into_owned(),
                ),
            }
        }
        Err(e) => (
            window_title("probe"),
            format!("{e}\nInstall the package or run: sudo am5-spd-diag probe"),
        ),
    }
}

fn run_live_recover() -> Result<serde_json::Value, String> {
    let out = run_pkexec_helper(HelperKind::Recover)?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    serde_json::from_slice(&out.stdout).map_err(|_| {
        let mut msg = stderr.trim().to_string();
        if msg.is_empty() {
            msg = stdout.trim().to_string();
        }
        if msg.is_empty() {
            msg = format!("pkexec-recover exited {}", out.status.code().unwrap_or(1));
        }
        msg
    })
}

fn update_fix_button(fix_btn: &Button, from_archive: bool) {
    use gtk4::prelude::WidgetExt;
    if from_archive {
        fix_btn.set_sensitive(false);
        fix_btn.set_tooltip_text(Some("Fix is not available while viewing a package"));
        return;
    }
    let prefix = share_dir();
    env::set_var("AM5_SPD_DIAG_SHARE", &prefix);
    let cfg = load_config(&prefix);
    let state = cfg.state_dir();
    let events = load_timeline(&state);
    let ctx = build_context(&cfg, &events, &state);
    if ctx.spd_now != "corrupted" {
        fix_btn.set_sensitive(false);
        fix_btn.set_tooltip_text(Some("SPD identity is not currently corrupted"));
        return;
    }
    let boot_id = events
        .iter()
        .rev()
        .find(|e| e.event == "boot")
        .map(|e| e.boot_id.as_str())
        .unwrap_or("");
    if let Some(ev) = recover_this_boot(&events, boot_id) {
        if recover_cleared(ev) {
            fix_btn.set_sensitive(false);
            fix_btn.set_tooltip_text(Some(
                "MR11 already cleared this boot; warm reboot so firmware re-reads SPD",
            ));
            return;
        }
    }
    fix_btn.set_sensitive(true);
    fix_btn.set_tooltip_text(Some(
        "Experimental in-band MR11 clear (admin password). A warm reboot is required after success.",
    ));
}

fn show_fix_dialog(parent: &ApplicationWindow, after: Rc<dyn Fn()>) {
    use gtk4::prelude::{DialogExt, GtkWindowExt};
    let dialog = gtk4::MessageDialog::builder()
        .transient_for(parent)
        .modal(true)
        .message_type(gtk4::MessageType::Warning)
        .text("Experimental in-band SPD hub fix")
        .secondary_text(format!(
            "This writes SPD5118 MR11 to 0x0000 on hubs that currently read 0x08 (stuck standby page).\n\n\
It does not rewrite SPD EEPROM. The kernel spd5118 driver may still be bound, so the write uses I2C_SLAVE_FORCE. BIOS may show “Devices Changed” after reboot.\n\n\
This will not reboot the machine. A warm reboot is required after a successful clear so firmware re-reads the real SPD. Sleep/wake is not enough.\n\n\
Source: {FORUM_URL}"
        ))
        .build();
    dialog.add_button("Cancel", gtk4::ResponseType::Cancel);
    dialog.add_button("Fix", gtk4::ResponseType::Accept);
    let parent = parent.clone();
    dialog.connect_response(move |d, response| {
        d.close();
        if response != gtk4::ResponseType::Accept {
            return;
        }
        match run_live_recover() {
            Ok(payload) => {
                let ok = payload.get("ok").and_then(|v| v.as_bool()) == Some(true);
                let reason = payload
                    .get("reason")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                let detail = if ok {
                    "The clear was recorded. Warm reboot now so firmware re-reads SPD. Do not expect identity to change until after reboot.".into()
                } else if reason == "no_stuck_hub" {
                    "No hub with MR11=0x08 was found. Firmware can still publish Unknown/missing part until a reboot or AC power loss.".into()
                } else {
                    format!("reason={reason}\n{}", serde_json::to_string_pretty(&payload).unwrap_or_default())
                };
                show_message(
                    &parent,
                    if ok {
                        gtk4::MessageType::Info
                    } else {
                        gtk4::MessageType::Warning
                    },
                    if ok {
                        "MR11 cleared — reboot now"
                    } else {
                        "Fix did not clear MR11"
                    },
                    &detail,
                );
                after();
            }
            Err(e) => {
                show_message(&parent, gtk4::MessageType::Error, "Could not run fix", &e);
            }
        }
    });
    dialog.present();
}

fn apply_activation_token(token: Option<&str>) {
    let Some(token) = token.map(str::trim).filter(|s| !s.is_empty()) else {
        return;
    };
    env::set_var("XDG_ACTIVATION_TOKEN", token);
    env::set_var("DESKTOP_STARTUP_ID", token);
}

fn register_app_icon() {
    gtk4::Window::set_default_icon_name(APP_ICON);
    let Some(display) = gtk4::gdk::Display::default() else {
        return;
    };
    let theme = gtk4::IconTheme::for_display(&display);
    let dir = share_dir().join("icons/hicolor");
    if dir.is_dir() {
        theme.add_search_path(dir);
    }
}

fn show_window(action: &str, token: Option<String>, package: Option<PackageSession>) {
    gtk_remove_notification();
    apply_activation_token(token.as_deref());
    let action = normalize_action(action).to_string();
    let app = Application::builder()
        .application_id(APP_ID)
        .flags(ApplicationFlags::NON_UNIQUE)
        .build();
    use gtk4::gdk::prelude::Cast;
    use gtk4::prelude::{
        ApplicationExt, ApplicationExtManual, BoxExt, ButtonExt, GtkWindowExt, WidgetExt,
    };

    let token_clone = token.clone();
    let package = Rc::new(package);
    let package_keep = package.clone();
    app.connect_startup(|_| register_app_icon());
    app.connect_activate(move |app| {
        let pkg_root = package.as_ref().as_ref().map(|p| p.root.clone());
        let from_archive = pkg_root.is_some();
        let window = ApplicationWindow::builder()
            .application(app)
            .title(window_title("status"))
            .default_width(1024)
            .default_height(680)
            .build();
        window.set_icon_name(Some(APP_ICON));
        if let Some(tok) = token_clone.clone() {
            window.set_startup_id(&tok);
            window.connect_realize(move |w| {
                if let Ok(display) = w.display().downcast::<gdk4_wayland::WaylandDisplay>() {
                    display.set_startup_notification_id(&tok);
                }
            });
        }
        let title_label = Label::new(None);
        let header = HeaderBar::new();
        header.set_title_widget(Some(&title_label));
        // Theme CSD would otherwise draw a ~16px decoration icon we cannot size.
        header.set_decoration_layout(Some(":minimize,maximize,close"));
        let logo = Image::from_icon_name(APP_ICON);
        logo.set_pixel_size(32);
        logo.set_valign(Align::Center);
        header.pack_start(&logo);
        window.set_titlebar(Some(&header));

        let content = GtkBox::new(Orientation::Vertical, 8);
        content.set_valign(Align::Start);
        content.set_halign(Align::Fill);
        content.set_hexpand(true);
        content.set_margin_start(12);
        content.set_margin_end(12);
        content.set_margin_top(10);
        content.set_margin_bottom(10);
        let scrolled = ScrolledWindow::builder()
            .hscrollbar_policy(PolicyType::Never)
            .vscrollbar_policy(PolicyType::Automatic)
            .hexpand(true)
            .vexpand(true)
            .child(&content)
            .build();

        let source = Rc::new(RefCell::new(String::new()));
        let status_btn = Button::with_label("Status");
        let analyze_btn = Button::with_label("Analyze");
        let report_btn = Button::with_label("Report");
        let probe_btn = Button::with_label("Probe");
        let package_btn = Button::with_label("Package");
        let copy_btn = Button::with_label("Copy");
        let fix_btn = Button::with_label("Fix");
        let copied = Label::new(None);
        copied.add_css_class("dim-label");
        package_btn.set_sensitive(!from_archive);
        package_btn.set_tooltip_text(Some(
            "Write an evidence tarball and open its folder for a support ticket",
        ));
        probe_btn.set_tooltip_text(Some(
            "Read SPD5118 MR11 on kernel hubs (no password in a local session)",
        ));
        update_fix_button(&fix_btn, from_archive);

        let load = {
            let window = window.clone();
            let title_label = title_label.clone();
            let content = content.clone();
            let source = source.clone();
            let status_btn = status_btn.clone();
            let analyze_btn = analyze_btn.clone();
            let report_btn = report_btn.clone();
            let probe_btn = probe_btn.clone();
            let fix_btn = fix_btn.clone();
            let copied = copied.clone();
            let pkg_root = pkg_root.clone();
            Rc::new(move |action: &str| {
                let action = normalize_action(action).to_string();
                let (title, body) = if action == "probe" && pkg_root.is_none() {
                    live_probe_text()
                } else {
                    results_text(&action, pkg_root.as_deref())
                };
                window.set_title(Some(&title));
                title_label.set_text(&title);
                markdown::fill_box(&content, action != "status" && action != "probe", &body);
                *source.borrow_mut() = body;
                status_btn.set_sensitive(action != "status");
                analyze_btn.set_sensitive(action != "analyze");
                report_btn.set_sensitive(action != "report");
                probe_btn.set_sensitive(action != "probe");
                update_fix_button(&fix_btn, pkg_root.is_some());
                copied.set_text("");
            })
        };

        {
            let load = load.clone();
            status_btn.connect_clicked(move |_| load("status"));
        }
        {
            let load = load.clone();
            analyze_btn.connect_clicked(move |_| load("analyze"));
        }
        {
            let load = load.clone();
            report_btn.connect_clicked(move |_| load("report"));
        }
        {
            let load = load.clone();
            probe_btn.connect_clicked(move |_| load("probe"));
        }
        {
            let window = window.clone();
            let load = load.clone();
            let fix_btn_cb = fix_btn.clone();
            fix_btn.connect_clicked(move |_| {
                let load = load.clone();
                let fix_btn_cb = fix_btn_cb.clone();
                show_fix_dialog(
                    &window,
                    Rc::new(move || {
                        load("status");
                        update_fix_button(&fix_btn_cb, false);
                    }),
                );
            });
        }
        {
            let source = source.clone();
            let copied = copied.clone();
            let window = window.clone();
            copy_btn.connect_clicked(move |_| {
                window.clipboard().set_text(&source.borrow());
                copied.set_text("Copied");
            });
        }
        {
            let copied = copied.clone();
            let window = window.clone();
            package_btn.connect_clicked(move |_| {
                copied.set_tooltip_text(None::<&str>);
                let prefix = share_dir();
                env::set_var("AM5_SPD_DIAG_SHARE", &prefix);
                let cfg = load_config(&prefix);
                let state = cfg.state_dir();
                let events = load_timeline(&state);
                let ctx = build_context(&cfg, &events, &state);
                let dir = package_output_dir(&cfg);
                match make_package(&prefix, &state, &cfg, &events, &ctx, &dir, false) {
                    Ok(path) => {
                        copied.set_text("Packaged");
                        open_containing_folder(&window, &path);
                    }
                    Err(e) => show_package_error(&window, &copied, &e),
                }
            });
        }

        let footer = GtkBox::new(Orientation::Horizontal, 8);
        footer.set_margin_start(10);
        footer.set_margin_end(10);
        footer.set_margin_top(8);
        footer.set_margin_bottom(8);
        footer.append(&status_btn);
        footer.append(&analyze_btn);
        footer.append(&report_btn);
        footer.append(&probe_btn);
        let spacer = GtkBox::new(Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        footer.append(&spacer);
        footer.append(&copied);
        footer.append(&fix_btn);
        footer.append(&package_btn);
        footer.append(&copy_btn);

        let vbox = GtkBox::new(Orientation::Vertical, 0);
        vbox.append(&scrolled);
        vbox.append(&Separator::new(Orientation::Horizontal));
        vbox.append(&footer);
        window.set_child(Some(&vbox));
        load(&action);
        window.present();
    });
    app.run_with_args::<&str>(&[]);
    drop(package_keep);
}
