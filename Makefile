NAME        := am5-spd-diag
PREFIX      ?= /usr
DESTDIR     ?=
BINDIR      := $(PREFIX)/bin
LIBEXECDIR  := $(PREFIX)/libexec/$(NAME)
SHAREDIR    := $(PREFIX)/share/$(NAME)
DOCDIR      := $(PREFIX)/share/doc/$(NAME)
UNITDIR     := $(PREFIX)/lib/systemd/system
PRESETDIR   := $(PREFIX)/lib/systemd/system-preset
SLEEPDIR    := $(PREFIX)/lib/systemd/system-sleep
TMPFILESDIR := $(PREFIX)/lib/tmpfiles.d
POLKITDIR   := $(PREFIX)/share/polkit-1/actions
POLKITRULESDIR := $(PREFIX)/share/polkit-1/rules.d
MANDIR      := $(PREFIX)/share/man/man1
APPDIR      := $(PREFIX)/share/applications
ICONDIR     := $(PREFIX)/share/icons/hicolor
DBUSDIR     := $(PREFIX)/share/dbus-1/services
SYSCONFDIR  := /etc

INSTALL ?= install
INSTALL_PROGRAM = $(INSTALL) -m 0755
INSTALL_DATA    = $(INSTALL) -m 0644

VERSION     ?= 1.0.6

# OBS project directories contain ':'; rustc rejects that in LD_LIBRARY_PATH.
# Use a path without ':' and without $HOME so `sudo make install` finds the
# binaries the user built (sudo resets PATH and HOME).
ifneq ($(findstring :,$(CURDIR)),)
CARGO_TARGET_DIR ?= /tmp/am5-spd-diag-target
export CARGO_TARGET_DIR
endif

CARGO ?= cargo
CARGOFLAGS ?=
ifneq ($(wildcard vendor),)
CARGOFLAGS += --offline --config net.offline=true
endif
# gtk4-sys will not enable v4_10 unless the chroot's gtk4 is ≥ 4.10.
# Debian 12 is 4.8; keep MessageDialog there. Newer distros keep AlertDialog.
ifneq ($(shell pkg-config --exists 'gtk4 >= 4.10' 2>/dev/null && echo yes),yes)
CARGOFLAGS += --no-default-features
endif

BIN_DEBUG = $(or $(CARGO_TARGET_DIR),target)/debug/$(NAME)
BIN_RELEASE = $(or $(CARGO_TARGET_DIR),target)/release/$(NAME)
NOTIFY_RELEASE = $(or $(CARGO_TARGET_DIR),target)/release/$(NAME)-notify

MAKEFILE_DIR := $(dir $(abspath $(lastword $(MAKEFILE_LIST))))
DIST_PARENT ?= $(abspath $(CURDIR)/..)
RUST_DIST_TXT := $(MAKEFILE_DIR)obs/rust-dist.txt
REPO ?= openSUSE_Tumbleweed
ARCH ?= x86_64
OSC_REPOS ?= openSUSE_Tumbleweed openSUSE_Slowroll 16.0 Fedora_44 Fedora_43 \
	xUbuntu_26.04 xUbuntu_25.10 xUbuntu_25.04 xUbuntu_24.10 xUbuntu_24.04 \
	Debian_Testing Debian_13 Debian_12

.PHONY: build test test-tool test-packaging bump bump-check rust-pin install uninstall uninstall-purge dist vendor \
	osc-fetch-rust osc-build osc-matrix osc-meta print-osc-repos

build:
	$(CARGO) build --release -p am5-spd-diag $(CARGOFLAGS)
	$(CARGO) build --release -p am5-spd-diag-notify $(CARGOFLAGS)

test: test-tool test-packaging

bump:
	@test -n "$(TO)" || { echo "usage: make bump TO=1.0.0 MSG='...'"; exit 1; }
	python3 scripts/bump_version.py $(TO) $(if $(MSG),-m "$(MSG)")

bump-check:
	python3 scripts/bump_version.py --check $(TO)

rust-pin:
	@test -n "$(TO)" || { echo "usage: make rust-pin TO=1.97.0"; exit 1; }
	$(MAKEFILE_DIR)scripts/sync_rust_pin.sh $(TO)

test-packaging:
	python3 tests/test_changelogs.py
	python3 tests/test_bump_version.py
	python3 scripts/bump_version.py --check
	python3 scripts/check_rust_pin.py
	python3 -m py_compile scripts/bump_version.py scripts/gen_changelogs.py \
	  scripts/obs_wait.py scripts/obs_release.py scripts/obs_commit_msg.py \
	  scripts/release_notes.py scripts/check_rust_pin.py
	sh -n scripts/obs_prep.sh
	sh -n scripts/osc_fetch_rust.sh
	sh -n scripts/rust_pin.sh
	sh -n scripts/sync_rust_pin.sh
	bash -n scripts/osc_build.sh
	python3 scripts/release_notes.py --version dummy --sha256 deadbeef | grep -q 'OBS download page'
	if [ -f am5-spd-diag.changes ]; then python3 scripts/gen_changelogs.py --check; fi
	grep -q "gtk4 >= 4.10" Makefile
	grep -q 'default = \["gtk4_v4_10"\]' crates/am5-spd-diag-notify/Cargo.toml
	if [ -d debian ]; then \
	  cmp -s debian.control debian/control && \
	  cmp -s debian.copyright debian/copyright && \
	  cmp -s debian.rules debian/rules && \
	  cmp -s debian.compat debian/compat; \
	fi
	grep -q '^enable am5-spd-diag.service$$' systemd/50-$(NAME).preset
	grep -q '^enable am5-spd-diag-pre-sleep.service$$' systemd/50-$(NAME).preset
	grep -q '^enable am5-spd-diag-post-sleep.service$$' systemd/50-$(NAME).preset
	grep -q '%service_add_post am5-spd-diag.service' $(NAME).spec
	grep -q '%service_del_preun am5-spd-diag.service' $(NAME).spec
	grep -q '%service_del_postun_without_restart am5-spd-diag.service' $(NAME).spec
	! grep -q '%systemd_postun_with_restart' $(NAME).spec
	grep -q 'system-preset/50-%{name}.preset' $(NAME).spec
	grep -q 'is-active --quiet am5-spd-diag.service' $(NAME).spec
	grep -q 'systemctl start am5-spd-diag.service' $(NAME).spec
	! grep -q -- '--no-enable' debian.rules
	grep -q '^d= /var/log/am5-spd-diag 0755 root root -$$' systemd/$(NAME).tmpfiles.conf
	grep -q '^d= /var/log/am5-spd-diag/events 0755 root root -$$' systemd/$(NAME).tmpfiles.conf
	grep -q '^d= /var/log/am5-spd-diag/latest 0755 root root -$$' systemd/$(NAME).tmpfiles.conf
	grep -q '^Z /var/log/am5-spd-diag ~0755 root root -$$' systemd/$(NAME).tmpfiles.conf
	grep -q '^R /var/log/am5-spd-diag$$' systemd/$(NAME)-purge.conf
	! grep -q '^R ' systemd/$(NAME).tmpfiles.conf
	ROOT=$$(mktemp -d); \
	  trap 'rm -rf "$$ROOT"' EXIT; \
	  mkdir -p "$$ROOT/var/log/$(NAME)"; \
	  ln -s /tmp "$$ROOT/var/log/$(NAME)/events"; \
	  ln -s /tmp "$$ROOT/var/log/$(NAME)/latest"; \
	  systemd-tmpfiles --create --root="$$ROOT" - < systemd/$(NAME).tmpfiles.conf >/dev/null 2>&1 || true; \
	  test -d "$$ROOT/var/log/$(NAME)/events" && test ! -L "$$ROOT/var/log/$(NAME)/events"; \
	  test -d "$$ROOT/var/log/$(NAME)/latest" && test ! -L "$$ROOT/var/log/$(NAME)/latest"

test-tool:
	$(CARGO) test -p am5-spd-diag $(CARGOFLAGS)
	$(CARGO) build -p am5-spd-diag $(CARGOFLAGS)
	$(CARGO) test -p am5-spd-diag-notify $(CARGOFLAGS)
	$(CARGO) build -p am5-spd-diag-notify $(CARGOFLAGS)
	make test-cli BIN=$(BIN_DEBUG)
	test -x $(or $(CARGO_TARGET_DIR),target)/debug/$(NAME)-notify
	$(or $(CARGO_TARGET_DIR),target)/debug/$(NAME)-notify --notify >/dev/null 2>&1; test $$? -eq 2

test-cli:
	test -n "$(BIN)"
	test -z "$$($(BIN) flags tests/fixture/events/20260817T040000.0Z-boot/dimm-summary.txt)"
	$(BIN) flags tests/fixture/events/20260817T043030.0Z-boot/dimm-summary.txt | grep -q unknown_part
	$(BIN) summarize tests/dmidecode-healthy.txt | grep -q 'locator=DIMMA2'
	$(BIN) summarize tests/dmidecode-healthy.txt | grep -q 'part=CMH32GX5M2M6000Z36'
	test -z "$$($(BIN) summarize tests/dmidecode-healthy.txt | $(BIN) flags -)"
	AM5_SPD_DIAG_SHARE=$(CURDIR) AM5_SPD_DIAG_STATE_DIR=tests/fixture $(BIN) status | grep -q 'SPD now: corrupted'
	AM5_SPD_DIAG_SHARE=$(CURDIR) AM5_SPD_DIAG_STATE_DIR=tests/fixture $(BIN) status | grep -q 'Monitor'
	AM5_SPD_DIAG_SHARE=$(CURDIR) AM5_SPD_DIAG_STATE_DIR=tests/fixture $(BIN) analyze | grep -q 'SPD now: corrupted'
	AM5_SPD_DIAG_SHARE=$(CURDIR) AM5_SPD_DIAG_STATE_DIR=tests/fixture $(BIN) analyze | grep -q 'Reproduction pattern'
	AM5_SPD_DIAG_SHARE=$(CURDIR) AM5_SPD_DIAG_STATE_DIR=tests/fixture $(BIN) analyze | grep -q 'boot=warm_reboot'
	AM5_SPD_DIAG_SHARE=$(CURDIR) AM5_SPD_DIAG_STATE_DIR=tests/fixture $(BIN) status | grep -q 'System:'
	AM5_SPD_DIAG_SHARE=$(CURDIR) AM5_SPD_DIAG_STATE_DIR=tests/fixture $(BIN) analyze | grep -q 'System'
	AM5_SPD_DIAG_SHARE=$(CURDIR) AM5_SPD_DIAG_STATE_DIR=tests/fixture $(BIN) analyze | grep -q '| Board |'
	$(BIN) inventory | grep -q bios_version
	mkdir -p tests/fixture/reports
	AM5_SPD_DIAG_SHARE=$(CURDIR) AM5_SPD_DIAG_STATE_DIR=tests/fixture $(BIN) report --no-snapshot --out tests/fixture/reports/report-out.md > tests/fixture/reports/cli.out
	grep -q '2.A52' tests/fixture/reports/report-out.md
	grep -q 'BIOS revision' tests/fixture/reports/report-out.md
	grep -q 'Kernel' tests/fixture/reports/report-out.md
	grep -q 'openSUSE Tumbleweed\|PRETTY_NAME\|OS' tests/fixture/reports/report-out.md
	grep -q '### Current' tests/fixture/reports/report-out.md
	grep -q 'Last healthy baseline' tests/fixture/reports/report-out.md
	grep -q 'SPD hub evidence' tests/fixture/reports/report-out.md
	grep -q '## Expected / Actual / Impact' tests/fixture/reports/report-out.md
	grep -q '6000 MT/s' tests/fixture/reports/report-out.md
	grep -q 'Board serial' tests/fixture/reports/report-out.md
	grep -q 'System RAM' tests/fixture/reports/report-out.md
	grep -q 'System RAM high range differs' tests/fixture/reports/report-out.md
	grep -q 'Full firmware e820 table' tests/fixture/reports/report-out.md
	grep -q 'reserved' tests/fixture/reports/report-out.md
	grep -q 'dirty power blip' tests/fixture/reports/report-out.md
	grep -q 'hub.json' tests/fixture/reports/report-out.md
	grep -qE 'any OS|VDDSPD' tests/fixture/reports/report-out.md
	grep -q 'am5-spd-diag fix' tests/fixture/reports/report-out.md
	grep -q 'page & 7' tests/fixture/reports/report-out.md
	! grep -qi 'serial numbers and uuids are not collected' tests/fixture/reports/report-out.md
	! grep -q 'Last alert:' tests/fixture/reports/report-out.md
	! grep -q 'Repro for engineering' tests/fixture/reports/report-out.md
	! grep -q 'Failure pattern' tests/fixture/reports/report-out.md
	! grep -qi 'to be filled by o.e.m' tests/fixture/reports/report-out.md
	! grep -q 'DIMMs before (healthy)' tests/fixture/reports/report-out.md
	! grep -q 'remote debug checklist' tests/fixture/reports/report-out.md
	grep -q '2.A52' tests/fixture/reports/cli.out
	grep -q 'report-out.md' tests/fixture/reports/cli.out
	sed 's|@HELPER@|/usr/libexec/am5-spd-diag/pkexec-snapshot|g; s|@LIBEXEC@|/usr/libexec/am5-spd-diag|g' \
	  polkit/org.opensuse.am5-spd-diag.snapshot.policy.in | grep -q /usr/libexec/am5-spd-diag/pkexec-snapshot
	sed 's|@HELPER@|/usr/libexec/am5-spd-diag/pkexec-snapshot|g; s|@LIBEXEC@|/usr/libexec/am5-spd-diag|g' \
	  polkit/org.opensuse.am5-spd-diag.snapshot.policy.in | grep -q /usr/libexec/am5-spd-diag/pkexec-probe
	sed 's|@HELPER@|/usr/libexec/am5-spd-diag/pkexec-snapshot|g; s|@LIBEXEC@|/usr/libexec/am5-spd-diag|g' \
	  polkit/org.opensuse.am5-spd-diag.snapshot.policy.in | grep -q /usr/libexec/am5-spd-diag/pkexec-recover
	grep -q 'allow_active>yes' polkit/org.opensuse.am5-spd-diag.snapshot.policy.in
	grep -q 'org.opensuse.am5-spd-diag.snapshot' polkit/org.opensuse.am5-spd-diag.rules
	grep -A8 'org.opensuse.am5-spd-diag.recover' polkit/org.opensuse.am5-spd-diag.snapshot.policy.in | grep -q 'auth_admin'
	grep -q 'experimental fix' polkit/org.opensuse.am5-spd-diag.snapshot.policy.in
	grep -q '^Icon=org.opensuse.am5spdDiag$$' share/applications/org.opensuse.am5spdDiag.desktop
	! grep -q '^NoDisplay=' share/applications/org.opensuse.am5spdDiag.desktop
	test -f icons/ghost-peek.png
	test -f icons/ghost-dimm.png
	test -f icons/ghost-glyph.png
	test -f icons/hicolor/48x48/apps/org.opensuse.am5spdDiag.png
	test -f icons/hicolor/128x128/apps/org.opensuse.am5spdDiag.png
	test -f icons/hicolor/256x256/apps/org.opensuse.am5spdDiag.png
	AM5_SPD_DIAG_SHARE=$(CURDIR) AM5_SPD_DIAG_STATE_DIR=tests/fixture $(BIN) analyze | grep -q 'SPD5118 hub'
	! AM5_SPD_DIAG_SHARE=$(CURDIR) AM5_SPD_DIAG_STATE_DIR=tests/fixture $(BIN) status | grep -q 'Reproduction pattern'
	! AM5_SPD_DIAG_SHARE=$(CURDIR) AM5_SPD_DIAG_STATE_DIR=tests/fixture $(BIN) analyze | grep -q 'Monitor'
	$(BIN) --help | grep -q 'am5-spd-diag <command>'
	$(BIN) help status | grep -q 'right now'
	$(BIN) help report | grep -q 'Prints the markdown'
	$(BIN) help report | grep -q -- '--from'
	$(BIN) help analyze | grep -q -- '--from'
	$(BIN) help open | grep -q 'GTK results window'
	$(BIN) help snapshot | grep -q 'not need a password'
	$(BIN) help probe | grep -q 'pkexec-probe'
	$(BIN) help fix | grep -q 'Experimental in-band'
	$(BIN) help recover | grep -q 'am5-spd-diag fix'
	$(BIN) --help | grep -q '  fix '
	$(BIN) help purge | grep -q 'Delete captured evidence'
	$(BIN) help purge | grep -q systemd-tmpfiles
	$(BIN) --help | grep -q 'world-readable'
	! $(BIN) --help | grep -qE '  (install|uninstall|capture) '
	mkdir -p tests/fixture/reports
	AM5_SPD_DIAG_SHARE=$(CURDIR) AM5_SPD_DIAG_STATE_DIR=tests/fixture $(BIN) package --no-snapshot --package-dir tests/fixture/reports > tests/fixture/reports/pkg.path
	AM5_SPD_DIAG_SHARE=$(CURDIR) $(BIN) analyze --from "$$(cat tests/fixture/reports/pkg.path)" | grep -q 'SPD now: corrupted'
	AM5_SPD_DIAG_SHARE=$(CURDIR) $(BIN) analyze --from "$$(cat tests/fixture/reports/pkg.path)" | grep -q 'Reproduction pattern'
	AM5_SPD_DIAG_SHARE=$(CURDIR) $(BIN) analyze --from "$$(cat tests/fixture/reports/pkg.path)" | grep -q 'boot=warm_reboot'
	AM5_SPD_DIAG_SHARE=$(CURDIR) $(BIN) report --from "$$(cat tests/fixture/reports/pkg.path)" | grep -q '2.A52'
	AM5_SPD_DIAG_SHARE=$(CURDIR) $(BIN) report --from "$$(cat tests/fixture/reports/pkg.path)" | grep -q 'Full firmware e820 table'
	AM5_SPD_DIAG_SHARE=$(CURDIR) $(BIN) report --from "$$(cat tests/fixture/reports/pkg.path)" --out tests/fixture/reports/from-out.md | grep -q 'from-out.md'
	grep -q '2.A52' tests/fixture/reports/from-out.md
	! AM5_SPD_DIAG_SHARE=$(CURDIR) AM5_SPD_DIAG_STATE_DIR=tests/fixture $(BIN) status --from "$$(cat tests/fixture/reports/pkg.path)" >/dev/null 2>&1

install:
	@test -x "$(BIN_RELEASE)" && test -x "$(NOTIFY_RELEASE)" || { \
	  echo "am5-spd-diag: missing release binaries at $(BIN_RELEASE)"; \
	  echo "Build as your user (not sudo):  make build"; \
	  echo "Then install:                   sudo make PREFIX=$(PREFIX) install"; \
	  exit 1; \
	}
	$(INSTALL) -d $(DESTDIR)$(BINDIR)
	$(INSTALL) -d $(DESTDIR)$(LIBEXECDIR)
	$(INSTALL) -d $(DESTDIR)$(SHAREDIR)/config
	$(INSTALL) -d $(DESTDIR)$(SHAREDIR)/templates
	$(INSTALL) -d $(DESTDIR)$(DOCDIR)
	$(INSTALL) -d $(DESTDIR)$(UNITDIR)
	$(INSTALL) -d $(DESTDIR)$(PRESETDIR)
	$(INSTALL) -d $(DESTDIR)$(SLEEPDIR)
	$(INSTALL) -d $(DESTDIR)$(TMPFILESDIR)
	$(INSTALL) -d $(DESTDIR)$(POLKITDIR)
	$(INSTALL) -d $(DESTDIR)$(POLKITRULESDIR)
	$(INSTALL) -d $(DESTDIR)$(MANDIR)
	$(INSTALL) -d $(DESTDIR)$(APPDIR)
	$(INSTALL) -d $(DESTDIR)$(ICONDIR)/48x48/apps
	$(INSTALL) -d $(DESTDIR)$(ICONDIR)/128x128/apps
	$(INSTALL) -d $(DESTDIR)$(ICONDIR)/256x256/apps
	$(INSTALL) -d $(DESTDIR)$(DBUSDIR)
	$(INSTALL_PROGRAM) $(BIN_RELEASE) $(DESTDIR)$(LIBEXECDIR)/$(NAME)
	ln -f $(DESTDIR)$(LIBEXECDIR)/$(NAME) $(DESTDIR)$(LIBEXECDIR)/pkexec-snapshot
	ln -f $(DESTDIR)$(LIBEXECDIR)/$(NAME) $(DESTDIR)$(LIBEXECDIR)/pkexec-probe
	ln -f $(DESTDIR)$(LIBEXECDIR)/$(NAME) $(DESTDIR)$(LIBEXECDIR)/pkexec-recover
	ln -sf ../libexec/$(NAME)/$(NAME) $(DESTDIR)$(BINDIR)/$(NAME)
	test -x "$(NOTIFY_RELEASE)"
	$(INSTALL_PROGRAM) $(NOTIFY_RELEASE) $(DESTDIR)$(LIBEXECDIR)/am5-spd-diag-notify
	sed 's|@LIBEXEC@|$(LIBEXECDIR)|g' share/applications/org.opensuse.am5spdDiag.desktop \
	  > $(DESTDIR)$(APPDIR)/org.opensuse.am5spdDiag.desktop
	$(INSTALL_DATA) icons/hicolor/48x48/apps/org.opensuse.am5spdDiag.png \
	  $(DESTDIR)$(ICONDIR)/48x48/apps/org.opensuse.am5spdDiag.png
	$(INSTALL_DATA) icons/hicolor/128x128/apps/org.opensuse.am5spdDiag.png \
	  $(DESTDIR)$(ICONDIR)/128x128/apps/org.opensuse.am5spdDiag.png
	$(INSTALL_DATA) icons/hicolor/256x256/apps/org.opensuse.am5spdDiag.png \
	  $(DESTDIR)$(ICONDIR)/256x256/apps/org.opensuse.am5spdDiag.png
	sed 's|@LIBEXEC@|$(LIBEXECDIR)|g' share/dbus-1/services/org.opensuse.am5spdDiag.service \
	  > $(DESTDIR)$(DBUSDIR)/org.opensuse.am5spdDiag.service
	$(INSTALL_DATA) config/default.conf $(DESTDIR)$(SHAREDIR)/config/default.conf
	$(INSTALL_DATA) templates/ticket.md.tmpl $(DESTDIR)$(SHAREDIR)/templates/ticket.md.tmpl
	$(INSTALL_DATA) README.md $(DESTDIR)$(DOCDIR)/README.md
	$(INSTALL_DATA) LICENSE $(DESTDIR)$(DOCDIR)/LICENSE
	$(INSTALL_DATA) systemd/$(NAME).service $(DESTDIR)$(UNITDIR)/$(NAME).service
	$(INSTALL_DATA) systemd/$(NAME)-pre-sleep.service $(DESTDIR)$(UNITDIR)/$(NAME)-pre-sleep.service
	$(INSTALL_DATA) systemd/$(NAME)-post-sleep.service $(DESTDIR)$(UNITDIR)/$(NAME)-post-sleep.service
	$(INSTALL_DATA) systemd/50-$(NAME).preset $(DESTDIR)$(PRESETDIR)/50-$(NAME).preset
	$(INSTALL_PROGRAM) systemd/system-sleep/$(NAME) $(DESTDIR)$(SLEEPDIR)/$(NAME)
	$(INSTALL_DATA) man/am5-spd-diag.1 $(DESTDIR)$(MANDIR)/am5-spd-diag.1
	$(INSTALL_DATA) systemd/$(NAME).tmpfiles.conf $(DESTDIR)$(TMPFILESDIR)/$(NAME).conf
	$(INSTALL_DATA) systemd/$(NAME)-purge.conf $(DESTDIR)$(SHAREDIR)/tmpfiles-purge.conf
	sed -e 's|@HELPER@|$(LIBEXECDIR)/pkexec-snapshot|g' \
	    -e 's|@LIBEXEC@|$(LIBEXECDIR)|g' \
	  polkit/org.opensuse.am5-spd-diag.snapshot.policy.in \
	  > $(DESTDIR)$(POLKITDIR)/org.opensuse.am5-spd-diag.snapshot.policy
	$(INSTALL_DATA) polkit/org.opensuse.am5-spd-diag.rules \
	  $(DESTDIR)$(POLKITRULESDIR)/org.opensuse.am5-spd-diag.rules
	if [ ! -f $(DESTDIR)$(SYSCONFDIR)/$(NAME).conf ]; then \
	  $(INSTALL) -d $(DESTDIR)$(SYSCONFDIR); \
	  $(INSTALL_DATA) config/default.conf $(DESTDIR)$(SYSCONFDIR)/$(NAME).conf; \
	fi
	if [ -z "$(DESTDIR)" ]; then \
	  systemd-tmpfiles --create $(TMPFILESDIR)/$(NAME).conf; \
	  chcon -t bin_t $(SLEEPDIR)/$(NAME) 2>/dev/null || true; \
	  gtk-update-icon-cache -f -t $(ICONDIR) >/dev/null 2>&1 || true; \
	  update-desktop-database $(APPDIR) >/dev/null 2>&1 || true; \
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
	-rm -f $(DESTDIR)$(PRESETDIR)/50-$(NAME).preset
	-rm -f $(DESTDIR)$(SLEEPDIR)/$(NAME)
	-rm -f $(DESTDIR)$(TMPFILESDIR)/$(NAME).conf
	-rm -f $(DESTDIR)$(POLKITDIR)/org.opensuse.am5-spd-diag.snapshot.policy
	-rm -f $(DESTDIR)$(POLKITRULESDIR)/org.opensuse.am5-spd-diag.rules
	-rm -f $(DESTDIR)$(MANDIR)/am5-spd-diag.1
	-rm -f $(DESTDIR)$(APPDIR)/$(NAME).desktop
	-rm -f $(DESTDIR)$(APPDIR)/org.opensuse.am5spdDiag.desktop
	-rm -f $(DESTDIR)$(ICONDIR)/48x48/apps/org.opensuse.am5spdDiag.png
	-rm -f $(DESTDIR)$(ICONDIR)/128x128/apps/org.opensuse.am5spdDiag.png
	-rm -f $(DESTDIR)$(ICONDIR)/256x256/apps/org.opensuse.am5spdDiag.png
	-rm -f $(DESTDIR)$(DBUSDIR)/org.opensuse.am5spdDiag.service
	if [ -z "$(DESTDIR)" ]; then \
	  gtk-update-icon-cache -f -t $(ICONDIR) >/dev/null 2>&1 || true; \
	  update-desktop-database $(APPDIR) >/dev/null 2>&1 || true; \
	  systemctl daemon-reload; \
	fi
	@echo "Logs kept at /var/log/$(NAME) (am5-spd-diag purge, or make uninstall-purge)"

uninstall-purge: uninstall
	-rm -rf /var/log/$(NAME)
	-rm -f $(DESTDIR)$(SYSCONFDIR)/$(NAME).conf

vendor:
	mkdir -p .cargo
	$(CARGO) vendor --locked vendor > .cargo/config.toml
	@echo "Vendored crates into vendor/ (.cargo/config.toml for OBS --offline)"

# Source0: git snapshot without vendor/ or rustc. Source1: vendor.tar.zst.
# Source2 (official rustc) is fetched by osc-fetch-rust / GHA, not dist.
# Snapshot to /tmp and pack with an absolute -f path so GNU tar does not
# exit 1 ("file changed as we read it") and a relative -f cannot land
# inside the tree. Keep packaging metadata in Source0: OBS %check runs
# `make test`, which reads $(NAME).changes, $(NAME).spec, and debian/.
dist: vendor
	@set -eu; \
	parent="$(DIST_PARENT)"; \
	snap=$$(mktemp -d /tmp/$(NAME)-dist.XXXXXX); \
	src_out="/tmp/$(NAME)-$(VERSION).$$$$.tar.xz"; \
	vend_out="/tmp/$(NAME)-$(VERSION)-vendor.$$$$.tar.zst"; \
	trap 'rm -rf "$$snap"; rm -f "$$src_out" "$$vend_out"' EXIT; \
	cp -a "$(CURDIR)" "$$snap/$(NAME)"; \
	rm -rf "$$snap/$(NAME)/target" "$$snap/$(NAME)/.git" \
	  "$$snap/$(NAME)/.osc" \
	  "$$snap/$(NAME)/.cargo-home"; \
	rm -f "$$snap/$(NAME)/"*.tar.xz "$$snap/$(NAME)/"*.tar.zst; \
	test -d "$$snap/$(NAME)/vendor"; \
	test -f "$$snap/$(NAME)/.cargo/config.toml"; \
	tar -C "$$snap/$(NAME)" -I 'zstd -T0 -19' -cf "$$vend_out" \
	  vendor .cargo/config.toml; \
	test -s "$$vend_out"; \
	rm -rf "$$snap/$(NAME)/vendor"; \
	rm -f "$$snap/$(NAME)/.cargo/config.toml"; \
	tar -C "$$snap" --exclude=__pycache__ --exclude='*.pyc' -cJf "$$src_out" \
	  --transform 's,^$(NAME),$(NAME)-$(VERSION),' \
	  $(NAME); \
	test -s "$$src_out"; \
	tar -tJf "$$src_out" > "$$snap/src.list"; \
	if grep -q '/vendor/' "$$snap/src.list"; then \
	  echo "Source0 must not contain vendor/" >&2; \
	  exit 1; \
	fi; \
	mv -f "$$src_out" "$$parent/$(NAME)-$(VERSION).tar.xz"; \
	mv -f "$$vend_out" "$$parent/$(NAME)-$(VERSION)-vendor.tar.zst"; \
	echo "Wrote $$parent/$(NAME)-$(VERSION).tar.xz"; \
	echo "Wrote $$parent/$(NAME)-$(VERSION)-vendor.tar.zst"

osc-fetch-rust:
	$(MAKEFILE_DIR)scripts/osc_fetch_rust.sh "$(DIST_PARENT)" "$(RUST_DIST_TXT)"

print-osc-repos:
	@echo $(OSC_REPOS)

osc-build: dist osc-fetch-rust
	$(MAKEFILE_DIR)scripts/osc_build.sh $(REPO) $(ARCH)

osc-matrix: dist osc-fetch-rust
	@failed=0; \
	for r in $(OSC_REPOS); do \
	  echo "==== osc build $$r $(ARCH) ===="; \
	  $(MAKEFILE_DIR)scripts/osc_build.sh $$r $(ARCH) || failed=1; \
	done; \
	exit $$failed

osc-meta:
	env -u APPIMAGE -u OWD osc meta prjconf home:fritz-fritz -F $(MAKEFILE_DIR)obs/prjconf
	env -u APPIMAGE -u OWD osc meta pkg home:fritz-fritz $(NAME) -F $(MAKEFILE_DIR)obs/package-meta.xml
