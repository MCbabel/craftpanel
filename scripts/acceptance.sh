#!/usr/bin/env bash
# Installs CraftPanel the way a stranger would, then drives the interface in a real
# browser. This is the test that answers "can someone actually use it".
#
#   scripts/acceptance.sh            build a bundle and install it
#   scripts/acceptance.sh --keep     leave it running afterwards
set -uo pipefail

cd "$(dirname "$0")/.."
ROOT="$PWD"
KEEP="${1:-}"
SHOT=/tmp/craftpanel-acceptance

say()  { printf '\n\033[32m==>\033[0m %s\n' "$*"; }
fail() { printf '\033[31mFAILED:\033[0m %s\n' "$*"; FAILURES=$((FAILURES + 1)); }
ok()   { printf '  \033[32mok\033[0m  %s\n' "$*"; }
FAILURES=0

teardown() {
	[ "$KEEP" = "--keep" ] && { say "left running — http://127.0.0.1:8099"; return; }
	remove_everything
}

remove_everything() {
	say "cleaning up"
	systemctl stop craftpanel.service craftpanel-helper.service 2>/dev/null
	systemctl disable craftpanel.service craftpanel-helper.service 2>/dev/null
	rm -f /etc/systemd/system/craftpanel*.service
	systemctl daemon-reload 2>/dev/null
	for p in $(pgrep -f "java -Xmx|craftpanel supervise|craftpanel-helper|craftpanel serve" 2>/dev/null); do
		kill -9 "$p" 2>/dev/null
	done
	sleep 1
	for u in $(getent passwd | grep '^craft-' | cut -d: -f1); do userdel -r "$u" 2>/dev/null; done
	userdel -r craftpanel 2>/dev/null
	groupdel craftpanel 2>/dev/null
	rm -rf /var/lib/craftpanel /etc/craftpanel /usr/local/bin/craftpanel /usr/local/bin/craftpanel-helper
	printf '  gone: accounts=%s group=%s\n' \
		"$(getent passwd | grep -c '^craft-')" "$(getent group craftpanel >/dev/null && echo yes || echo no)"
}
trap teardown EXIT

rm -rf "$SHOT" && mkdir -p "$SHOT"

# A previous run left with --keep would otherwise send the installer down its
# update path and this would stop being a test of installing.
if [ -x /usr/local/bin/craftpanel ]; then
	say "removing what a previous run left behind"
	remove_everything >/dev/null 2>&1
fi

say "building a release bundle"
rm -f "$ROOT"/dist/craftpanel-*.tar.gz
./scripts/release.sh >"$SHOT/release.log" 2>&1 || { tail -20 "$SHOT/release.log"; exit 1; }
BUNDLE="$(ls -1t "$ROOT"/dist/craftpanel-*.tar.gz 2>/dev/null | head -1)"
[ -n "$BUNDLE" ] && [ -f "$BUNDLE" ] || {
	echo "release.sh produced no bundle:"; tail -20 "$SHOT/release.log"; exit 1
}
ok "$(du -h "$BUNDLE" | cut -f1)  $(basename "$BUNDLE")"

say "installing it"
CRAFTPANEL_BUNDLE="$BUNDLE" CRAFTPANEL_VERSION=test CRAFTPANEL_NONINTERACTIVE=1 \
	CRAFTPANEL_PORT=8099 CRAFTPANEL_POOL_START=25700 CRAFTPANEL_POOL_END=25750 \
	CRAFTPANEL_ADMIN=operator \
	bash install.sh >"$SHOT/install.log" 2>&1
grep -q "CraftPanel is running" "$SHOT/install.log" || { tail -25 "$SHOT/install.log"; exit 1; }
PW=$(grep -oP 'Password\s+\K\S+' "$SHOT/install.log" | tail -1 | sed 's/\x1b\[[0-9;]*m//g')
ok "installed, admin password captured"

sleep 2
systemctl is-active --quiet craftpanel.service && ok "craftpanel.service active" || fail "service not active"
systemctl is-active --quiet craftpanel-helper.service && ok "helper active" || fail "helper not active"

say "checking that the panel runs unprivileged and the helper does not"
ps -o user= -C craftpanel 2>/dev/null | grep -q craftpanel && ok "panel runs as craftpanel" || fail "panel is not running as craftpanel"
ps -eo user,cmd | grep -q "^root.*craftpanel-helper" && ok "helper runs as root" || fail "helper is not root"

# The panel fetches its own Java into here (docs/JAVA.md), so it has to own the
# directory, and a game server under its own account has to be able to walk in.
[ "$(stat -c '%U %a' /var/lib/craftpanel/runtimes 2>/dev/null)" = "craftpanel 755" ] &&
	ok "runtimes/ belongs to the panel, 0755" ||
	fail "runtimes/ is not there as craftpanel 0755: $(stat -c '%U %a' /var/lib/craftpanel/runtimes 2>/dev/null)"

say "driving the interface in a browser"
CHROME=$(ls -d /root/.cache/ms-playwright/chromium-*/chrome-linux*/chrome 2>/dev/null | head -1)
[ -x "$CHROME" ] || { echo "no chromium; skipping the browser part"; exit $FAILURES; }

CRAFTPANEL_PW="$PW" CRAFTPANEL_SHOT="$SHOT" python3 scripts/drive.py
DRIVE=$?
[ $DRIVE -eq 0 ] && ok "browser walkthrough passed" || fail "browser walkthrough failed"

say "result"
if [ "$FAILURES" -eq 0 ]; then
	printf '  \033[32mall checks passed\033[0m — screenshots in %s\n' "$SHOT"
else
	printf '  \033[31m%s check(s) failed\033[0m\n' "$FAILURES"
fi
exit "$FAILURES"
