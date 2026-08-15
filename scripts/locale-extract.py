#!/usr/bin/env python3
"""Collects every id together with its defaultMessage from web/src and writes the English catalogue.

    scripts/locale-extract.py            writes web/src/locales/en-US/index.json
    scripts/locale-extract.py --check    only reports whether the catalogue matches the source
    scripts/locale-extract.py --list     shows the prefixes with their counts

What is read is the source, not the bundle: from .ts everything, from .vue only
the <script> blocks. What is looked for are objects that directly carry an id
and a defaultMessage — whether they sit in defineMessages or go to formatMessage
one at a time. Texts joined with + are put back together.

An id that only comes into being at run time cannot be found by any reader of
the source; it is reported and stands with its text under EXTRA.
"""
import argparse
import json
import os
import re
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SOURCE = os.path.join(ROOT, "web", "src")
CATALOGUE = os.path.join(SOURCE, "locales", "en-US", "index.json")
SUFFIXES = (".ts", ".mts", ".vue")
SKIPPED = {"node_modules", "dist", "locales", "__pycache__"}

EXTRA = {
    "server.settings.tabs.general": "General",
    "server.settings.tabs.installation": "Installation",
    "server.settings.tabs.network": "Network",
    "server.settings.tabs.properties": "Properties",
    "server.settings.tabs.advanced": "Advanced",
}

SCRIPT = re.compile(r"<script\b[^>]*>(.*?)</script>", re.S)
KEY = re.compile(r"\s*(?:([A-Za-z_$][\w$]*)|'([^']*)'|\"([^\"]*)\")\s*:")
ESCAPES = {"n": "\n", "t": "\t", "r": "\r", "b": "\b", "f": "\f", "v": "\v", "0": "\0"}

CODE, TEXT, NOISE = 0, 1, 2


def script_only(content):
    kept = [" "] * len(content)
    for hit in SCRIPT.finditer(content):
        for i in range(hit.start(1), hit.end(1)):
            kept[i] = content[i]
    return "".join(kept)


def unescape(raw):
    pieces = []
    i = 0
    while i < len(raw):
        char = raw[i]
        if char != "\\" or i + 1 >= len(raw):
            pieces.append(char)
            i += 1
            continue
        following = raw[i + 1]
        if following == "u" and raw[i + 2 : i + 3] == "{":
            end = raw.index("}", i)
            pieces.append(chr(int(raw[i + 3 : end], 16)))
            i = end + 1
        elif following == "u":
            pieces.append(chr(int(raw[i + 2 : i + 6], 16)))
            i += 6
        elif following == "x":
            pieces.append(chr(int(raw[i + 2 : i + 4], 16)))
            i += 4
        elif following == "\n":
            i += 2
        else:
            pieces.append(ESCAPES.get(following, following))
            i += 2
    return "".join(pieces)


def scan(content):
    kind = bytearray([CODE]) * len(content)
    texts = {}
    i = 0
    length = len(content)
    while i < length:
        char = content[i]
        if char in "'\"`":
            start = i
            i += 1
            while i < length:
                here = content[i]
                if here == "\\":
                    i += 2
                    continue
                if here == char:
                    i += 1
                    break
                i += 1
            texts[start] = (i, unescape(content[start + 1 : i - 1]))
            for k in range(start, min(i, length)):
                kind[k] = TEXT
        elif char == "/" and content[i + 1 : i + 2] == "/":
            start = i
            while i < length and content[i] != "\n":
                i += 1
            for k in range(start, i):
                kind[k] = NOISE
        elif char == "/" and content[i + 1 : i + 2] == "*":
            start = i
            end = content.find("*/", i + 2)
            i = length if end < 0 else end + 2
            for k in range(start, i):
                kind[k] = NOISE
        else:
            i += 1
    return kind, texts


def objects(content, kind):
    open_braces = []
    found = []
    for i, char in enumerate(content):
        if kind[i] != CODE:
            continue
        if char == "{":
            open_braces.append(i)
        elif char == "}" and open_braces:
            found.append((open_braces.pop(), i))
    return found


def fields(content, kind, start, end):
    bounds = []
    depth = 0
    piece = start + 1
    for i in range(start + 1, end):
        if kind[i] != CODE:
            continue
        char = content[i]
        if char in "{[(":
            depth += 1
        elif char in "}])":
            depth -= 1
        elif char == "," and depth == 0:
            bounds.append((piece, i))
            piece = i + 1
    bounds.append((piece, end))

    result = {}
    for begin, stop in bounds:
        hit = KEY.match(content, begin, stop)
        if not hit or kind[hit.end() - 1] != CODE:
            continue
        name = hit.group(1) or hit.group(2) or hit.group(3)
        result[name] = (hit.end(), stop)
    return result


def text_from(content, kind, texts, begin, stop):
    pieces = []
    i = begin
    while i < stop:
        if i in texts and kind[i] == TEXT:
            closer, value = texts[i]
            if content[i] == "`":
                return None
            pieces.append(value)
            i = closer
            continue
        if kind[i] == CODE and content[i] not in " \t\r\n+":
            return None
        if kind[i] == NOISE:
            return None
        i += 1
    return "".join(pieces) if pieces else None


def from_file(path):
    with open(path, encoding="utf-8") as handle:
        content = handle.read()
    if path.endswith(".vue"):
        content = script_only(content)
    kind, texts = scan(content)
    found = []
    unsure = []
    for start, end in objects(content, kind):
        entry = fields(content, kind, start, end)
        if "id" not in entry or "defaultMessage" not in entry:
            continue
        key = text_from(content, kind, texts, *entry["id"])
        message = text_from(content, kind, texts, *entry["defaultMessage"])
        line = content.count("\n", 0, start) + 1
        if key is None or message is None:
            unsure.append((line, content[entry["id"][0] : entry["id"][1]].strip()))
            continue
        found.append((key, message, line))
    return found, unsure


def files(root):
    for folder, subfolders, names in os.walk(root):
        subfolders[:] = sorted(u for u in subfolders if u not in SKIPPED)
        for name in sorted(names):
            if name.endswith(SUFFIXES):
                yield os.path.join(folder, name)


def gather(root):
    messages = dict(EXTRA)
    origin = {k: "scripts/locale-extract.py" for k in EXTRA}
    faults = []
    dynamic = []
    for path in files(root):
        short = os.path.relpath(path, ROOT)
        found, unsure = from_file(path)
        for line, expression in unsure:
            dynamic.append(f"{short}:{line} — id {expression}")
        for key, message, line in found:
            spot = f"{short}:{line}"
            if key in messages and messages[key] != message:
                faults.append(f"{key} stands twice and differently: {origin[key]} and {spot}")
            messages[key] = message
            origin.setdefault(key, spot)
    return messages, origin, faults, dynamic


def as_catalogue(messages):
    ordered = {k: {"defaultMessage": messages[k]} for k in sorted(messages)}
    return json.dumps(ordered, indent=2, ensure_ascii=False) + "\n"


def prefixes(messages):
    counted = {}
    for key in messages:
        head = key.split(".")[0] + "." if "." in key else key
        counted[head] = counted.get(head, 0) + 1
    return sorted(counted.items(), key=lambda p: (-p[1], p[0]))


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--check", action="store_true")
    parser.add_argument("--list", action="store_true")
    parser.add_argument("--catalogue", default=CATALOGUE)
    parser.add_argument("--source", default=SOURCE)
    arguments = parser.parse_args()

    messages, _, faults, dynamic = gather(arguments.source)

    for line in dynamic:
        print(f"put together, stands under EXTRA: {line}")

    if faults:
        for line in faults:
            print(f"FAILED — {line}")
        return 1

    wanted = as_catalogue(messages)

    if arguments.list:
        for head, count in prefixes(messages):
            print(f"{count:5}  {head}")
        print(f"{len(messages):5}  in total")
        return 0

    if arguments.check:
        if not os.path.exists(arguments.catalogue):
            print(f"FAILED — {arguments.catalogue} is missing, {len(messages)} messages are waiting")
            return 1
        with open(arguments.catalogue, encoding="utf-8") as handle:
            present = handle.read()
        if present == wanted:
            print(f"clean — {len(messages)} messages")
            return 0
        before = json.loads(present)
        missing = sorted(set(messages) - set(before))
        surplus = sorted(set(before) - set(messages))
        changed = sorted(
            k
            for k in set(before) & set(messages)
            if before[k].get("defaultMessage") != messages[k]
        )
        print(f"FAILED — {arguments.catalogue} does not match the source")
        for head, listing in (("missing", missing), ("surplus", surplus), ("changed", changed)):
            for key in listing[:20]:
                print(f"  {head}: {key}")
            if len(listing) > 20:
                print(f"  {head}: … and {len(listing) - 20} more")
        if not (missing or surplus or changed):
            print("  the order or the indentation differs")
        print("  write it: scripts/locale-extract.py")
        return 1

    os.makedirs(os.path.dirname(arguments.catalogue), exist_ok=True)
    with open(arguments.catalogue, "w", encoding="utf-8") as handle:
        handle.write(wanted)
    print(f"{len(messages)} messages in {os.path.relpath(arguments.catalogue, ROOT)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
