#!/usr/bin/env bash
set -uo pipefail

cd "$(dirname "$0")/.." || exit 1

echo "checking for comments in our own code"

if ! command -v python3 >/dev/null 2>&1; then
	echo "FAILED — python3 is missing, scripts/comments.py cannot run"
	exit 1
fi

python3 scripts/comments.py --check "$@"
state=$?

if [ "$state" -eq 0 ]; then
	echo "clean"
else
	echo "FAILED — the reason belongs in docs/, not in the code."
	echo "         take out:  python3 scripts/comments.py --remove"
	echo "         look at:   python3 scripts/comments.py --diff"
	echo "         What may stay (clap help, @ts-expect-error and"
	echo "         their kind) is listed in scripts/comments.py under TOOLING."
fi

exit "$state"
