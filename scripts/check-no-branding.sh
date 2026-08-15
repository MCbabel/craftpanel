#!/usr/bin/env bash
# Modrinth's UI is GPL-3.0, but its brand is not: logo, wordmark, mascot and the
# Modrinth Servers icon stay under "all rights reserved" (vendor/modrinth/ui/COPYING.md).
# They must never reappear in this repository.
set -uo pipefail

cd "$(dirname "$0")/.."

fail=0
note() { printf '  %s\n' "$1"; fail=1; }

paths=(
	"vendor/modrinth/ui/src/components/brand"
	"vendor/modrinth/ui/src/components/servers/ModrinthServersIcon.vue"
	"vendor/modrinth/ui/src/components/servers/marketing"
	"vendor/modrinth/assets/branding"
)

echo "checking for reintroduced Modrinth branding"

for path in "${paths[@]}"; do
	[ -e "$path" ] && note "trademarked path is back: $path"
done

identifiers='ModrinthIcon|ModrinthPlusIcon|ModrinthServersIcon|ModrinthHostingLogo|AnimatedLogo|TextLogo|Rinthbot|MedalBackgroundImage'

hits=$(grep -rlnE "$identifiers" \
	--include='*.ts' --include='*.vue' --include='*.js' --include='*.json' \
	vendor web crates 2>/dev/null || true)

if [ -n "$hits" ]; then
	while IFS= read -r file; do
		note "trademarked identifier referenced in: $file"
	done <<<"$hits"
fi

assets=$(find vendor web -type f \( -name 'rinthbot*' -o -name 'modrinth-plus*' -o -name 'logo.svg' \) 2>/dev/null || true)
if [ -n "$assets" ]; then
	while IFS= read -r file; do
		note "trademarked asset present: $file"
	done <<<"$assets"
fi

if [ "$fail" -eq 0 ]; then
	echo "clean"
else
	echo "FAILED — see vendor/modrinth/ui/COPYING.md"
fi

exit "$fail"
