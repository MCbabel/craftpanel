#!/usr/bin/env bash
set -uo pipefail

cd "$(dirname "$0")/.."

echo "typechecking our own frontend code"

log="$(mktemp)"
trap 'rm -f "$log"' EXIT

pnpm --filter @craftpanel/web typecheck >"$log" 2>&1

ours="$(grep -E ': error TS[0-9]+' "$log" | grep -vE '^(\.\./|node_modules/)' || true)"

if [ -z "$ours" ]; then
	echo "clean"
	exit 0
fi

printf '%s\n' "$ours"
printf '%s type errors in our own files\n' "$(printf '%s\n' "$ours" | wc -l | tr -d ' ')"
echo "FAILED — vendor/modrinth and node_modules do not typecheck under our"
echo "         configuration and are not ours to fix; everything else is."
exit 1
