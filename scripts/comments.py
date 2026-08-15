#!/usr/bin/env python3
"""Takes the comments out of our own code — and nothing but the comments.

    scripts/comments.py                       show what would fall (writes nothing)
    scripts/comments.py --diff                the same as a diff
    scripts/comments.py --remove              write
    scripts/comments.py --check               guard: 1 if one is still there
    scripts/comments.py --check crates/craftpanel/src/main.rs

Without paths, `crates` and `web/src` apply; `vendor`, `node_modules`, `target`,
`dist` are always passed over, even when they are named outright.

A `//` inside a string is not a comment. That is why every file is read the way
its compiler reads it: Rust with raw strings (`r#"…"#`), character literals and
lifetimes that look the same; TypeScript with templates (`${…}`) and regular
expressions in which a slash is just a character; a .vue file as three languages
in one file.

The comments that a tool acts on stay: clap derives `--help` from the doc
comments of a derived struct, `<!--[if mso]>` is not a comment in a mail
template but an instruction to Outlook, and `@ts-expect-error` and its kind are
instructions to a tool, not to a reader.

The migrations (`*.sql`) are not touched: sqlx remembers the checksum of every
applied migration, and a file that changes afterwards stops the running database
at the next start.

Two nets catch a mistake of the reader before it reaches the disk: the text
without comments and without whitespace has to be the same before and after, and
so does the list of all the strings in the file.
"""
import argparse
import bisect
import dataclasses
import difflib
import os
import re
import sys

DEFAULT_PATHS = ("crates", "web/src")
SKIPPED = {"vendor", "node_modules", "target", "dist", ".git", "__pycache__", ".venv"}

SUFFIX_TO_LANGUAGE = {
    ".rs": "rust",
    ".ts": "ts",
    ".mts": "ts",
    ".cts": "ts",
    ".js": "ts",
    ".mjs": "ts",
    ".vue": "vue",
    ".scss": "scss",
    ".sass": "scss",
    ".less": "scss",
    ".css": "css",
    ".html": "html",
}

SKIPPED_WITH_REASON = {
    ".sql": "sqlx remembers the checksum of every migration; one comment less in "
            "it stops the running database",
}

TOOLING = re.compile(
    r"\[if\s[^\]]*\]|<!\[endif\]"
    r"|@ts-(ignore|expect-error|nocheck)"
    r"|eslint-(disable|enable)"
    r"|prettier-ignore|biome-ignore|deno-lint|dprint-ignore"
    r"|@vite-ignore|webpack(ChunkName|Ignore|Prefetch|Preload|Mode)"
    r"|@vitest-environment|@jest-environment"
    r"|istanbul ignore|c8 ignore|v8 ignore"
    r"|@license|@preserve|__PURE__|sourceMappingURL"
    r"|<reference\s|@jsxImportSource"
    r"|SPDX-License-Identifier|noqa|type: ignore"
)

RUST_RAW = re.compile(r'[bc]?r(#*)"')
RUST_BYTE = re.compile(r'[bc]"')
RUST_CHAR = re.compile(
    r"'(?:\\(?:x[0-9a-fA-F]{2}|u\{[0-9a-fA-F]{1,6}\}|.)|[^\\'\n])'",
    re.S,
)
RUST_DERIVE = re.compile(r"#\[derive\(([^)]*)\)\]")
CLAP_KINDS = ("Parser", "Subcommand", "Args", "ValueEnum")

TS_WORD = re.compile(r"[A-Za-z_$][\w$]*")
TS_KEYWORD = {
    "return", "typeof", "instanceof", "in", "of", "new", "delete", "void",
    "throw", "case", "do", "else", "yield", "await", "if", "while", "for",
    "switch", "as", "satisfies",
}
TS_BEFORE_REGEX = set("(,=:[!&|?{};+-*%^~<>")

SFC_OPENER = re.compile(r"^<(template|script|style|i18n|docs)((?:\s[^>]*)?)>", re.M)
HTML_OPENER = re.compile(r"<(script|style)((?:\s[^>]*)?)>", re.I)
HTML_NAME = re.compile(r"</?[A-Za-z!?]")


@dataclasses.dataclass
class Find:
    start: int
    end: int
    kind: str
    keep: bool = False
    reason: str = ""


@dataclasses.dataclass
class Parsed:
    finds: list
    marks: list
    unclear: int = 0


def is_word_char(c):
    return c.isalnum() or c == "_"


def up_to_char(text, i, char, end, with_escape=True, across_lines=True):
    """The index behind the closing character; on a break the index of the break."""
    while i < end:
        c = text[i]
        if with_escape and c == "\\":
            i += 2
        elif c == char:
            return i + 1, True
        elif c == "\n" and not across_lines:
            return i, False
        else:
            i += 1
    return end, False


def read_rust(text, start=0, end=None):
    end = len(text) if end is None else end
    finds, marks, unclear = [], [], 0
    i = start
    while i < end:
        c = text[i]
        if c == "/" and text.startswith("//", i):
            j = text.find("\n", i)
            j = end if j < 0 or j > end else j
            doc = text.startswith("//!", i) or (
                text.startswith("///", i) and not text.startswith("////", i)
            )
            finds.append(Find(i, j, "doc" if doc else "line"))
            i = j
        elif c == "/" and text.startswith("/*", i):
            depth, j = 1, i + 2
            while j < end and depth:
                if text.startswith("/*", j):
                    depth += 1
                    j += 2
                elif text.startswith("*/", j):
                    depth -= 1
                    j += 2
                else:
                    j += 1
            if depth:
                unclear += 1
            doc = (
                text.startswith("/**", i) or text.startswith("/*!", i)
            ) and not text.startswith("/**/", i)
            finds.append(Find(i, j, "doc" if doc else "block"))
            i = j
        elif c == '"':
            j, whole = up_to_char(text, i + 1, '"', end)
            unclear += 0 if whole else 1
            marks.append(text[i:j])
            i = j
        elif c == "'":
            hit = RUST_CHAR.match(text, i, end)
            if hit:
                marks.append(hit.group(0))
                i = hit.end()
            else:
                i += 1
        elif c in "rbc" and (i == start or not is_word_char(text[i - 1])):
            hit = RUST_RAW.match(text, i, end)
            if hit:
                closer = '"' + hit.group(1)
                j = text.find(closer, hit.end(), end)
                if j < 0:
                    unclear += 1
                    j = end
                else:
                    j += len(closer)
                marks.append(text[i:j])
                i = j
                continue
            hit = RUST_BYTE.match(text, i, end)
            if hit:
                j, whole = up_to_char(text, hit.end(), '"', end)
                unclear += 0 if whole else 1
                marks.append(text[i:j])
                i = j
            else:
                i += 1
        else:
            i += 1
    return Parsed(finds, marks, unclear)


def ts_regex_end(text, i, end):
    j = i + 1
    char_class = False
    while j < end:
        c = text[j]
        if c == "\\":
            j += 2
        elif c == "\n":
            return -1
        elif c == "[":
            char_class = True
            j += 1
        elif c == "]":
            char_class = False
            j += 1
        elif c == "/" and not char_class:
            j += 1
            while j < end and text[j].isalpha():
                j += 1
            return j
        else:
            j += 1
    return -1


def read_ts(text, start=0, end=None):
    end = len(text) if end is None else end
    finds, marks, unclear = [], [], 0
    i = start
    last_char, word = "", ""
    stack = ["code"]
    depth = [0]
    template_from = []
    while i < end:
        c = text[i]
        if stack[-1] == "vorlage":
            if c == "\\":
                i += 2
            elif c == "`":
                marks.append(text[template_from.pop():i + 1])
                stack.pop()
                last_char, word = "`", ""
                i += 1
            elif c == "$" and text.startswith("${", i):
                marks.append(text[template_from.pop():i + 2])
                stack.append("code")
                depth.append(0)
                last_char, word = "{", ""
                i += 2
            else:
                i += 1
            continue
        if c == "/" and text.startswith("//", i):
            j = text.find("\n", i)
            j = end if j < 0 or j > end else j
            finds.append(Find(i, j, "line"))
            i = j
            continue
        if c == "/" and text.startswith("/*", i):
            j = text.find("*/", i + 2, end)
            if j < 0:
                unclear += 1
                j = end
            else:
                j += 2
            doc = text.startswith("/**", i) and not text.startswith("/**/", i)
            finds.append(Find(i, j, "doc" if doc else "block"))
            i = j
            continue
        if c == "/" and (last_char == "" or last_char in TS_BEFORE_REGEX or word in TS_KEYWORD):
            j = ts_regex_end(text, i, end)
            if j > 0:
                marks.append(text[i:j])
                last_char, word = "/", ""
                i = j
                continue
        if c in "\"'":
            j, whole = up_to_char(text, i + 1, c, end, across_lines=False)
            unclear += 0 if whole else 1
            marks.append(text[i:j])
            last_char, word = c, ""
            i = j
            continue
        if c == "`":
            stack.append("vorlage")
            template_from.append(i)
            i += 1
            continue
        if c == "{":
            depth[-1] += 1
            last_char, word = c, ""
            i += 1
            continue
        if c == "}":
            if depth[-1] == 0 and len(stack) > 1:
                stack.pop()
                depth.pop()
                template_from.append(i)
            else:
                depth[-1] = max(0, depth[-1] - 1)
            last_char, word = c, ""
            i += 1
            continue
        if c.isspace():
            i += 1
            continue
        hit = TS_WORD.match(text, i, end)
        if hit:
            word = hit.group(0)
            last_char = word[-1]
            i = hit.end()
            continue
        last_char, word = c, ""
        i += 1
    return Parsed(finds, marks, unclear)


def read_css(text, start=0, end=None, line_comments=False):
    end = len(text) if end is None else end
    finds, marks, unclear = [], [], 0
    i = start
    while i < end:
        c = text[i]
        if text.startswith("/*", i):
            j = text.find("*/", i + 2, end)
            if j < 0:
                unclear += 1
                j = end
            else:
                j += 2
            finds.append(Find(i, j, "block"))
            i = j
        elif line_comments and text.startswith("//", i):
            j = text.find("\n", i)
            j = end if j < 0 or j > end else j
            finds.append(Find(i, j, "line"))
            i = j
        elif c in "\"'":
            j, whole = up_to_char(text, i + 1, c, end, across_lines=False)
            unclear += 0 if whole else 1
            marks.append(text[i:j])
            i = j
        elif text.startswith("url(", i):
            j, _ = up_to_char(text, i + 4, ")", end, with_escape=False)
            marks.append(text[i:j])
            i = j
        else:
            i += 1
    return Parsed(finds, marks, unclear)


def read_html(text, start=0, end=None):
    end = len(text) if end is None else end
    finds, marks, unclear = [], [], 0
    i = start
    while i < end:
        if text.startswith("<!--", i):
            j = text.find("-->", i + 4, end)
            if j < 0:
                unclear += 1
                j = end
            else:
                j += 3
            finds.append(Find(i, j, "html"))
            i = j
        elif text[i] == "<" and HTML_NAME.match(text, i, end):
            j = i + 1
            while j < end:
                c = text[j]
                if c in "\"'":
                    k, _ = up_to_char(text, j + 1, c, end, with_escape=False)
                    marks.append(text[j:k])
                    j = k
                elif c == ">":
                    j += 1
                    break
                else:
                    j += 1
            i = j
        else:
            i += 1
    return Parsed(finds, marks, unclear)


def block_end(text, name, i):
    if name != "template":
        j = text.find(f"</{name}>", i)
        return len(text) if j < 0 else j
    depth = 1
    pattern = re.compile(r"</?template[\s>]")
    while True:
        hit = pattern.search(text, i)
        if not hit:
            return len(text)
        depth += -1 if hit.group(0).startswith("</") else 1
        if depth == 0:
            return hit.start()
        i = hit.end()


def sfc_blocks(text):
    blocks = []
    for hit in SFC_OPENER.finditer(text):
        if blocks and hit.start() < blocks[-1][3]:
            continue
        name = hit.group(1)
        blocks.append((name, hit.group(2), hit.end(), block_end(text, name, hit.end())))
    return blocks


def style_language(attributes):
    return "scss" if re.search(r'lang\s*=\s*"(scss|sass|less)"', attributes) else "css"


def read_vue(text):
    finds, marks, unclear = [], [], 0
    read_to = 0
    for name, attributes, start, end in sfc_blocks(text):
        prefix = read_html(text, read_to, start)
        finds += prefix.finds
        marks += prefix.marks
        unclear += prefix.unclear
        if name == "template":
            part = read_html(text, start, end)
        elif name == "script":
            part = read_ts(text, start, end)
        elif name == "style":
            part = read_css(text, start, end, style_language(attributes) == "scss")
        else:
            part = Parsed([], [text[start:end]])
        finds += part.finds
        marks += part.marks
        unclear += part.unclear
        read_to = end
    rest = read_html(text, read_to, len(text))
    return Parsed(finds + rest.finds, marks + rest.marks, unclear + rest.unclear)


def read_html_file(text):
    finds, marks, unclear = [], [], 0
    read_to = 0
    for hit in HTML_OPENER.finditer(text):
        if hit.start() < read_to:
            continue
        name = hit.group(1).lower()
        end = block_end(text, name, hit.end())
        outside = read_html(text, read_to, hit.end())
        inside = (
            read_ts(text, hit.end(), end)
            if name == "script"
            else read_css(text, hit.end(), end, style_language(hit.group(2)) == "scss")
        )
        for part in (outside, inside):
            finds += part.finds
            marks += part.marks
            unclear += part.unclear
        read_to = end
    rest = read_html(text, read_to, len(text))
    return Parsed(finds + rest.finds, marks + rest.marks, unclear + rest.unclear)


READERS = {
    "rust": read_rust,
    "ts": read_ts,
    "vue": read_vue,
    "css": lambda text: read_css(text),
    "scss": lambda text: read_css(text, line_comments=True),
    "html": read_html_file,
}


def read(text, language):
    gelesen = READERS[language](text)
    gelesen.finds.sort(key=lambda f: f.start)
    return gelesen


def opaque(gelesen, text):
    """The stretches that hold no code: comments and strings."""
    spots = [(f.start, f.end) for f in gelesen.finds]
    from_pos = 0
    for mark in gelesen.marks:
        spot = text.find(mark, from_pos)
        if spot < 0:
            continue
        spots.append((spot, spot + len(mark)))
        from_pos = spot + len(mark)
    return sorted(spots)


def code_index(spots):
    starts = [a for a, _ in spots]

    def next_from(i):
        spot = bisect.bisect_right(starts, i) - 1
        if spot >= 0 and spots[spot][0] <= i < spots[spot][1]:
            return spots[spot][1]
        return i + 1

    return next_from


def clap_ranges(text, gelesen):
    """Bodies of structs whose doc comments come out as `--help`."""
    spots = opaque(gelesen, text)
    next_code = code_index(spots)
    ranges = []
    for hit in RUST_DERIVE.finditer(text):
        if not any(re.search(rf"\b{kind}\b", hit.group(1)) for kind in CLAP_KINDS):
            continue
        if next_code(hit.start()) != hit.start() + 1:
            continue
        head = head_before(text, hit.start())
        i = hit.end()
        depth = 0
        end = None
        while i < len(text):
            c = text[i]
            next_at = next_code(i)
            if next_at != i + 1:
                i = next_at
                continue
            if c == "{":
                depth += 1
            elif c == "}":
                depth -= 1
                if depth == 0:
                    end = i + 1
                    break
            elif c == ";" and depth == 0:
                end = i + 1
                break
            i += 1
        ranges.append((head, end if end else len(text)))
    return ranges


def head_before(text, spot):
    start = text.rfind("\n", 0, spot) + 1
    while start > 0:
        earlier = text.rfind("\n", 0, start - 1) + 1
        line = text[earlier:start].strip()
        if line.startswith("///") or line.startswith("#["):
            start = earlier
        else:
            break
    return start


def has_doctests(path):
    """Only a library crate can have doctests; a binary crate never has any."""
    folder = os.path.dirname(os.path.abspath(path))
    while True:
        if os.path.exists(os.path.join(folder, "Cargo.toml")):
            return os.path.exists(os.path.join(folder, "src", "lib.rs"))
        parent = os.path.dirname(folder)
        if parent == folder:
            return True
        folder = parent


def groups(text, finds):
    """Consecutive comment lines of the same kind are one block."""
    bundled = []
    for find in finds:
        if bundled:
            last = bundled[-1][-1]
            between = text[last.end:find.start]
            if last.kind == find.kind and between.strip() == "" and between.count("\n") <= 1:
                bundled[-1].append(find)
                continue
        bundled.append([find])
    return bundled


def judge(text, language, gelesen, doctests=False):
    """Sets for every find whether it stays, and why."""
    ranges = clap_ranges(text, gelesen) if language == "rust" else []
    for find in gelesen.finds:
        content = text[find.start:find.end]
        if TOOLING.search(content):
            find.keep = True
            find.reason = "an instruction to a tool"
        elif find.kind == "doc" and any(a <= find.start and find.end <= e for a, e in ranges):
            find.keep = True
            find.reason = "clap prints this as --help"
    if language == "rust" and doctests:
        for block in groups(text, gelesen.finds):
            if block[0].kind != "doc":
                continue
            if "```" not in "".join(text[f.start:f.end] for f in block):
                continue
            for find in block:
                find.keep = True
                find.reason = "doc comment with a code block (doctest?)"
    return gelesen


def line_starts(text):
    starts = [0]
    for hit in re.finditer("\n", text):
        starts.append(hit.end())
    return starts


def line_of(starts, spot):
    return bisect.bisect_right(starts, spot) - 1


def remove(text, finds):
    """Builds the text without the named comments, tidying the blank lines as it goes."""
    to_remove = [f for f in finds if not f.keep]
    if not to_remove:
        return text
    starts = line_starts(text)
    lines = text.split("\n")
    cuts = {}
    for find in to_remove:
        first = line_of(starts, find.start)
        last = line_of(starts, max(find.start, find.end - 1))
        for number in range(first, last + 1):
            a = max(find.start, starts[number]) - starts[number]
            limit = starts[number] + len(lines[number])
            b = min(find.end, limit) - starts[number]
            if b > a:
                cuts.setdefault(number, []).append((a, b))

    after = []
    seams = set()
    for number, line in enumerate(lines):
        if number not in cuts:
            after.append(line)
            continue
        rest = ""
        pos = 0
        for a, b in sorted(cuts[number]):
            if a < pos:
                continue
            rest += line[pos:a]
            pos = b
            if rest.strip() and rest[-1:] in (" ", "\t"):
                while pos < len(line) and line[pos] in " \t":
                    pos += 1
            elif rest[-1:].strip() and line[pos:pos + 1].strip():
                rest += " "
        rest += line[pos:]
        prefix = line[:sorted(cuts[number])[0][0]]
        if not prefix.strip():
            rest = prefix + rest[len(prefix):].lstrip()
        if not rest.strip():
            seams.add(len(after))
        else:
            after.append(rest.rstrip())
    return "\n".join(tidy_blank_lines(after, seams))


def tidy_blank_lines(lines, seams):
    """Tidies only where something was taken out."""
    if not seams:
        return lines
    result = []
    i = 0
    while i < len(lines):
        if lines[i].strip():
            result.append(lines[i])
            i += 1
            continue
        run = i
        while run < len(lines) and not lines[run].strip():
            run += 1
        touched = any(i <= spot <= run for spot in seams)
        previous = result[-1].rstrip() if result else ""
        following = lines[run].lstrip() if run < len(lines) else ""
        if not touched:
            result += lines[i:run]
        elif not result or run >= len(lines):
            pass
        elif previous.endswith(("{", "(", "[")) or following.startswith(("}", ")", "]")):
            pass
        else:
            result.append("")
        i = run
    while result and not result[-1].strip():
        result.pop()
    result.append("")
    return result


def skeleton(text, language):
    """The text without comments and without whitespace, plus all its strings."""
    gelesen = read(text, language)
    pieces = []
    pos = 0
    for find in gelesen.finds:
        pieces.append(text[pos:find.start])
        pos = find.end
    pieces.append(text[pos:])
    without = re.sub(r"\s+", "", "".join(pieces))
    return without, tuple(gelesen.marks), gelesen.unclear


@dataclasses.dataclass
class Result:
    path: str
    language: str
    before: str
    after: str
    finds: list
    complaints: list


def clean(text, language, path="<text>"):
    doctests = language == "rust" and path != "<text>" and has_doctests(path)
    gelesen = judge(text, language, read(text, language), doctests)
    after = remove(text, gelesen.finds)
    complaints = []
    before_text = skeleton(text, language)
    after_text = skeleton(after, language)
    if before_text[0] != after_text[0]:
        complaints.append("the code itself would have changed")
    if before_text[1] != after_text[1]:
        complaints.append("the strings would have changed")
    if before_text[2]:
        complaints.append(f"{before_text[2]}× unclear end of a string")
    return Result(path, language, text, after, gelesen.finds, complaints)


def files(paths):
    """The files that are read, and the ones left lying with a reason."""
    found, left_alone = [], []

    def sort_in(path):
        suffix = os.path.splitext(path)[1]
        if suffix in SKIPPED_WITH_REASON:
            left_alone.append(path)
        elif SUFFIX_TO_LANGUAGE.get(suffix):
            found.append(path)

    for path in paths:
        if skipped(path):
            continue
        if os.path.isfile(path):
            sort_in(path)
            continue
        for root, directories, names in os.walk(path):
            directories[:] = sorted(v for v in directories if v not in SKIPPED)
            for name in sorted(names):
                sort_in(os.path.join(root, name))
    return found, left_alone


def skipped(path):
    return any(part in SKIPPED for part in os.path.normpath(path).split(os.sep))


def first_line(text):
    line = text.strip().split("\n")[0].strip()
    return line if len(line) <= 96 else line[:93] + "…"


def line_number(text, spot):
    return text.count("\n", 0, spot) + 1


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("paths", nargs="*", default=list(DEFAULT_PATHS))
    parser.add_argument("--remove", action="store_true", help="write instead of show")
    parser.add_argument("--diff", action="store_true", help="show as a diff")
    parser.add_argument("--check", action="store_true", help="guard: 1 if one is there")
    parser.add_argument("--show-kept", action="store_true", help="list the ones that stay as well")
    parser.add_argument("--at-most", type=int, default=50, help="name this many finds")
    arguments = parser.parse_args(argv)
    paths = arguments.paths or list(DEFAULT_PATHS)

    total_gone = total_kept = total_files = named = 0
    broken = []
    lines_gone = 0
    to_read, left_alone = files(paths)
    for path in to_read:
        language = SUFFIX_TO_LANGUAGE[os.path.splitext(path)[1]]
        with open(path, encoding="utf-8") as handle:
            before = handle.read()
        result = clean(before, language, path)
        gone = [f for f in result.finds if not f.keep]
        kept = [f for f in result.finds if f.keep]
        if result.complaints:
            broken.append((path, result.complaints))
            continue
        if not gone and not (arguments.show_kept and kept):
            continue
        total_files += 1 if gone else 0
        total_gone += len(gone)
        total_kept += len(kept)
        lines_gone += before.count("\n") - result.after.count("\n")

        if arguments.check:
            for find in gone[:max(0, arguments.at_most - named)]:
                print(f"  {path}:{line_number(before, find.start)}: {first_line(before[find.start:find.end])}")
            named += len(gone)
        elif arguments.diff:
            sys.stdout.writelines(
                difflib.unified_diff(
                    before.splitlines(True), result.after.splitlines(True),
                    fromfile=path, tofile=path, n=2,
                )
            )
        elif arguments.remove:
            if result.after != before:
                with open(path, "w", encoding="utf-8") as handle:
                    handle.write(result.after)
            print(f"  {path}: {len(gone)} gone, {len(kept)} stay")
        else:
            print(f"  {path}: {len(gone)} gone, {len(kept)} stay")
            if arguments.show_kept:
                for find in kept:
                    spot = line_number(before, find.start)
                    print(f"      stays {path}:{spot}: {find.reason}")

    for path, reasons in broken:
        print(f"  {path}: NOT TOUCHED — {'; '.join(reasons)}")

    if arguments.check:
        if total_gone > arguments.at_most:
            print(f"  … and {total_gone - arguments.at_most} more")
        if total_gone:
            print(f"{total_gone} comments in {total_files} files")
        return 1 if (total_gone or broken) else 0

    print(f"{total_gone} comments in {total_files} files, {lines_gone} lines; "
          f"{total_kept} stay")
    for suffix, reason in SKIPPED_WITH_REASON.items():
        how_many = sum(1 for p in left_alone if p.endswith(suffix))
        if how_many:
            print(f"{how_many}× {suffix} is left lying: {reason}")
    return 1 if broken else 0


if __name__ == "__main__":
    sys.exit(main())
