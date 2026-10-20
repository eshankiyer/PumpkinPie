#!/usr/bin/env python3
"""
Differential oracle: ask vanilla and Pumpkin the same thing over RCON, diff.

This is what makes parity discovery SAFE to automate. The cardinal rule of
this project is that the model must never guess vanilla behaviour; two public
retractions came from exactly that. Here vanilla behaviour is not guessed, it
is observed: a real vanilla MC 26.2 server runs beside Pumpkin on the NUC, and
this sends an identical command to both and returns both answers. A mismatch
is a real, evidenced parity gap, not a hunch.

    python3 nuc_probe.py "seed"
    python3 nuc_probe.py "time query daytime" "gamerule doDaylightCycle"
"""

import socket
import struct
import sys

NUC = "192.168.1.93"
VANILLA = (NUC, 25576, "oracletest")
PUMPKIN = (NUC, 25575, "pumpkintest")


def _pkt(req_id, ptype, body):
    payload = struct.pack("<ii", req_id, ptype) + body.encode() + b"\x00\x00"
    return struct.pack("<i", len(payload)) + payload


def _read(sock):
    raw_len = sock.recv(4)
    if len(raw_len) < 4:
        return None, None, ""
    (length,) = struct.unpack("<i", raw_len)
    data = b""
    while len(data) < length:
        chunk = sock.recv(length - len(data))
        if not chunk:
            break
        data += chunk
    req_id, ptype = struct.unpack("<ii", data[:8])
    return req_id, ptype, data[8:-2].decode(errors="replace")


def rcon(host, port, password, command, timeout=8):
    """One command over RCON. Returns the server's response text."""
    with socket.create_connection((host, port), timeout=timeout) as s:
        s.settimeout(timeout)
        s.sendall(_pkt(1, 3, password))  # 3 = auth
        rid, _, _ = _read(s)
        if rid == -1:
            return "<AUTH FAILED>"
        s.sendall(_pkt(2, 2, command))  # 2 = command
        _, _, body = _read(s)
        return body.strip()


def diff(command):
    """Same command to both servers; return (vanilla, pumpkin, match)."""
    try:
        v = rcon(*VANILLA, command)
    except OSError as e:
        v = f"<vanilla unreachable: {e}>"
    try:
        p = rcon(*PUMPKIN, command)
    except OSError as e:
        p = f"<pumpkin unreachable: {e}>"
    return v, p, v == p


if __name__ == "__main__":
    cmds = sys.argv[1:] or ["seed", "time query daytime"]
    for c in cmds:
        v, p, ok = diff(c)
        mark = "MATCH" if ok else "DIFF "
        print(f"[{mark}] /{c}")
        print(f"   vanilla: {v}")
        print(f"   pumpkin: {p}")
