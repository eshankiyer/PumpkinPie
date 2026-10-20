#!/usr/bin/env python3
"""
Behavioural verification: does the PATCHED Pumpkin now match vanilla?

The spree's clippy gate only proves a fix compiles. This proves it behaves,
by running the patched server and diffing it against the live vanilla oracle
on the NUC over RCON. Two entry points:

  verify_local(worktree, probes)  - option 2: build + boot the patched server
      on THIS machine on a spare port, probe it vs vanilla. Per-fix, no NUC
      rebuild. Fast-ish; contends with the model for RAM like clippy already
      does.

  verify_nuc(probes)              - option 1: probe whatever Pumpkin is live
      on the NUC (25575) vs vanilla (25576). To verify a NEW fix this way you
      must deploy_nuc() first (Docker rebuild on a 4-core i3: slow). Cheap as
      a final confirmation, not as an inner-loop gate.

A "probe" is one RCON command. Both servers share seed 1784236824834363978 and
level "world", so world-dependent probes are comparable; prefer state-setting
probes (setblock -> act -> query) so they do not depend on worldgen at all.
"""

import os
import subprocess
import time

from nuc_probe import VANILLA, rcon

NUC = "192.168.1.93"
PUMPKIN_NUC = (NUC, 25575, "pumpkintest")
LOCAL_RCON_PORT = 25599
LOCAL_RCON_PW = "spreetest"


def probe_diff(pumpkin_target, probes):
    """Run each probe on vanilla and on pumpkin_target; return rows.

    Row: (cmd, vanilla_out, pumpkin_out, match). match is the pass signal:
    the patched server agreeing with vanilla on the fixed behaviour.
    """
    rows = []
    for cmd in probes:
        try:
            v = rcon(*VANILLA, cmd)
        except OSError as e:
            v = f"<vanilla unreachable: {e}>"
        try:
            p = rcon(*pumpkin_target, cmd)
        except OSError as e:
            p = f"<pumpkin unreachable: {e}>"
        rows.append((cmd, v, p, v == p))
    return rows


def verify_nuc(probes):
    """Option 1: probe the currently-live NUC Pumpkin vs vanilla."""
    return probe_diff(PUMPKIN_NUC, probes)


def _wait_rcon(host, port, pw, timeout=90):
    """Block until the server answers RCON, or time out."""
    deadline = time.time() + timeout
    while time.time() < deadline:
        try:
            if rcon(host, port, pw, "seed"):
                return True
        except OSError:
            time.sleep(2)
    return False


def verify_local(worktree, probes, build_timeout=2400, boot_timeout=120):
    """Option 2: build + boot the patched server here, probe vs vanilla.

    Returns (rows, note). rows is probe_diff output, or [] if the server
    never came up (note says why). Everything runs in a throwaway server dir
    so the worktree source is untouched.
    """
    env = {**os.environ}
    # Debug build: faster to produce than release, and behaviour is identical.
    rc = subprocess.run(
        ["bash", "-c", "source ~/.cargo/env && cargo build -p pumpkin"],
        cwd=worktree, env=env, capture_output=True, text=True, timeout=build_timeout,
    ).returncode
    if rc != 0:
        return [], "patched server failed to build"

    binary = os.path.join(worktree, "target", "debug", "pumpkin")
    if not os.path.exists(binary):
        return [], "no pumpkin binary after build"

    srv = os.path.join(worktree, ".verify-server")
    os.makedirs(os.path.join(srv, "config"), exist_ok=True)
    # Match the oracle: same seed, offline, rcon on a spare port.
    with open(os.path.join(srv, "config", "pumpkin.toml"), "w") as f:
        f.write(
            'seed = "1784236824834363978"\n'
            "online_mode = false\n\n"
            "[networking.rcon]\n"
            "enabled = true\n"
            f'address = "0.0.0.0:{LOCAL_RCON_PORT}"\n'
            f'password = "{LOCAL_RCON_PW}"\n'
        )
    proc = subprocess.Popen(
        [binary], cwd=srv, env=env,
        stdout=open(os.path.join(srv, "boot.log"), "w"), stderr=subprocess.STDOUT,
    )
    try:
        if not _wait_rcon("127.0.0.1", LOCAL_RCON_PORT, LOCAL_RCON_PW, boot_timeout):
            return [], "patched server did not open RCON in time"
        rows = probe_diff(("127.0.0.1", LOCAL_RCON_PORT, LOCAL_RCON_PW), probes)
    finally:
        proc.terminate()
        try:
            proc.wait(timeout=15)
        except subprocess.TimeoutExpired:
            proc.kill()
        # Drop the throwaway world so verify runs do not accumulate worldgen
        # data (each boot generates spawn chunks; over many nights that adds up).
        import shutil
        shutil.rmtree(srv, ignore_errors=True)
    return rows, "ok"


def deploy_nuc(branch="live", timeout=3600):
    """Option 1 full: rebuild+restart Pumpkin on the NUC. Slow (i3 Docker)."""
    script = os.path.expanduser("~/pumpkin-bot/deploy_live.sh")
    if not os.path.exists(script):
        return False, "deploy_live.sh not found"
    rc = subprocess.run(
        ["bash", script], capture_output=True, text=True, timeout=timeout
    ).returncode
    return rc == 0, ("deployed" if rc == 0 else "deploy failed")


if __name__ == "__main__":
    import sys

    probes = sys.argv[1:] or ["seed", "difficulty"]
    print("=== option 1: live NUC pumpkin vs vanilla ===")
    for cmd, v, p, ok in verify_nuc(probes):
        print(f"[{'MATCH' if ok else 'DIFF '}] /{cmd}\n   vanilla: {v}\n   pumpkin: {p}")
