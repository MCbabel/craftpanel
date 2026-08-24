#!/usr/bin/env bash
# Every shell script this repository ships, through shellcheck. install.sh weighs
# most: a stranger pipes it into bash as root, and a quoting mistake there is the
# expensive kind of mistake.
set -uo pipefail

cd "$(dirname "$0")/.." || exit 1

if ! command -v shellcheck >/dev/null 2>&1; then
	echo "FAILED — shellcheck is missing (Debian/Ubuntu: apt-get install shellcheck)"
	exit 1
fi

echo "shellchecking install.sh and scripts/*.sh with $(shellcheck --version | sed -n 's/^version: //p')"

if shellcheck install.sh scripts/*.sh; then
	echo "clean"
	exit 0
fi

echo "FAILED — fix it, or, where the check is wrong about this one line, silence"
echo "         exactly that one: '# shellcheck disable=SCxxxx  # and why'."
echo "         Never the whole file, never the whole check."
exit 1
