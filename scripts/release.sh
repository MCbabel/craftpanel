#!/usr/bin/env bash
# Builds what install.sh downloads: one tarball per architecture, with a checksum.
#   scripts/release.sh [version]
set -euo pipefail

cd "$(dirname "$0")/.."

VERSION="${1:-$(grep -m1 '^version' Cargo.toml | sed -E 's/.*"([^"]+)".*/\1/')}"
# musl, statically linked, and not the machine's own glibc: a bundle built against
# a new glibc greets Debian 12 with "GLIBC_2.39 not found", which for something
# installed by one curl command is the end of the road. Nothing here stands in the
# way — the TLS is rustls, and no part of the tree calls getpwnam and friends, which
# is what static musl cannot do; the helper makes accounts through useradd instead.
TARGET="${CRAFTPANEL_TARGET:-$(uname -m)-unknown-linux-musl}"
OUT="dist"

say() { printf '==> %s\n' "$*"; }

# What install.sh asks the release for. Deliberately shorter than the Rust target
# triple: how the binaries are linked may change, the name people download should not.
# install.sh detect_arch builds the same two names — keep the two sides in step.
case "$TARGET" in
	x86_64-*)  SLUG="linux-x86_64" ;;
	aarch64-*) SLUG="linux-aarch64" ;;
	*) SLUG="linux-${TARGET%%-*}"
	   say "releases carry x86_64 and aarch64; $SLUG is a bundle for whoever asked for it" ;;
esac

[ -d "$(rustc --print sysroot)/lib/rustlib/$TARGET" ] ||
	{ echo "the $TARGET standard library is missing; run: rustup target add $TARGET" >&2
	  echo "and for musl targets the C compiler as well: apt install musl-tools" >&2; exit 1; }

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

# The whole point of the musl target. A bundle that came out dynamically linked
# would install fine here and fail on the first machine with an older libc.
if [ "${TARGET%-musl}" != "$TARGET" ] && command -v ldd >/dev/null; then
	for binary in craftpanel craftpanel-helper; do
		if ldd "target/$TARGET/release/$binary" 2>&1 | grep -q '=>'; then
			echo "$binary is dynamically linked; it was supposed to be static" >&2
			exit 1
		fi
	done
	say "both binaries are statically linked"
fi

mkdir -p "$OUT"
STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT

install -m 0755 "target/$TARGET/release/craftpanel" "$STAGE/craftpanel"
install -m 0755 "target/$TARGET/release/craftpanel-helper" "$STAGE/craftpanel-helper"
install -m 0644 LICENSE "$STAGE/LICENSE"
install -m 0644 COPYING.md "$STAGE/COPYING.md"

BUNDLE="$OUT/craftpanel-$SLUG.tar.gz"
tar -czf "$BUNDLE" -C "$STAGE" craftpanel craftpanel-helper LICENSE COPYING.md
sha256sum "$BUNDLE" | awk '{print $1}' > "$BUNDLE.sha256"

say "done"
printf '  %s  %s\n' "$(du -h "$BUNDLE" | cut -f1)" "$BUNDLE"
printf '  %s\n' "$(cat "$BUNDLE.sha256")"
