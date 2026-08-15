#!/usr/bin/env python3
"""Asks a Minecraft server over an address the way a client would.

    scripts/reachable-publicly.py 147.185.221.231:15878 mauritania-nice.tun.ply.gg

No TCP knocking: what goes out is the handshake and the status request of the server list ping,
and what counts is only what the server answers — version, MOTD, player count. With a tunnel an
open connection alone proves nothing, because its edge accepts even when nobody is listening
behind it.

Addresses without a port are treated as in the game: first `_minecraft._tcp.<name>` as SRV,
otherwise port 25565.
"""
import json
import socket
import struct
import subprocess
import sys


def varint(number: int) -> bytes:
    raw = b""
    while True:
        chunk = number & 0x7F
        number >>= 7
        raw += bytes([chunk | (0x80 if number else 0)])
        if not number:
            return raw


def read_varint(sock: socket.socket) -> int:
    number, shift = 0, 0
    while True:
        chunk = sock.recv(1)
        if not chunk:
            raise ConnectionError("the other side hung up in the middle of the number")
        number |= (chunk[0] & 0x7F) << shift
        if not chunk[0] & 0x80:
            return number
        shift += 7
        if shift > 35:
            raise ValueError("varint without an end")


def packet(content: bytes) -> bytes:
    return varint(len(content)) + content


def srv(name: str):
    """The detour the game itself takes. `dig` instead of a library, so that the script runs
    without extra packages."""
    try:
        out = subprocess.run(
            ["dig", "+short", "SRV", f"_minecraft._tcp.{name}"],
            capture_output=True, text=True, timeout=8,
        ).stdout.split()
    except (FileNotFoundError, subprocess.TimeoutExpired):
        return None
    if len(out) >= 4:
        return out[3].rstrip("."), int(out[2])
    return None


def ask(address: str, seconds: float = 10.0) -> dict:
    if address.startswith("["):
        host, _, rest = address[1:].partition("]")
        port = int(rest.lstrip(":")) if rest.lstrip(":") else 25565
    elif address.count(":") == 1:
        host, _, p = address.partition(":")
        port = int(p)
    else:
        host, port = address, 0

    detour = None
    if port == 0:
        detour = srv(host)
        target, port = detour if detour else (host, 25565)
    else:
        target = host

    sock = socket.create_connection((target, port), timeout=seconds)
    sock.settimeout(seconds)
    try:
        # The handshake carries the address the player typed in — tunnels and reverse proxies
        # decide from it where things go on to.
        handshake = (
            b"\x00" + varint(767)
            + varint(len(host.encode())) + host.encode()
            + struct.pack(">H", port) + varint(1)
        )
        sock.sendall(packet(handshake) + packet(b"\x00"))

        read_varint(sock)  # length
        if read_varint(sock) != 0:
            raise ValueError("the server answers with a foreign packet")
        length = read_varint(sock)
        raw = b""
        while len(raw) < length:
            piece = sock.recv(min(4096, length - len(raw)))
            if not piece:
                raise ConnectionError("the answer breaks off")
            raw += piece
    finally:
        sock.close()

    answer = json.loads(raw.decode("utf-8"))
    return {"target": f"{target}:{port}", "srv": detour, "answer": answer}


def main() -> int:
    if len(sys.argv) < 2:
        return print(__doc__) or 2

    bad = 0
    for address in sys.argv[1:]:
        try:
            found = ask(address)
            game = found["answer"]
            players = game.get("players", {})
            description = game.get("description")
            if isinstance(description, dict):
                description = description.get("text") or json.dumps(description)[:60]
            print(f"\033[32m✓\033[0m {address}")
            print(f"    over        {found['target']}" + (f"  (SRV {found['srv'][0]}:{found['srv'][1]})" if found["srv"] else ""))
            print(f"    version     {game.get('version', {}).get('name')}")
            print(f"    players     {players.get('online')} of {players.get('max')}")
            print(f"    MOTD        {description}")
        except Exception as fault:
            bad += 1
            print(f"\033[31m✗\033[0m {address}\n    {type(fault).__name__}: {fault}")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
