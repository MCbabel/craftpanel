#!/usr/bin/env python3
"""Every `file.rs:123` in docs/, held against the file it names.

The documents point into the code by line number, and the code moves. Twice now a
round of work has left docs/ naming lines that had shifted underneath it, and both
times a human found it by reading. This is the machine reading instead.

Two questions are asked, and only the first one can fail a build:

  1. Does the line exist?  A reference to `cgroup.rs:98-107` in a file of 97 lines
     is wrong no matter what anybody meant. There is no judgement in this and no
     way for it to be a false alarm, so it is the guard.

  2. Is the named thing still there?  Where the sentence puts an identifier next
     to the reference — ``ensure_accounts` (`install.sh:267`)` — the identifier
     ought to appear within a few lines of the target. This one is a good hint and
     not a proof: the identifier may be a caller rather than the definition, and a
     range may legitimately hold neither. It is counted on every run and listed
     with --anchors, but it does not fail the build, because the number of stale
     line numbers in docs/ is far larger than one round of work can put right and
     a guard that is red for months is a guard nobody reads.

What is deliberately not checked:

  * vendor/ — the vendored Modrinth tree is a trimmed copy and the documents cite
    line numbers from Modrinth's own repository. Half of `docs/api/` would light up.
  * A bare `:849` that continues an earlier sentence. Which file it continues is a
    guess, and the wrong guess is worse than no check.
  * A path that names nothing in this repository (`access.vue:182`). That is a
    reference into somebody else's source and there is nothing here to hold it to.
"""

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
NOT_OURS = {"vendor", "node_modules", "target", ".git", "dist", "__pycache__"}
OUR_ROOTS = {"crates", "web", "scripts", "docs", ".github"}
WINDOW = 6

REFERENCE = re.compile(
    r"`(?P<path>[A-Za-z0-9_][A-Za-z0-9_./-]*\.[A-Za-z][A-Za-z0-9]*)"
    r":(?P<first>\d+)(?:[-–](?P<last>\d+))?`"
)
IDENTIFIER = re.compile(r"`([A-Za-z_][A-Za-z0-9_]{3,}(?:::[A-Za-z_][A-Za-z0-9_]*)*)`")


def repository_files():
    by_suffix = {}
    for path in ROOT.rglob("*"):
        if not path.is_file():
            continue
        relative = path.relative_to(ROOT)
        if NOT_OURS.intersection(relative.parts):
            continue
        parts = relative.as_posix().split("/")
        for start in range(len(parts)):
            by_suffix.setdefault("/".join(parts[start:]), []).append(relative.as_posix())
    return by_suffix


def main():
    listing = "--anchors" in sys.argv[1:]
    by_suffix = repository_files()
    contents = {}

    def lines_of(name):
        if name not in contents:
            contents[name] = (ROOT / name).read_text(errors="replace").splitlines()
        return contents[name]

    print("checking the line numbers docs/ points at")

    broken, drifted, unchecked = [], [], 0

    for document in sorted(ROOT.glob("docs/**/*.md")):
        shown = document.relative_to(ROOT).as_posix()
        for number, line in enumerate(document.read_text().splitlines(), 1):
            for found in REFERENCE.finditer(line):
                named = found.group("path")
                candidates = by_suffix.get(named, [])
                if not candidates:
                    # Only a path that starts in one of our own directories is
                    # certainly ours; anything else names a foreign repository.
                    if named.split("/")[0] in OUR_ROOTS:
                        broken.append((shown, number, found.group(0), "no such file"))
                    else:
                        unchecked += 1
                    continue

                first = int(found.group("first"))
                last = int(found.group("last") or first)

                # An ambiguous name (`main.rs`) is held against every candidate and
                # only complained about when no file it could mean is long enough.
                longest = max(len(lines_of(c)) for c in candidates)
                if last > longest or first < 1:
                    where = candidates[0] if len(candidates) == 1 else f"{len(candidates)} files named that"
                    broken.append((shown, number, found.group(0), f"{where} ends at line {longest}"))
                    continue

                if len(candidates) != 1:
                    unchecked += 1
                    continue

                target = candidates[0]
                body = lines_of(target)

                nearest = None
                for other in IDENTIFIER.finditer(line):
                    if REFERENCE.fullmatch(other.group(0)):
                        continue
                    away = abs(other.start() - found.start())
                    if nearest is None or away < nearest[0]:
                        nearest = (away, other.group(1))
                if nearest is None:
                    continue
                name = nearest[1].split("::")[-1]
                if not any(name in text for text in body):
                    continue
                near = body[max(0, first - 1 - WINDOW):last + WINDOW]
                if not any(name in text for text in near):
                    drifted.append((shown, number, found.group(0), target, name))

    if listing:
        print()
        print(f"{len(drifted)} references whose neighbouring name is not within {WINDOW} lines:")
        for shown, number, reference, target, name in drifted:
            print(f"  {shown}:{number}  {reference} — `{name}` is not near there in {target}")

    print()
    if broken:
        for shown, number, reference, why in broken:
            print(f"  {shown}:{number}  {reference} — {why}")
        print()
        print(f"FAILED — {len(broken)} reference(s) point past the end of the file they name.")
        print("         Open the file, find what the sentence is about, write that line down.")
        return 1

    print(f"clean — every line referenced in docs/ exists ({unchecked} not checkable, see the top of this file)")
    if drifted:
        print(f"  {len(drifted)} of them name a symbol that has moved away; list them with --anchors")
    return 0


if __name__ == "__main__":
    sys.exit(main())
