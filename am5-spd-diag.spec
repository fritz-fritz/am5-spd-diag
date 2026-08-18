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

# Please submit bugfixes or comments via https://bugs.opensuse.org/
#


Name:           am5-spd-diag
Version:        0.1.0
Release:        0
Summary:        AM5 DDR5 SPD hub diagnostics after sleep/warm reboot
License:        MIT
Group:          System/Monitoring
URL:            https://build.opensuse.org/package/show/home:fritz-fritz/am5-spd-diag
Source0:        %{name}-%{version}.tar.xz
BuildArch:      noarch
BuildRequires:  make
BuildRequires:  systemd-rpm-macros
Requires:       python3-base
Requires:       systemd
Recommends:     dmidecode
Recommends:     i2c-tools
Recommends:     polkit
Recommends:     glow
Recommends:     python3-gobject
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
# nothing to compile

%install
# openSUSE %{_docdir} is /usr/share/doc/packages; Fedora is /usr/share/doc.
%make_install PREFIX=%{_prefix} DOCDIR=%{_docdir}/%{name}

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
%{_libexecdir}/am5-spd-diag/capture.sh
%{_libexecdir}/am5-spd-diag/analyze.py
%{_libexecdir}/am5-spd-diag/spd_hub.py
%{_libexecdir}/am5-spd-diag/pkexec-snapshot
%{_libexecdir}/am5-spd-diag/open-term
%{_libexecdir}/am5-spd-diag/notify-app
%{_datadir}/am5-spd-diag/
%{_datadir}/applications/org.opensuse.am5spdDiag.desktop
%{_datadir}/dbus-1/services/org.opensuse.am5spdDiag.service
%{_datadir}/polkit-1/actions/org.opensuse.am5-spd-diag.snapshot.policy
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
* Mon Aug 17 2026 Fritz <code@fritztech.net> - 0.1.0
- Derive Debian changelog from am5-spd-diag.changes so RPM and Debian
  packages share the same history.
- Fix Debian package metadata (maintainer, description, copyright, Homepage,
  and recommends).

* Mon Aug 17 2026 Fritz <code@fritztech.net> - 0.1.0
- Own /usr/lib/systemd/system-sleep so openSUSE post-build-checks does not
  fail. Drop duplicate share-dir listing.

* Mon Aug 17 2026 Fritz <code@fritztech.net> - 0.1.0
- Install docs into %%{_docdir} so openSUSE finds LICENSE/README.
- Add Debian/Ubuntu OBS sources (dsc + debian.*) so those repos are not
  excluded.

* Mon Aug 17 2026 Fritz <code@fritztech.net> - 0.1.0
- Initial package 0.1.0: AM5 DDR5 SPD hub diagnostics after sleep and warm
  reboot.
