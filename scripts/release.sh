#!/usr/bin/env bash
# Builds what install.sh downloads: one tarball per architecture, with a checksum.
#   scripts/release.sh [version]
set -euo pipefail

cd "$(dirname "$0")/.."

VERSION="${1:-$(grep -m1 '^version' Cargo.toml | sed -E 's/.*"([^"]+)".*/\1/')}"
TARGET="${CRAFTPANEL_TARGET:-$(rustc -vV | sed -n 's/^host: //p')}"
OUT="dist"

say() { printf '==> %s\n' "$*"; }

say "building the interface"
pnpm install --frozen-lockfile >/dev/null
pnpm --filter @craftpanel/web build >/dev/null

# The panel serves the interface out of its own binary, so the interface has to
# exist before the binary is compiled — rust-embed reads web/dist at build time.
[ -f web/dist/index.html ] || { echo "web/dist is empty; the interface did not build" >&2; exit 1; }

say "building CraftPanel $VERSION for $TARGET"
cargo build --release --target "$TARGET" -p craftpanel -p craftpanel-helper

say "checking that no Modrinth branding slipped back in"
./scripts/check-no-branding.sh

mkdir -p "$OUT"
STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT

install -m 0755 "target/$TARGET/release/craftpanel" "$STAGE/craftpanel"
install -m 0755 "target/$TARGET/release/craftpanel-helper" "$STAGE/craftpanel-helper"
install -m 0644 LICENSE "$STAGE/LICENSE"
install -m 0644 COPYING.md "$STAGE/COPYING.md"

BUNDLE="$OUT/craftpanel-$TARGET.tar.gz"
tar -czf "$BUNDLE" -C "$STAGE" craftpanel craftpanel-helper LICENSE COPYING.md
sha256sum "$BUNDLE" | awk '{print $1}' > "$BUNDLE.sha256"

say "done"
printf '  %s  %s\n' "$(du -h "$BUNDLE" | cut -f1)" "$BUNDLE"
printf '  %s\n' "$(cat "$BUNDLE.sha256")"
