#!/usr/bin/env bash
# CraftPanel installer. Run it once to install, run it again to update or remove.
#   curl -fsSL https://<host>/install.sh | bash
set -euo pipefail

REPO="${CRAFTPANEL_REPO:-MCbabel/craftpanel}"
PREFIX="${CRAFTPANEL_PREFIX:-/usr/local/bin}"
CONFIG_DIR="${CRAFTPANEL_CONFIG_DIR:-/etc/craftpanel}"
DATA_DIR="${CRAFTPANEL_DATA_DIR:-/var/lib/craftpanel}"
RUN_DIR="/run/craftpanel"
CGROUP_ROOT="/sys/fs/cgroup/system.slice/craftpanel-games"
SERVICE_USER="craftpanel"
SERVICE_GROUP="craftpanel"
FETCH_TMP=""
trap '[ -n "$FETCH_TMP" ] && rm -rf "$FETCH_TMP"' EXIT

BOLD=$'\033[1m'; DIM=$'\033[2m'; GREEN=$'\033[32m'; RED=$'\033[31m'; YELLOW=$'\033[33m'; OFF=$'\033[0m'

say()  { printf '%s\n' "$*"; }
step() { printf '%s==>%s %s\n' "$GREEN" "$OFF" "$*"; }
warn() { printf '%s warning:%s %s\n' "$YELLOW" "$OFF" "$*"; }
die()  { printf '%s error:%s %s\n' "$RED" "$OFF" "$*" >&2; exit 1; }

# Questions are read from the terminal, not stdin: with `curl … | bash` stdin is
# the script itself. Setting CRAFTPANEL_NONINTERACTIVE=1 answers everything from the
# environment instead, which is what the acceptance test and unattended installs need.
ask() {
	local prompt="$1" default="${2:-}" var="${3:-}" answer

	if [ -n "$var" ] && [ -n "${!var:-}" ]; then
		printf '%s' "${!var}"
		return
	fi
	if [ -n "${CRAFTPANEL_NONINTERACTIVE:-}" ] || [ ! -r /dev/tty ]; then
		printf '%s' "$default"
		return
	fi

	if [ -n "$default" ]; then
		read -r -p "$prompt [$default]: " answer </dev/tty || true
		printf '%s' "${answer:-$default}"
	else
		read -r -p "$prompt: " answer </dev/tty || true
		printf '%s' "$answer"
	fi
}

confirm() {
	local answer
	read -r -p "$1 [y/N]: " answer </dev/tty || true
	[[ "${answer,,}" == y* ]]
}

require_root() {
	[ "$(id -u)" -eq 0 ] || die "this installer needs root. Try: curl -fsSL … | sudo bash"
}

detect_arch() {
	case "$(uname -m)" in
		x86_64|amd64)  printf 'x86_64-unknown-linux-gnu' ;;
		aarch64|arm64) printf 'aarch64-unknown-linux-gnu' ;;
		*) die "unsupported architecture: $(uname -m)" ;;
	esac
}

preflight() {
	[ "$(uname -s)" = "Linux" ] || die "CraftPanel runs on Linux only"
	command -v systemctl >/dev/null || die "systemd is required"
	command -v curl >/dev/null || die "curl is required"
	command -v java >/dev/null || warn "java was not found — install a JRE 21 before creating a server"

	if [ ! -e /sys/fs/cgroup/cgroup.controllers ]; then
		warn "cgroup v2 is not mounted — per-user CPU and memory limits will not work"
	fi
}

installed_version() {
	[ -x "$PREFIX/craftpanel" ] && "$PREFIX/craftpanel" --version 2>/dev/null | awk '{print $2}' || true
}

latest_version() {
	curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" 2>/dev/null |
		grep -m1 '"tag_name"' | sed -E 's/.*"v?([^"]+)".*/\1/' || true
}

fetch_binaries() {
	local version="$1" arch tmp
	arch="$(detect_arch)"
	tmp="$(mktemp -d)"
	# Not a RETURN trap: bash does not scope those to the function, so it would
	# fire again later with $tmp long gone and take `set -u` down with it.
	FETCH_TMP="$tmp"

	# A local bundle covers two cases that both matter: trying a build before it
	# is published, and installing on a machine that cannot reach GitHub.
	if [ -n "${CRAFTPANEL_BUNDLE:-}" ]; then
		[ -f "$CRAFTPANEL_BUNDLE" ] || die "CRAFTPANEL_BUNDLE is set but $CRAFTPANEL_BUNDLE is not there"
		step "installing from $CRAFTPANEL_BUNDLE"
		cp "$CRAFTPANEL_BUNDLE" "$tmp/bundle.tar.gz"
		[ -f "$CRAFTPANEL_BUNDLE.sha256" ] && cp "$CRAFTPANEL_BUNDLE.sha256" "$tmp/sum"
	else
		local base="https://github.com/$REPO/releases/download/v$version"
		step "downloading CraftPanel $version for $arch"

		curl -fsSL "$base/craftpanel-$arch.tar.gz" -o "$tmp/bundle.tar.gz" ||
			die "download failed — is v$version published for $arch?"

		curl -fsSL "$base/craftpanel-$arch.tar.gz.sha256" -o "$tmp/sum" 2>/dev/null || true
	fi

	if [ -f "$tmp/sum" ]; then
		(cd "$tmp" && awk '{print $1"  bundle.tar.gz"}' sum | sha256sum -c - >/dev/null) ||
			die "checksum mismatch — refusing to install"
		say "  checksum verified"
	else
		warn "no checksum published for this release"
	fi

	tar -xzf "$tmp/bundle.tar.gz" -C "$tmp"
	install -m 0755 "$tmp/craftpanel" "$PREFIX/craftpanel"
	install -m 0755 "$tmp/craftpanel-helper" "$PREFIX/craftpanel-helper"
	rm -rf "$tmp"
	FETCH_TMP=""
}

ensure_accounts() {
	getent group "$SERVICE_GROUP" >/dev/null || groupadd --system "$SERVICE_GROUP"
	getent passwd "$SERVICE_USER" >/dev/null || useradd --system \
		--gid "$SERVICE_GROUP" --home-dir "$DATA_DIR" --no-create-home \
		--shell /usr/sbin/nologin --comment "CraftPanel service account" "$SERVICE_USER"

	# The two hops above the accounts belong to root, and this is the whole reason
	# the helper can be sure what it is walking. Everything below users/<id> is the
	# managed account's own (2770), so it may rename any of it at any time; if the
	# panel owned users/ it could do the same one level higher — move an account
	# aside, put a link in its place, and have root's chown-tree land wherever it
	# chose. Root owning users/ is what nails those names down.
	#
	# $DATA_DIR itself cannot be closed the same way: panel.db lives in it, so the
	# panel has to be able to write there. The sticky bit is the answer — whoever
	# may write in the directory may still only rename or delete what is his own,
	# and users/ is root's. The panel keeps its database, its cache and its
	# backups; it does not keep the ability to move the accounts aside.
	#
	# 0751/1771 rather than anything tighter: a managed account shares no group
	# with the panel and has to walk through both hops to reach its own directory.
	# Traversable is enough — the listing stays closed and each account's own
	# directory is 2770, so knowing a path leads nowhere without owning it.
	#
	# `install -d` puts an existing directory right as well as making a new one,
	# which is how an installation from before this comes along on the next run.
	install -d -o root -g "$SERVICE_GROUP" -m 1771 "$DATA_DIR"
	install -d -o root -g "$SERVICE_GROUP" -m 0751 "$DATA_DIR/users"
	install -d -o "$SERVICE_USER" -g "$SERVICE_GROUP" -m 0750 "$DATA_DIR/cache"
	install -d -m 0755 "$CONFIG_DIR"
}

write_config() {
	local port="$1" start="$2" end="$3"
	[ -f "$CONFIG_DIR/config.toml" ] && return 0

	cat >"$CONFIG_DIR/config.toml" <<EOF
bind = "0.0.0.0:$port"
data_dir = "$DATA_DIR"
helper_socket = "$RUN_DIR/helper.sock"

[ports]
start = $start
end = $end
EOF
	chown root:"$SERVICE_GROUP" "$CONFIG_DIR/config.toml"
	chmod 0640 "$CONFIG_DIR/config.toml"
}

write_units() {
	cat >/etc/systemd/system/craftpanel-helper.service <<EOF
[Unit]
Description=CraftPanel privileged helper
Before=craftpanel.service

[Service]
Type=simple
ExecStart=$PREFIX/craftpanel-helper
Environment=CRAFTPANEL_HELPER_SOCKET=$RUN_DIR/helper.sock
Environment=CRAFTPANEL_USERS_ROOT=$DATA_DIR/users
Environment=CRAFTPANEL_SUPERVISOR=$PREFIX/craftpanel
Environment=CRAFTPANEL_GROUP=$SERVICE_GROUP
# Games live in a tree of their own, beside the two services rather than
# inside either: systemd kills a service's whole control group when it stops,
# so a tree under craftpanel.service would take every running server down with a
# panel update. It sits under system.slice because the kernel weighs a move
# against the common ancestor of the two groups, and only there is that
# ancestor one this host lets root write. The panel reads the same tree for
# what an account is using, which is why both units name it.
Environment=CRAFTPANEL_CGROUP_ROOT=$CGROUP_ROOT
# The panel runs unprivileged but has to create its own supervisor socket in
# here, so the directory carries the shared group and is group-writable.
Group=$SERVICE_GROUP
RuntimeDirectory=craftpanel
RuntimeDirectoryMode=0771
RuntimeDirectoryPreserve=yes
Restart=on-failure
RestartSec=2

[Install]
WantedBy=multi-user.target
EOF

	cat >/etc/systemd/system/craftpanel.service <<EOF
[Unit]
Description=CraftPanel
After=network.target craftpanel-helper.service
Requires=craftpanel-helper.service

[Service]
Type=simple
User=$SERVICE_USER
Group=$SERVICE_GROUP
ExecStart=$PREFIX/craftpanel serve
Environment=CRAFTPANEL_CONFIG=$CONFIG_DIR/config.toml
Environment=CRAFTPANEL_CGROUP_ROOT=$CGROUP_ROOT
Delegate=yes
NoNewPrivileges=yes
ProtectSystem=strict
ProtectHome=yes
PrivateTmp=yes
# The panel opens its own socket for supervisors to report back on, and
# ProtectSystem=strict would otherwise hand it a read-only /run.
ReadWritePaths=$DATA_DIR $RUN_DIR
AmbientCapabilities=
Restart=on-failure
RestartSec=2

[Install]
WantedBy=multi-user.target
EOF

	systemctl daemon-reload
}

# ------------------------------------------------------------- the old name
#
# A machine set up before the rename carries mcpanel in eight places: the data
# directory, the config, the two units with their drop-ins, the service account,
# the group, one system account per panel user, and the control group the games
# run in. Everything from here to do_upgrade moves such an installation across.
#
# Every step asks first whether it still has anything to do, so a second run
# finds nothing and changes nothing, and a run that broke off halfway carries on
# where it stopped.

OLD_NAME="mcpanel"
OLD_DISPLAY="MCPanel"
OLD_ENV_PREFIX="MCPANEL_"
OLD_ACCOUNT_PREFIX="mcp-"
OLD_CONFIG_DIR="/etc/$OLD_NAME"
OLD_DATA_DIR="/var/lib/$OLD_NAME"
OLD_RUN_DIR="/run/$OLD_NAME"
OLD_CGROUP_ROOT="/sys/fs/cgroup/system.slice/$OLD_NAME-games"
OLD_KEEP="upgrade-from-$OLD_NAME"
STOP_GRACE="${CRAFTPANEL_STOP_GRACE:-300}"
SQL_RUNNER=""

old_installation() {
	local marker
	for marker in \
		"/etc/systemd/system/$OLD_NAME.service" \
		"/etc/systemd/system/$OLD_NAME-helper.service" \
		"$OLD_CONFIG_DIR/config.toml" \
		"$OLD_DATA_DIR" \
		"$OLD_CGROUP_ROOT" \
		"$PREFIX/$OLD_NAME" \
		"$PREFIX/$OLD_NAME-helper"
	do
		if [ -e "$marker" ]; then
			return 0
		fi
	done

	getent passwd "$OLD_NAME" >/dev/null && return 0
	getent group "$OLD_NAME" >/dev/null && return 0
	[ -n "$(old_accounts)" ] && return 0
	[ -n "$(old_groups)" ] && return 0
	return 1
}

# name:home per line. The id in the account name is the panel id in lower case
# while the directory keeps its own spelling, so the new home is the old one with
# the data directory swapped — never one built from the name.
old_accounts() {
	getent passwd | awk -F: -v prefix="$OLD_ACCOUNT_PREFIX" \
		'index($1, prefix) == 1 { print $1 ":" $6 }'
}

old_groups() {
	getent group | awk -F: -v prefix="$OLD_ACCOUNT_PREFIX" 'index($1, prefix) == 1 { print $1 }'
}

renamed() {
	printf 'craft-%s' "${1#"$OLD_ACCOUNT_PREFIX"}"
}

# Everything that could make the move stop in the middle is asked here, before a
# single service is stopped: a missing tool, a name that is already taken, a data
# directory on a mount of its own. Failing in this function costs nothing.
upgrade_preflight() {
	local tool entry name

	for tool in usermod groupmod; do
		command -v "$tool" >/dev/null || die "the move needs $tool (shadow-utils). Nothing has been touched."
	done

	if [ -f "$OLD_DATA_DIR/panel.db" ] || [ -f "$DATA_DIR/panel.db" ]; then
		pick_sql_runner
		[ -n "$SQL_RUNNER" ] ||
			die "the move has to write in panel.db and found neither sqlite3 nor python3. Install one of them (apt install sqlite3) and run this again — nothing has been touched."
	fi

	# An empty $DATA_DIR is what ensure_accounts leaves behind when an earlier run
	# of this installer got that far and no further; it may go. One with anything
	# else in it is a second installation, and picking between two is not a
	# decision an installer should make on its own.
	if [ -d "$OLD_DATA_DIR" ] && [ -d "$DATA_DIR" ]; then
		rmdir "$DATA_DIR/cache" "$DATA_DIR/users" "$DATA_DIR" 2>/dev/null || true
		if [ -e "$DATA_DIR" ]; then
			die "$OLD_DATA_DIR and $DATA_DIR both hold data. Move or remove one of them by hand — see docs/UPGRADE.md."
		fi
	fi

	# A rename inside one directory is what the move of the data is, and a mount of
	# its own turns that into a copy of every world on the machine.
	if [ -d "$OLD_DATA_DIR" ] &&
		[ "$(stat -c %d "$OLD_DATA_DIR")" != "$(stat -c %d "$(dirname "$OLD_DATA_DIR")")" ]; then
		die "$OLD_DATA_DIR is a filesystem of its own, so it cannot be renamed. Mount it at $DATA_DIR instead and run this again."
	fi

	if getent passwd "$SERVICE_USER" >/dev/null && getent passwd "$OLD_NAME" >/dev/null; then
		die "both $OLD_NAME and $SERVICE_USER exist as system accounts. Decide which one owns the files and remove the other — see docs/UPGRADE.md."
	fi

	if getent group "$SERVICE_GROUP" >/dev/null && getent group "$OLD_NAME" >/dev/null; then
		die "both $OLD_NAME and $SERVICE_GROUP exist as groups. Decide which one owns the files and remove the other — see docs/UPGRADE.md."
	fi

	while IFS= read -r entry; do
		[ -n "$entry" ] || continue
		name="$(renamed "${entry%%:*}")"
		if getent passwd "$name" >/dev/null; then
			die "${entry%%:*} should become $name, and $name already exists. Nothing has been touched."
		fi
	done < <(old_accounts)

	while IFS= read -r entry; do
		[ -n "$entry" ] || continue
		name="$(renamed "$entry")"
		if getent group "$name" >/dev/null; then
			die "the group $entry should become $name, and $name already exists. Nothing has been touched."
		fi
	done < <(old_groups)
}

# Every process in the games' tree, one pid per line. A cgroup file always
# measures zero bytes, so what is in it can only be found by reading it.
cgroup_pids() {
	local roll
	[ -d "$OLD_CGROUP_ROOT" ] || return 0

	while IFS= read -r roll; do
		cat "$roll" 2>/dev/null || true
	done < <(find "$OLD_CGROUP_ROOT" -name cgroup.procs 2>/dev/null || true)
}

# Everything in that tree that is not a supervisor is a game. The supervisor is in
# there with its child (it joins the group before it execs), and telling the two
# apart is what keeps the SIGTERM below off the wrong process.
game_processes() {
	local pid exe

	while read -r pid; do
		[ -d "/proc/$pid" ] || continue
		exe="$(readlink "/proc/$pid/exe" 2>/dev/null || true)"
		exe="${exe% (deleted)}"
		case "$exe" in
			"$PREFIX/$OLD_NAME"|"$PREFIX/$OLD_NAME-helper") continue ;;
			"$PREFIX/craftpanel"|"$PREFIX/craftpanel-helper") continue ;;
		esac
		printf '%s\n' "$pid"
	done < <(cgroup_pids)
}

# SIGTERM to the game's process group, and never anything harder: what saves the
# world is the shutdown hook the game runs on that signal, and a SIGKILL in the
# middle of it is the one thing this whole path exists to avoid. The panel is
# still up while this runs, so every supervisor can hand its last console lines
# over and end by itself instead of waiting for a panel that is already gone.
stop_games() {
	local pids=() pid waited=0

	mapfile -t pids < <(game_processes)
	if [ "${#pids[@]}" -eq 0 ]; then
		say "  no Minecraft server is running"
		return 0
	fi

	step "asking ${#pids[@]} running game process(es) to save and stop"
	for pid in "${pids[@]}"; do
		kill -TERM -- "-$pid" 2>/dev/null || kill -TERM "$pid" 2>/dev/null || true
	done

	while [ "$waited" -lt "$STOP_GRACE" ]; do
		mapfile -t pids < <(game_processes)
		if [ "${#pids[@]}" -eq 0 ]; then
			say "  every server has saved and stopped"
			return 0
		fi
		sleep 2
		waited=$((waited + 2))
	done

	die "still running after ${STOP_GRACE}s: ${pids[*]}. Nothing has been moved. Stop those servers from the panel and run this again, or give them longer with CRAFTPANEL_STOP_GRACE=<seconds>."
}

stop_old_services() {
	local unit
	for unit in "$OLD_NAME.service" "$OLD_NAME-helper.service"; do
		if [ ! -f "/etc/systemd/system/$unit" ] && ! systemctl is-active --quiet "$unit"; then
			continue
		fi
		step "stopping $unit"
		systemctl disable --now "$unit" >/dev/null 2>&1 || true
		if systemctl is-active --quiet "$unit"; then
			die "$unit would not stop. Nothing has been moved. See: systemctl status $unit"
		fi
	done
}

# A cgroup directory cannot be renamed — the kernel answers EPERM to rename(2) on
# cgroup2 — so the old tree is taken down instead. Nothing is lost with it: the
# panel writes every ready account's ceilings back at start (auth/users.rs,
# rewrite_limits), and the helper makes the directory it needs on the way to a
# spawn. Both use the new root because both units name it.
move_cgroup() {
	local pid waited=0
	[ -d "$OLD_CGROUP_ROOT" ] || return 0
	step "taking down $OLD_CGROUP_ROOT"

	# The games are gone by now; whatever is left is a supervisor that could not
	# reach the panel, and it holds no world open.
	while read -r pid; do
		kill -TERM "$pid" 2>/dev/null || true
	done < <(cgroup_pids)

	while [ "$waited" -lt 30 ] && [ -n "$(cgroup_pids)" ]; do
		sleep 1
		waited=$((waited + 1))
	done

	find "$OLD_CGROUP_ROOT" -mindepth 1 -depth -type d -exec rmdir {} + 2>/dev/null || true
	rmdir "$OLD_CGROUP_ROOT" ||
		die "$OLD_CGROUP_ROOT still holds processes. See who: cat $OLD_CGROUP_ROOT/*/cgroup.procs"
}

move_data() {
	[ -d "$OLD_DATA_DIR" ] || return 0
	step "moving $OLD_DATA_DIR → $DATA_DIR"
	mv -T -- "$OLD_DATA_DIR" "$DATA_DIR" || die "moving the data directory failed. Nothing else has been changed."
}

# usermod -l and groupmod -n and nothing else: uid and gid stay as they are, and
# so every file in the tree that just moved keeps its owner without a single
# chown. -d without -m for the same reason — the directory is already where the
# new entry says it is.
rename_service_account() {
	local args

	if getent group "$OLD_NAME" >/dev/null; then
		step "renaming the group $OLD_NAME → $SERVICE_GROUP (gid unchanged)"
		groupmod -n "$SERVICE_GROUP" "$OLD_NAME" || die "renaming the group $OLD_NAME failed."
	fi

	if getent passwd "$OLD_NAME" >/dev/null; then
		step "renaming the account $OLD_NAME → $SERVICE_USER (uid unchanged)"
		args=(-l "$SERVICE_USER" -d "$DATA_DIR")
		if [ "$(getent passwd "$OLD_NAME" | cut -d: -f5)" = "$OLD_NAME service account" ]; then
			args+=(-c "CraftPanel service account")
		fi
		usermod "${args[@]}" "$OLD_NAME" ||
			die "renaming the account $OLD_NAME failed — is something still running as $OLD_NAME? Check with: ps -u $OLD_NAME"
	fi
}

rename_managed_accounts() {
	local entries=() groups=() entry name home new comment args
	mapfile -t entries < <(old_accounts)
	mapfile -t groups < <(old_groups)

	if [ "${#entries[@]}" -gt 0 ]; then
		step "renaming ${#entries[@]} managed account(s) $OLD_ACCOUNT_PREFIX… → craft-…"
	fi

	for entry in "${entries[@]}"; do
		name="${entry%%:*}"
		home="${entry#*:}"
		new="$(renamed "$name")"
		case "$home" in
			"$OLD_DATA_DIR"/*) home="$DATA_DIR${home#"$OLD_DATA_DIR"}" ;;
		esac

		args=(-l "$new" -d "$home")
		comment="$(getent passwd "$name" | cut -d: -f5)"
		if [ "$comment" = "$OLD_NAME managed account" ]; then
			args+=(-c "craftpanel managed account")
		fi

		usermod "${args[@]}" "$name" ||
			die "renaming $name to $new failed. Check with: ps -u $name"
	done

	# useradd made a group of the same name beside each account, and that group is
	# what a game process runs with.
	for entry in "${groups[@]}"; do
		getent group "$entry" >/dev/null || continue
		new="$(renamed "$entry")"
		groupmod -n "$new" "$entry" || die "renaming the group $entry to $new failed."
	done
}

# One rule for every file that came from the old installation: the token changes,
# the setting does not. It catches the paths as well, because the only thing that
# differs between /var/lib/mcpanel and /var/lib/craftpanel is the name.
retune() {
	local file="$1"
	sed -i \
		-e "s/$OLD_ENV_PREFIX/CRAFTPANEL_/g" \
		-e "s/$OLD_DISPLAY/CraftPanel/g" \
		-e "s/$OLD_NAME/craftpanel/g" \
		-e "s/$OLD_ACCOUNT_PREFIX/craft-/g" \
		"$file" || die "rewriting $file failed."
}

move_config() {
	install -d -m 0755 "$CONFIG_DIR"

	if [ -f "$OLD_CONFIG_DIR/config.toml" ]; then
		if [ -f "$CONFIG_DIR/config.toml" ]; then
			warn "$CONFIG_DIR/config.toml already exists — $OLD_CONFIG_DIR/config.toml is left for you to compare"
		else
			step "moving the configuration"
			mv -- "$OLD_CONFIG_DIR/config.toml" "$CONFIG_DIR/config.toml" ||
				die "moving $OLD_CONFIG_DIR/config.toml failed."
			retune "$CONFIG_DIR/config.toml"
		fi
	fi

	if [ -f "$CONFIG_DIR/config.toml" ]; then
		chown root:"$SERVICE_GROUP" "$CONFIG_DIR/config.toml"
		chmod 0640 "$CONFIG_DIR/config.toml"
	fi
	rmdir "$OLD_CONFIG_DIR" 2>/dev/null || true
}

# The units themselves are written fresh by write_units — they are the
# installer's own and nothing is lost by making them again. The drop-ins are not:
# those are the operator's, so they move over one by one, keeping their order and
# their content bar the name.
move_units() {
	local old new
	for old in "$OLD_NAME" "$OLD_NAME-helper"; do
		new="craftpanel${old#"$OLD_NAME"}"
		move_dropins "$old.service.d" "$new.service.d"

		if [ -f "/etc/systemd/system/$old.service" ]; then
			install -d -o root -g root -m 0700 "$DATA_DIR/$OLD_KEEP"
			mv -- "/etc/systemd/system/$old.service" "$DATA_DIR/$OLD_KEEP/$old.service" ||
				die "moving /etc/systemd/system/$old.service aside failed."
		fi
	done

	rm -f "/etc/systemd/system/multi-user.target.wants/$OLD_NAME.service" \
		"/etc/systemd/system/multi-user.target.wants/$OLD_NAME-helper.service"
	write_units
}

move_dropins() {
	local from="/etc/systemd/system/$1" to="/etc/systemd/system/$2" conf name
	[ -d "$from" ] || return 0
	install -d -m 0755 "$to"

	for conf in "$from"/*; do
		[ -e "$conf" ] || continue
		name="$(basename "$conf")"
		name="${name//"$OLD_NAME"/craftpanel}"
		if [ -e "$to/$name" ]; then
			warn "$to/$name already exists — $conf stays where it is"
			continue
		fi
		step "moving the drop-in $conf → $to/$name"
		mv -- "$conf" "$to/$name" || die "moving $conf failed."
		case "$name" in
			*.conf) retune "$to/$name" ;;
		esac
	done

	rmdir "$from" 2>/dev/null || warn "$from is not empty and stays behind"
}

# No column holds a system account name: the panel works it out from the panel id
# every time it needs it (craftpanel-proto, system_username), so the accounts
# renamed above are already the ones the panel will look for. What the database
# does keep in plain text is the helper's last complaint about an account, and
# that sentence carries the old name and the old path around with it — along with
# two settings whose default was written before the rename.
patch_database() {
	local db="$DATA_DIR/panel.db"
	[ -f "$db" ] || return 0
	[ -n "$SQL_RUNNER" ] || pick_sql_runner
	[ -n "$SQL_RUNNER" ] || die "panel.db needs sqlite3 or python3, and neither is here. Everything else has moved, see docs/UPGRADE.md."
	step "putting the old name right in panel.db"

	run_sql "$db" "UPDATE users \
		SET system_error_message = replace(replace(system_error_message, '$OLD_NAME', 'craftpanel'), '$OLD_ACCOUNT_PREFIX', 'craft-') \
		WHERE system_error_message LIKE '%$OLD_NAME%' OR system_error_message LIKE '%$OLD_ACCOUNT_PREFIX%'"

	if has_table "$db" mail_settings; then
		run_sql "$db" "UPDATE mail_settings SET from_name = 'CraftPanel' WHERE from_name = '$OLD_NAME'"
	fi

	# Only the untouched default. A folder that already exists in somebody's Drive
	# is found by the id stored beside the account, so it keeps its name and its
	# backups either way (drive/mod.rs, folder).
	if has_table "$db" drive_settings; then
		run_sql "$db" "UPDATE drive_settings SET folder_name = 'craftpanel-backups' WHERE folder_name = '$OLD_NAME-backups'"
	fi

	# Writing to it as root can leave the journal beside it owned by root, and the
	# panel does not run as root. SQLite takes the two files away again when the
	# last connection closes, so this is for the run that does not get that far.
	chown "$SERVICE_USER:$SERVICE_GROUP" "$db" "$db-wal" "$db-shm" 2>/dev/null || true
}

# sqlite3 is not on every machine, and python3 carries the same library on nearly
# all of them. One of the two is asked for in the preflight, long before anything
# moves.
pick_sql_runner() {
	if command -v sqlite3 >/dev/null; then
		SQL_RUNNER="sqlite3"
	elif command -v python3 >/dev/null && python3 -c 'import sqlite3' 2>/dev/null; then
		SQL_RUNNER="python3"
	fi
}

has_table() {
	local db="$1" name="$2" found
	found="$(sql_value "$db" "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = '$name'")" ||
		die "reading $db failed."
	[ "$found" = "1" ]
}

sql_value() {
	case "$SQL_RUNNER" in
		sqlite3) sqlite3 "$1" "$2" ;;
		python3) python3 -c 'import sqlite3, sys; print(sqlite3.connect(sys.argv[1]).execute(sys.argv[2]).fetchone()[0])' "$1" "$2" ;;
		*) return 1 ;;
	esac
}

run_sql() {
	local db="$1" statement="$2" worked=1
	case "$SQL_RUNNER" in
		sqlite3) sqlite3 "$db" "$statement" >/dev/null && worked=0 ;;
		python3) python3 -c 'import sqlite3, sys; c = sqlite3.connect(sys.argv[1]); c.execute(sys.argv[2]); c.commit()' "$db" "$statement" && worked=0 ;;
	esac
	[ "$worked" -eq 0 ] ||
		die "this statement did not go through: $statement — everything else has moved, see docs/UPGRADE.md."
}

# One machine can carry both: the installation people use and a second one put
# there to try something. The move therefore happens when it is asked for — by
# hand at the terminal, or with CRAFTPANEL_UPGRADE=yes without one — and never as
# the silent default of an unattended run, which would stop somebody's servers
# without anybody having said so.
wants_upgrade() {
	local answer
	say
	say "${BOLD}An installation under the old name $OLD_NAME is on this machine.${OFF}"
	say "${DIM}Moving it keeps the worlds, the database, the accounts and their uids."
	say "Running Minecraft servers are told to save and stop first, and stay stopped.${OFF}"
	say

	if [ -n "${CRAFTPANEL_UPGRADE:-}" ]; then
		answer="${CRAFTPANEL_UPGRADE}"
	elif [ -n "${CRAFTPANEL_NONINTERACTIVE:-}" ] || [ ! -r /dev/tty ]; then
		say "This run answers no questions, so $OLD_NAME is left alone."
		say "${DIM}Run it again with CRAFTPANEL_UPGRADE=yes to move it.${OFF}"
		return 1
	else
		read -r -p "Move it to CraftPanel now? [Y/n]: " answer </dev/tty || true
		answer="${answer:-yes}"
	fi

	case "${answer,,}" in
		y*|j*) return 0 ;;
	esac
	say "Leaving $OLD_NAME where it is."
	return 1
}

do_upgrade() {
	local version="$1"
	say
	upgrade_preflight
	# Before a single service goes down: a download that fails here leaves an
	# installation that is still running.
	fetch_binaries "$version"

	stop_games
	stop_old_services
	move_cgroup
	move_data
	rename_service_account
	rename_managed_accounts
	move_config
	move_units
	patch_database
	ensure_accounts

	rm -rf "$OLD_RUN_DIR"
	rm -f "$PREFIX/$OLD_NAME" "$PREFIX/$OLD_NAME-helper"

	step "starting CraftPanel"
	systemctl enable --now craftpanel-helper.service >/dev/null 2>&1 || true
	systemctl enable --now craftpanel.service >/dev/null 2>&1 || true

	sleep 2
	if ! systemctl is-active --quiet craftpanel.service; then
		die "everything was moved, but CraftPanel did not start. See: journalctl -u craftpanel -n 50"
	fi

	say
	say "${GREEN}${BOLD}CraftPanel is running under its new name.${OFF}"
	say
	say "  Data      $DATA_DIR"
	say "  Config    $CONFIG_DIR/config.toml"
	say "  Services  craftpanel.service, craftpanel-helper.service"
	say "  Accounts  $SERVICE_USER, and craft-… for every panel user"
	say
	say "${DIM}Your usernames and passwords are unchanged."
	say "The Minecraft servers are stopped — start them again from the panel."
	say "The old unit files are kept in $DATA_DIR/$OLD_KEEP; docs/UPGRADE.md says what happened.${OFF}"
	say
}

do_install() {
	local version="$1"
	say
	say "${BOLD}Installing CraftPanel${OFF}"
	say "${DIM}A panel for running Minecraft servers on this machine.${OFF}"
	say

	local port pool_start pool_end admin
	port="$(ask 'Web interface port' '8080' CRAFTPANEL_PORT)"
	pool_start="$(ask 'First port for Minecraft servers' '25565' CRAFTPANEL_POOL_START)"
	pool_end="$(ask 'Last port for Minecraft servers' '25700' CRAFTPANEL_POOL_END)"
	admin="$(ask 'Administrator username' 'admin' CRAFTPANEL_ADMIN)"
	say

	ensure_accounts
	fetch_binaries "$version"
	write_config "$port" "$pool_start" "$pool_end"
	write_units

	step "creating the administrator account"
	local password
	password="$(CRAFTPANEL_CONFIG=$CONFIG_DIR/config.toml \
		runuser -u "$SERVICE_USER" -- "$PREFIX/craftpanel" admin create --username "$admin" --print-password)" ||
		die "could not create the administrator account"

	systemctl enable --now craftpanel-helper.service >/dev/null 2>&1
	systemctl enable --now craftpanel.service >/dev/null 2>&1

	sleep 1
	if ! systemctl is-active --quiet craftpanel.service; then
		die "CraftPanel did not start. See: journalctl -u craftpanel -n 50"
	fi

	say
	say "${GREEN}${BOLD}CraftPanel is running.${OFF}"
	say
	say "  Address   http://$(hostname -I 2>/dev/null | awk '{print $1}'):$port"
	say "  Username  $admin"
	say "  Password  ${BOLD}$password${OFF}"
	say
	say "${DIM}Write the password down; it is not shown again."
	say "Run this installer again to update or remove CraftPanel.${OFF}"
	say
}

do_update() {
	local current="$1" latest="$2"

	if [ "$current" = "$latest" ]; then
		say "CraftPanel $current is already the newest version."
		confirm "Reinstall it anyway?" || return 0
	fi

	step "updating $current → $latest"
	# Not only the binaries: an update is also where an installation from an
	# older layout is put right. `ensure_accounts` makes nothing that is already
	# there — it sets the owner and the mode, which is what moved.
	ensure_accounts
	fetch_binaries "$latest"
	write_units
	systemctl restart craftpanel-helper.service
	systemctl restart craftpanel.service

	sleep 1
	if systemctl is-active --quiet craftpanel.service; then
		say "${GREEN}Updated to $latest.${OFF} Running Minecraft servers were not interrupted."
	else
		die "CraftPanel did not come back up. See: journalctl -u craftpanel -n 50"
	fi
}

do_uninstall() {
	say
	warn "This removes CraftPanel from this machine."
	confirm "Continue?" || return 0

	step "stopping services"
	systemctl disable --now craftpanel.service >/dev/null 2>&1 || true
	systemctl disable --now craftpanel-helper.service >/dev/null 2>&1 || true
	rm -f /etc/systemd/system/craftpanel.service /etc/systemd/system/craftpanel-helper.service
	systemctl daemon-reload

	step "removing programs"
	rm -f "$PREFIX/craftpanel" "$PREFIX/craftpanel-helper"

	say
	say "Your Minecraft servers and the database are still in $DATA_DIR."
	if confirm "Delete them too? This cannot be undone"; then
		rm -rf "$DATA_DIR" "$CONFIG_DIR"
		say "Everything removed."
	else
		say "Kept $DATA_DIR. Reinstalling CraftPanel will pick it up again."
	fi
	say
}

main() {
	require_root
	preflight

	local current latest
	current="$(installed_version)"
	if [ -n "${CRAFTPANEL_BUNDLE:-}" ]; then
		latest="${CRAFTPANEL_VERSION:-local}"
	else
		latest="${CRAFTPANEL_VERSION:-$(latest_version)}"
		[ -n "$latest" ] || die "could not reach GitHub to find the newest version"
	fi

	# Before install and before update, because an installation under the old name
	# is neither. Saying no to the move leads on to the ordinary paths below, which
	# is how a second, fresh installation beside the old one stays possible.
	if old_installation && wants_upgrade; then
		do_upgrade "$latest"
		return
	fi

	if [ -z "$current" ]; then
		do_install "$latest"
		return
	fi

	say
	say "${BOLD}CraftPanel $current is installed.${OFF}  Newest available: $latest"
	say
	say "  1) Update"
	say "  2) Uninstall"
	say "  3) Nothing, quit"
	say
	case "$(ask 'What would you like to do?' '1' CRAFTPANEL_ACTION)" in
		1) do_update "$current" "$latest" ;;
		2) do_uninstall ;;
		*) say "Nothing changed." ;;
	esac
}

main "$@"
