#
# spec file for package am5-spd-diag
#
# Copyright (c) 2026 SUSE LLC and contributors
#
# All modifications and additions to the file contributed by third parties
# remain the property of their copyright owners, unless otherwise agreed
# upon. The license for this file, and modifications and additions to the
# file, is the same license as for the pristine package itself (unless the
# license for the pristine package is not an Open Source License, in which
# case the license is the MIT License). An "Open Source License" is a
# license that conforms to the Open Source Definition (Version 1.9)
# published by the Open Source Initiative.

# Please submit bugfixes or comments via https://github.com/fritz-fritz/am5-spd-diag/issues
#


Name:           am5-spd-diag
Version:        1.0.1
Release:        0
Summary:        AM5 DDR5 SPD hub diagnostics after sleep/warm reboot
License:        MIT
Group:          System/Monitoring
URL:            https://github.com/fritz-fritz/am5-spd-diag
Source0:        %{name}-%{version}.tar.xz
Source1:        %{name}-%{version}-vendor.tar.zst
# Official rustc 1.92 (build-time only). Fetched by make osc-fetch-rust / GHA.
Source2:        rust-1.92.0-x86_64-unknown-linux-gnu.tar.xz
ExclusiveArch:  x86_64
BuildRequires:  cargo
BuildRequires:  gcc
BuildRequires:  gtk4-devel
BuildRequires:  make
BuildRequires:  pkgconfig
BuildRequires:  pkgconfig(gtk4)
BuildRequires:  python3
BuildRequires:  rust
BuildRequires:  systemd-rpm-macros
BuildRequires:  xz
BuildRequires:  zstd
Requires:       systemd
Recommends:     dmidecode
Recommends:     i2c-tools
Recommends:     polkit
Requires(post): systemd
Requires(preun): systemd
Requires(postun): systemd

%description
Capture DIMM/SPD identity across boot, shutdown, and systemd sleep/resume
on AMD AM5 systems. Helps document the AGESA/firmware failure where a
warm reboot after sleep reports Unknown/2 GiB DIMMs until AC power is
removed.

%prep
%setup -q

%build
# Source0 has no vendor/. Extract Source1 and install Source2 rustc to
# /tmp/am5-rust (path without ':') so every OBS chroot uses rustc 1.92.
sh %{_builddir}/%{name}-%{version}/scripts/obs_prep.sh %{SOURCE1} %{SOURCE2}
export PATH=/tmp/am5-rust/bin:$PATH
export CARGO_HOME=%{_builddir}/%{name}-%{version}/.cargo-home
%make_build

%install
# openSUSE %{_docdir} is /usr/share/doc/packages; Fedora is /usr/share/doc.
%make_install PREFIX=%{_prefix} DOCDIR=%{_docdir}/%{name}

%check
export PATH=/tmp/am5-rust/bin:$PATH
export CARGO_HOME=%{_builddir}/%{name}-%{version}/.cargo-home
%make_build test

%preun
%systemd_preun am5-spd-diag.service am5-spd-diag-pre-sleep.service am5-spd-diag-post-sleep.service

%post
%tmpfiles_create %{_tmpfilesdir}/am5-spd-diag.conf
%systemd_post am5-spd-diag.service am5-spd-diag-pre-sleep.service am5-spd-diag-post-sleep.service

%postun
%systemd_postun_with_restart am5-spd-diag.service

%files
%dir %{_docdir}/%{name}
%license %{_docdir}/%{name}/LICENSE
%doc %{_docdir}/%{name}/README.md
%{_bindir}/am5-spd-diag
%dir %{_libexecdir}/am5-spd-diag
%{_libexecdir}/am5-spd-diag/pkexec-snapshot
%{_libexecdir}/am5-spd-diag/pkexec-probe
%{_libexecdir}/am5-spd-diag/pkexec-recover
%{_libexecdir}/am5-spd-diag/am5-spd-diag-notify
%{_datadir}/am5-spd-diag/
%{_datadir}/applications/org.opensuse.am5spdDiag.desktop
%{_datadir}/icons/hicolor/48x48/apps/org.opensuse.am5spdDiag.png
%{_datadir}/icons/hicolor/128x128/apps/org.opensuse.am5spdDiag.png
%{_datadir}/icons/hicolor/256x256/apps/org.opensuse.am5spdDiag.png
%{_datadir}/dbus-1/services/org.opensuse.am5spdDiag.service
# Leap 16 does not own these directories on the polkit package.
%dir %{_datadir}/polkit-1
%dir %{_datadir}/polkit-1/actions
%{_datadir}/polkit-1/actions/org.opensuse.am5-spd-diag.snapshot.policy
%dir %{_datadir}/polkit-1/rules.d
%{_datadir}/polkit-1/rules.d/org.opensuse.am5-spd-diag.rules
%{_unitdir}/am5-spd-diag.service
%{_unitdir}/am5-spd-diag-pre-sleep.service
%{_unitdir}/am5-spd-diag-post-sleep.service
# systemd does not own this directory on openSUSE; post-build-checks requires it.
%dir %{_prefix}/lib/systemd/system-sleep
%{_prefix}/lib/systemd/system-sleep/%{name}
%{_tmpfilesdir}/am5-spd-diag.conf
%{_mandir}/man1/am5-spd-diag.1*
%config(noreplace) %{_sysconfdir}/am5-spd-diag.conf

%changelog
* Thu Aug 20 2026 Fritz <code@fritztech.net> - 1.0.1
- Split OBS sources and pin official rustc 1.92 so older distros can build.
  Not a Ghost DIMM behavior change.

* Wed Aug 19 2026 Fritz <code@fritztech.net> - 1.0.1
- Make /var/log/am5-spd-diag 0755 root:root and write capture state with
  O_NOFOLLOW so a local user cannot symlink-swap passwordless snapshot into
  a root file write.
- Recursively restore leftover files in that tree to root:root via tmpfiles
  Z. Purge wipes /var/log/am5-spd-diag with systemd-tmpfiles --remove on a
  snippet kept out of tmpfiles.d (so boot does not empty logs), then removes
  user XDG data so a failed sudo leaves reports in place. Ignore
  AM5_SPD_DIAG_STATE_DIR after sudo. The log tree stays world-readable so
  users can inspect captures without root; package still copies evidence
  into a tarball they own.
- Return a real exit status from capture so report/package refuse stale
  evidence; systemd boot/sleep units stay best-effort.

* Wed Aug 19 2026 Fritz <code@fritztech.net> - 1.0.1
- First stable release (Rust rewrite of the Python/bash tool).

* Wed Aug 19 2026 Fritz <code@fritztech.net> - 1.0.1
- Show Ghost DIMM as the desktop notice sender (keep the D-Bus app id).
- Rename the in-band command to fix; keep recover as an alias. CLI fix uses
  pkexec like Probe. A successful clear no longer re-fires the corruption
  notice.
- Run make test in RPM %%check and Debian dh_auto_test so OBS builds both
  package types.

* Tue Aug 18 2026 Fritz <code@fritztech.net> - 1.0.1
- Add an application icon and a visible desktop launcher for the GTK window.
  Corruption notices still use dialog-warning as the main image; the logo
  comes from the desktop file.
- Reply to D-Bus Activate before closing the session name so Plasma does not
  show "Launching AM5 SPD diagnostics (Failed)" on menu start.

* Tue Aug 18 2026 Fritz <code@fritztech.net> - 1.0.1
- Rewrite the tool in Rust: one x86_64 CLI binary plus a GTK notify window.
  Capture schema, systemd units, and D-Bus app id are unchanged.
- Drop Python/bash runtime helpers and the terminal launcher. Notification
  clicks open am5-spd-diag-notify. Package is ExclusiveArch x86_64 with
  vendored Cargo crates and gtk4-devel.

* Mon Aug 17 2026 Fritz <code@fritztech.net> - 1.0.1
- Derive Debian changelog from am5-spd-diag.changes so RPM and Debian
  packages share the same history.
- Fix Debian package metadata (maintainer, description, copyright, Homepage,
  and recommends).

* Mon Aug 17 2026 Fritz <code@fritztech.net> - 1.0.1
- Own /usr/lib/systemd/system-sleep so openSUSE post-build-checks does not
  fail. Drop duplicate share-dir listing.

* Mon Aug 17 2026 Fritz <code@fritztech.net> - 1.0.1
- Install docs into %%{_docdir} so openSUSE finds LICENSE/README.
- Add Debian/Ubuntu OBS sources (dsc + debian.*) so those repos are not
  excluded.

* Mon Aug 17 2026 Fritz <code@fritztech.net> - 1.0.1
- Initial package 0.1.0: AM5 DDR5 SPD hub diagnostics after sleep and warm
  reboot.
