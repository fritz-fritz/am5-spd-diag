//! Pick GTK 4.10 dialogs at runtime when this binary was built against them.
//!
//! `gtk4-sys` refuses `v4_10` unless pkg-config gtk4 is ≥ 4.10, so Debian 12
//! builds with `--no-default-features` (Makefile) and only compiles
//! [`gtk4::MessageDialog`]. Newer OBS chroots and `cargo build` keep the
//! default `gtk4_v4_10` feature. Runtime still checks
//! [`gtk4::check_version`] so a 4.10-linked binary would not call missing
//! symbols if it were ever copied onto older libgtk.

use gtk4::gio;
use gtk4::ApplicationWindow;
use std::cell::RefCell;
use std::path::{Path, PathBuf};

/// True when libgtk in this process is at least `major.minor`.
pub fn gtk_runtime_at_least(major: u32, minor: u32) -> bool {
    gtk4::check_version(major, minor, 0).is_none()
}

/// Use AlertDialog / FileLauncher: compiled with 4.10 bindings and running
/// on GTK 4.10+.
pub fn use_gtk4_10_apis() -> bool {
    cfg!(feature = "gtk4_v4_10") && gtk_runtime_at_least(4, 10)
}

/// Directories to pass to [`gtk4::IconTheme::add_search_path`].
///
/// GTK treats each path as a *parent of themes* (`hicolor/`, `Adwaita/`),
/// not as the `hicolor` directory itself.
pub fn icon_theme_search_dirs(share: &Path) -> Vec<PathBuf> {
    let icons = share.join("icons");
    if icons.join("hicolor").is_dir() {
        vec![icons]
    } else {
        Vec::new()
    }
}

pub fn show_message(
    parent: &ApplicationWindow,
    kind: gtk4::MessageType,
    message: &str,
    detail: &str,
) {
    if use_gtk4_10_apis() {
        #[cfg(feature = "gtk4_v4_10")]
        {
            let dialog = gtk4::AlertDialog::builder()
                .modal(true)
                .message(message)
                .detail(detail)
                .build();
            dialog.show(Some(parent));
            return;
        }
    }
    show_message_legacy(parent, kind, message, detail);
}

pub fn confirm_fix<F>(parent: &ApplicationWindow, title: &str, detail: &str, on_response: F)
where
    F: FnOnce(bool) + 'static,
{
    if use_gtk4_10_apis() {
        #[cfg(feature = "gtk4_v4_10")]
        {
            let dialog = gtk4::AlertDialog::builder()
                .modal(true)
                .message(title)
                .detail(detail)
                .buttons(["Cancel", "Fix"])
                .cancel_button(0)
                .default_button(1)
                .build();
            dialog.choose(Some(parent), None::<&gio::Cancellable>, move |result| {
                on_response(result.ok() == Some(1));
            });
            return;
        }
    }
    confirm_fix_legacy(parent, title, detail, on_response);
}

pub fn open_containing_folder(parent: &ApplicationWindow, file_path: &Path) {
    if use_gtk4_10_apis() {
        #[cfg(feature = "gtk4_v4_10")]
        {
            let file = gio::File::for_path(file_path);
            let launcher = gtk4::FileLauncher::new(Some(&file));
            launcher.open_containing_folder(Some(parent), None::<&gio::Cancellable>, |_| {});
            return;
        }
    }
    open_containing_folder_legacy(parent, file_path);
}

#[allow(deprecated)]
fn show_message_legacy(
    parent: &ApplicationWindow,
    kind: gtk4::MessageType,
    message: &str,
    detail: &str,
) {
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

#[allow(deprecated)]
fn confirm_fix_legacy<F>(parent: &ApplicationWindow, title: &str, detail: &str, on_response: F)
where
    F: FnOnce(bool) + 'static,
{
    use gtk4::prelude::{DialogExt, GtkWindowExt};
    let dialog = gtk4::MessageDialog::builder()
        .transient_for(parent)
        .modal(true)
        .message_type(gtk4::MessageType::Warning)
        .text(title)
        .secondary_text(detail)
        .build();
    dialog.add_button("Cancel", gtk4::ResponseType::Cancel);
    dialog.add_button("Fix", gtk4::ResponseType::Accept);
    let on_response = RefCell::new(Some(on_response));
    dialog.connect_response(move |d, response| {
        d.close();
        if let Some(cb) = on_response.borrow_mut().take() {
            cb(response == gtk4::ResponseType::Accept);
        }
    });
    dialog.present();
}

#[allow(deprecated)]
fn open_containing_folder_legacy(parent: &ApplicationWindow, file_path: &Path) {
    use gtk4::gio::prelude::FileExt;
    let dir = file_path.parent().unwrap_or(file_path);
    let uri = gio::File::for_path(dir).uri();
    gtk4::show_uri(Some(parent), uri.as_str(), 0);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn runtime_version_probe_does_not_panic() {
        let _ = gtk_runtime_at_least(4, 8);
        let _ = gtk_runtime_at_least(4, 10);
        assert_eq!(
            use_gtk4_10_apis(),
            cfg!(feature = "gtk4_v4_10") && gtk_runtime_at_least(4, 10)
        );
    }

    #[test]
    fn icon_search_dir_is_theme_parent_not_hicolor() {
        let root = std::env::temp_dir().join(format!(
            "am5-icon-theme-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(root.join("icons/hicolor/48x48/apps")).unwrap();
        let dirs = icon_theme_search_dirs(&root);
        assert_eq!(dirs, vec![root.join("icons")]);
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn icon_search_dir_empty_without_hicolor() {
        let root = std::env::temp_dir().join(format!(
            "am5-icon-missing-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        assert!(icon_theme_search_dirs(&root).is_empty());
        fs::remove_dir_all(&root).ok();
    }
}
