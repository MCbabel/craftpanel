#!/usr/bin/env bash
# Keeps a test machine from drowning in Minecraft servers. Agents start them and
# do not always get to stop them; each one is a JVM with a gigabyte reserved.
#
# It only steps in at real danger, so a normal run of two or three servers is
# left alone. Everything it does is written down, because a server that vanished
# without a line in a log looks like a bug in the panel.
set -uo pipefail

LOG=/tmp/craftpanel-watchdog.log
MAX_SERVERS="${MAX_SERVERS:-4}"
MIN_FREE_GB="${MIN_FREE_GB:-4}"
MIN_DISK_GB="${MIN_DISK_GB:-3}"
EVERY="${EVERY:-30}"

note() { printf '%s  %s\n' "$(date +%H:%M:%S)" "$*" >>"$LOG"; }

# Newest first: a server that just started is the one an agent can most cheaply
# try again, and the long-running one is more likely to be the user's.
newest_servers() {
	# shellcheck disable=SC2009  # pgrep prints neither the start time nor the command
	ps -eo lstart=,pid=,cmd= | grep '[j]ava -Xmx' |
		while read -r _ mon day time year pid _; do
			printf '%s %s\n' "$(date -d "$mon $day $time $year" +%s 2>/dev/null || echo 0)" "$pid"
		done | sort -rn | awk '{print $2}'
}

stop_one() {
	local pid="$1" why="$2"
	local cmd
	cmd=$(ps -o cmd= -p "$pid" 2>/dev/null | cut -c1-80)
	note "stopping pid $pid ($why): $cmd"
	kill "$pid" 2>/dev/null
	for _ in $(seq 1 15); do
		kill -0 "$pid" 2>/dev/null || return 0
		sleep 1
	done
	note "pid $pid ignored the stop; killing"
	kill -9 "$pid" 2>/dev/null
}

note "watchdog up: at most $MAX_SERVERS servers, keep ${MIN_FREE_GB}G memory and ${MIN_DISK_GB}G disk"

while true; do
	sleep "$EVERY"

	mapfile -t servers < <(newest_servers)
	count=${#servers[@]}
	free_gb=$(free -g | awk '/^Mem:/ {print $7}')
	disk_gb=$(df -BG --output=avail / | tail -1 | tr -dc '0-9')

	if [ "$count" -gt "$MAX_SERVERS" ]; then
		note "$count servers running, $MAX_SERVERS allowed"
		for pid in "${servers[@]:0:$((count - MAX_SERVERS))}"; do
			stop_one "$pid" "over the limit"
		done
		continue
	fi

	if [ "${free_gb:-99}" -lt "$MIN_FREE_GB" ] && [ "$count" -gt 0 ]; then
		note "only ${free_gb}G memory left with $count servers running"
		stop_one "${servers[0]}" "memory running out"
		continue
	fi

	if [ "${disk_gb:-99}" -lt "$MIN_DISK_GB" ] && [ "$count" -gt 0 ]; then
		note "only ${disk_gb}G disk left with $count servers running"
		stop_one "${servers[0]}" "disk running out"
	fi
done
