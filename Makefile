NAME        := am5-spd-diag
PREFIX      ?= /usr
DESTDIR     ?=
BINDIR      := $(PREFIX)/bin
LIBEXECDIR  := $(PREFIX)/libexec/$(NAME)
SHAREDIR    := $(PREFIX)/share/$(NAME)
DOCDIR      := $(PREFIX)/share/doc/$(NAME)
UNITDIR     := $(PREFIX)/lib/systemd/system
SLEEPDIR    := $(PREFIX)/lib/systemd/system-sleep
TMPFILESDIR := $(PREFIX)/lib/tmpfiles.d
POLKITDIR   := $(PREFIX)/share/polkit-1/actions
MANDIR      := $(PREFIX)/share/man/man1
APPDIR      := $(PREFIX)/share/applications
DBUSDIR     := $(PREFIX)/share/dbus-1/services
SYSCONFDIR  := /etc

INSTALL ?= install
INSTALL_PROGRAM = $(INSTALL) -m 0755
INSTALL_DATA    = $(INSTALL) -m 0644

VERSION     ?= 0.1.0

.PHONY: all test test-tool test-packaging install uninstall uninstall-purge dist

all:
	@true

test: test-tool test-packaging

test-packaging:
	python3 tests/test_changelogs.py
	python3 scripts/gen_changelogs.py --check
	cmp -s debian.control debian/control
	cmp -s debian.copyright debian/copyright
	cmp -s debian.rules debian/rules
	cmp -s debian.compat debian/compat

test-tool:
	python3 -m py_compile libexec/analyze.py libexec/spd_hub.py libexec/notify_app.py scripts/gen_changelogs.py tests/test_dmidecode.py tests/test_smbios.py tests/test_analyze.py tests/test_i2c_filter.py tests/test_changelogs.py tests/test_notify.py
	bash -n bin/$(NAME)
	bash -n libexec/capture.sh
	bash -n libexec/pkexec-snapshot
	bash -n libexec/open-term
	sh -n systemd/system-sleep/$(NAME)
	python3 tests/test_dmidecode.py
	python3 tests/test_smbios.py
	python3 tests/test_analyze.py
	python3 tests/test_i2c_filter.py
	python3 tests/test_notify.py
	python3 tests/make_fixture.py
	test -z "$$(python3 libexec/spd_hub.py flags tests/fixture/events/20260817T040000.0Z-boot/dimm-summary.txt)"
	python3 libexec/spd_hub.py flags tests/fixture/events/20260817T043030.0Z-boot/dimm-summary.txt | grep -q unknown_part
	python3 libexec/spd_hub.py summarize tests/dmidecode-healthy.txt | grep -q 'locator=DIMMA2'
	python3 libexec/spd_hub.py summarize tests/dmidecode-healthy.txt | grep -q 'part=CMH32GX5M2M6000Z36'
	test -z "$$(python3 libexec/spd_hub.py summarize tests/dmidecode-healthy.txt | python3 libexec/spd_hub.py flags -)"
	AM5_SPD_DIAG_STATE_DIR=tests/fixture bin/$(NAME) status | grep -q 'SPD now: corrupted'
	AM5_SPD_DIAG_STATE_DIR=tests/fixture bin/$(NAME) status | grep -q 'Monitor'
	AM5_SPD_DIAG_STATE_DIR=tests/fixture bin/$(NAME) analyze | grep -q 'SPD now: corrupted'
	AM5_SPD_DIAG_STATE_DIR=tests/fixture bin/$(NAME) analyze | grep -q 'Reproduction pattern'
	AM5_SPD_DIAG_STATE_DIR=tests/fixture bin/$(NAME) analyze | grep -q 'boot=warm_reboot'
	AM5_SPD_DIAG_STATE_DIR=tests/fixture bin/$(NAME) status | grep -q 'System:'
	AM5_SPD_DIAG_STATE_DIR=tests/fixture bin/$(NAME) analyze | grep -q 'System:'
	python3 libexec/analyze.py inventory | grep -q bios_version
	AM5_SPD_DIAG_STATE_DIR=tests/fixture bin/$(NAME) report --no-snapshot --out tests/fixture/report-out.md >/dev/null
	grep -q '2.A52' tests/fixture/report-out.md
	grep -q 'BIOS revision' tests/fixture/report-out.md
	grep -q 'Kernel' tests/fixture/report-out.md
	grep -q 'openSUSE Tumbleweed\|PRETTY_NAME\|OS' tests/fixture/report-out.md
	grep -q '### Current' tests/fixture/report-out.md
	grep -q 'Last healthy baseline' tests/fixture/report-out.md
	grep -q 'SPD hub evidence' tests/fixture/report-out.md
	grep -q '## Expected / Actual / Impact' tests/fixture/report-out.md
	grep -q '6000 MT/s' tests/fixture/report-out.md
	grep -q 'Board serial' tests/fixture/report-out.md
	grep -q 'System RAM' tests/fixture/report-out.md
	grep -q 'System RAM high range differs' tests/fixture/report-out.md
	grep -q 'Full firmware e820 table' tests/fixture/report-out.md
	grep -q 'reserved' tests/fixture/report-out.md
	grep -q 'dirty power blip' tests/fixture/report-out.md
	grep -q 'hub.json' tests/fixture/report-out.md
	grep -qE 'any OS|VDDSPD' tests/fixture/report-out.md
	! grep -qi 'serial numbers and uuids are not collected' tests/fixture/report-out.md
	! grep -q 'Last alert:' tests/fixture/report-out.md
	! grep -q 'Repro for engineering' tests/fixture/report-out.md
	! grep -q 'Failure pattern' tests/fixture/report-out.md
	! grep -qi 'to be filled by o.e.m' tests/fixture/report-out.md
	! grep -q 'DIMMs before (healthy)' tests/fixture/report-out.md
	sed 's|@HELPER@|/usr/libexec/am5-spd-diag/pkexec-snapshot|g' \
	  polkit/org.opensuse.am5-spd-diag.snapshot.policy.in | grep -q /usr/libexec/am5-spd-diag/pkexec-snapshot
	AM5_SPD_DIAG_STATE_DIR=tests/fixture bin/$(NAME) analyze | grep -q 'SPD5118 hub'
	! AM5_SPD_DIAG_STATE_DIR=tests/fixture bin/$(NAME) status | grep -q 'Reproduction pattern'
	! AM5_SPD_DIAG_STATE_DIR=tests/fixture bin/$(NAME) analyze | grep -q 'Monitor'
	bin/$(NAME) --help | grep -q 'am5-spd-diag <command>'
	bin/$(NAME) help status | grep -q 'right now'
	bin/$(NAME) help purge | grep -q 'Delete captured evidence'
	! bin/$(NAME) --help | grep -qE '  (install|uninstall|capture) '

install:
	$(INSTALL) -d $(DESTDIR)$(BINDIR)
	$(INSTALL) -d $(DESTDIR)$(LIBEXECDIR)
	$(INSTALL) -d $(DESTDIR)$(SHAREDIR)/config
	$(INSTALL) -d $(DESTDIR)$(SHAREDIR)/templates
	$(INSTALL) -d $(DESTDIR)$(DOCDIR)
	$(INSTALL) -d $(DESTDIR)$(UNITDIR)
	$(INSTALL) -d $(DESTDIR)$(SLEEPDIR)
	$(INSTALL) -d $(DESTDIR)$(TMPFILESDIR)
	$(INSTALL) -d $(DESTDIR)$(POLKITDIR)
	$(INSTALL) -d $(DESTDIR)$(MANDIR)
	$(INSTALL) -d $(DESTDIR)$(APPDIR)
	$(INSTALL) -d $(DESTDIR)$(DBUSDIR)
	$(INSTALL_PROGRAM) bin/$(NAME) $(DESTDIR)$(BINDIR)/$(NAME)
	$(INSTALL_PROGRAM) libexec/capture.sh $(DESTDIR)$(LIBEXECDIR)/capture.sh
	$(INSTALL_PROGRAM) libexec/analyze.py $(DESTDIR)$(LIBEXECDIR)/analyze.py
	$(INSTALL_PROGRAM) libexec/spd_hub.py $(DESTDIR)$(LIBEXECDIR)/spd_hub.py
	$(INSTALL_PROGRAM) libexec/pkexec-snapshot $(DESTDIR)$(LIBEXECDIR)/pkexec-snapshot
	$(INSTALL_PROGRAM) libexec/open-term $(DESTDIR)$(LIBEXECDIR)/open-term
	$(INSTALL_PROGRAM) libexec/notify_app.py $(DESTDIR)$(LIBEXECDIR)/notify-app
	sed 's|@LIBEXEC@|$(LIBEXECDIR)|g' share/applications/org.opensuse.am5spdDiag.desktop \
	  > $(DESTDIR)$(APPDIR)/org.opensuse.am5spdDiag.desktop
	sed 's|@LIBEXEC@|$(LIBEXECDIR)|g' share/dbus-1/services/org.opensuse.am5spdDiag.service \
	  > $(DESTDIR)$(DBUSDIR)/org.opensuse.am5spdDiag.service
	$(INSTALL_DATA) config/default.conf $(DESTDIR)$(SHAREDIR)/config/default.conf
	$(INSTALL_DATA) templates/ticket.md.tmpl $(DESTDIR)$(SHAREDIR)/templates/ticket.md.tmpl
	$(INSTALL_DATA) README.md $(DESTDIR)$(DOCDIR)/README.md
	$(INSTALL_DATA) LICENSE $(DESTDIR)$(DOCDIR)/LICENSE
	$(INSTALL_DATA) systemd/$(NAME).service $(DESTDIR)$(UNITDIR)/$(NAME).service
	$(INSTALL_DATA) systemd/$(NAME)-pre-sleep.service $(DESTDIR)$(UNITDIR)/$(NAME)-pre-sleep.service
	$(INSTALL_DATA) systemd/$(NAME)-post-sleep.service $(DESTDIR)$(UNITDIR)/$(NAME)-post-sleep.service
	$(INSTALL_PROGRAM) systemd/system-sleep/$(NAME) $(DESTDIR)$(SLEEPDIR)/$(NAME)
	$(INSTALL_DATA) man/am5-spd-diag.1 $(DESTDIR)$(MANDIR)/am5-spd-diag.1
	$(INSTALL_DATA) systemd/$(NAME).tmpfiles.conf $(DESTDIR)$(TMPFILESDIR)/$(NAME).conf
	sed 's|@HELPER@|$(LIBEXECDIR)/pkexec-snapshot|g' \
	  polkit/org.opensuse.am5-spd-diag.snapshot.policy.in \
	  > $(DESTDIR)$(POLKITDIR)/org.opensuse.am5-spd-diag.snapshot.policy
	if [ ! -f $(DESTDIR)$(SYSCONFDIR)/$(NAME).conf ]; then \
	  $(INSTALL) -d $(DESTDIR)$(SYSCONFDIR); \
	  $(INSTALL_DATA) config/default.conf $(DESTDIR)$(SYSCONFDIR)/$(NAME).conf; \
	fi
	if [ -z "$(DESTDIR)" ]; then \
	  systemd-tmpfiles --create $(TMPFILESDIR)/$(NAME).conf; \
	  chcon -t bin_t $(SLEEPDIR)/$(NAME) 2>/dev/null || true; \
	  systemctl daemon-reload; \
	  systemctl enable --now $(NAME).service; \
	  systemctl enable $(NAME)-pre-sleep.service; \
	  systemctl enable $(NAME)-post-sleep.service; \
	fi

uninstall:
	-systemctl disable --now $(NAME).service 2>/dev/null
	-systemctl disable --now $(NAME)-pre-sleep.service 2>/dev/null
	-systemctl disable --now $(NAME)-post-sleep.service 2>/dev/null
	-rm -f $(DESTDIR)$(BINDIR)/$(NAME)
	-rm -rf $(DESTDIR)$(LIBEXECDIR) $(DESTDIR)$(SHAREDIR) $(DESTDIR)$(DOCDIR)
	-rm -f $(DESTDIR)$(UNITDIR)/$(NAME).service \
	      $(DESTDIR)$(UNITDIR)/$(NAME)-pre-sleep.service \
	      $(DESTDIR)$(UNITDIR)/$(NAME)-post-sleep.service
	-rm -f $(DESTDIR)$(SLEEPDIR)/$(NAME)
	-rm -f $(DESTDIR)$(TMPFILESDIR)/$(NAME).conf
	-rm -f $(DESTDIR)$(POLKITDIR)/org.opensuse.am5-spd-diag.snapshot.policy
	-rm -f $(DESTDIR)$(MANDIR)/am5-spd-diag.1
	-rm -f $(DESTDIR)$(APPDIR)/$(NAME).desktop
	-rm -f $(DESTDIR)$(APPDIR)/org.opensuse.am5spdDiag.desktop
	-rm -f $(DESTDIR)$(DBUSDIR)/org.opensuse.am5spdDiag.service
	if [ -z "$(DESTDIR)" ]; then systemctl daemon-reload; fi
	@echo "Logs kept at /var/log/$(NAME) (am5-spd-diag purge, or make uninstall-purge)"

uninstall-purge: uninstall
	-rm -rf /var/log/$(NAME)
	-rm -f $(DESTDIR)$(SYSCONFDIR)/$(NAME).conf

dist:
	tar -C .. -cJf $(NAME)-$(VERSION).tar.xz \
	  --exclude=.osc --exclude='*.tar.xz' --exclude=debian \
	  --exclude=__pycache__ --exclude='*.pyc' \
	  --exclude=$(NAME).spec --exclude=$(NAME).changes \
	  --transform 's,^$(NAME),$(NAME)-$(VERSION),' \
	  $(NAME)
