"""Fail closed on Unix socket peers outside the copied desktop and its private RPC proxy."""
import json
from pathlib import Path
import subprocess as sp
import time


def records(output):
    # Darwin omits Unix PCB addresses from -F D fields. The ordinary DEVICE column
    # includes them, and is necessary to join an unnamed client to its named peer.
    result = []
    for line in output.splitlines()[1:]:
        parts = line.split()
        if len(parts) < 8 or parts[4] != "unix":
            continue
        result.append({"pid": int(parts[1]), "fd": parts[3], "t": parts[4],
                       "D": parts[5], "n": " ".join(parts[7:])})
    return result


def check(rows, root, app_pid, proxy_pid):
    root = Path(root).resolve()
    devices = {}
    for row in rows:
        devices.setdefault(row.get("D"), []).append(row)
    app_rows = [row for row in rows if row["pid"] == app_pid and row.get("t") == "unix"]
    assert app_rows, "lsof returned no desktop Unix sockets"
    evidence = []
    for row in app_rows:
        name = row.get("n", "")
        assert name.startswith("->0x"), f"Unexpected desktop socket: {row}"
        peers = devices.get(name[2:], [])
        assert peers, f"Unresolved desktop socket peer: {row}"
        if all(peer["pid"] == app_pid for peer in peers):
            kind = "desktop socketpair"
        else:
            assert all(peer["pid"] == proxy_pid for peer in peers), f"External peer: {peers}"
            assert all(Path(peer.get("n", "")).resolve() == root / "app.sock" for peer in peers), f"Non-sandbox endpoint: {peers}"
            kind = "sandbox RPC"
        evidence.append({"fd": row["fd"], "kind": kind, "peers": peers})
    assert any(row["kind"] == "sandbox RPC" for row in evidence), "No sandbox RPC socket"
    return evidence


def verify(root, app_pid, proxy_pid):
    ip = sp.run(["/usr/sbin/lsof", "-nP", "-a", "-p", str(app_pid), "-i", "-F", "pftn"],
                capture_output=True, text=True)
    (root / "out/desktop-lsof-ip.txt").write_text(ip.stdout)
    assert ip.returncode == 1 and not ip.stdout, f"Desktop has IP sockets or lsof failed: {ip}"
    # One lsof invocation observes the app and its accepting proxy. A connection established
    # between process scans can briefly lack a peer, so retry a bounded number of snapshots.
    for attempt in range(5):
        command = ["/usr/sbin/lsof", "-nP", "-a", "-p", f"{app_pid},{proxy_pid}", "-U"]
        result = sp.run(command, capture_output=True, text=True, check=True)
        (root / "out/desktop-lsof.txt").write_text(result.stdout)
        (root / "out/desktop-lsof-stderr.txt").write_text(result.stderr)
        try:
            evidence = check(records(result.stdout), root, app_pid, proxy_pid)
            (root / "out/desktop-socket-audit.json").write_text(json.dumps(evidence, indent=2))
            return evidence
        except AssertionError:
            if attempt == 4:
                raise
            time.sleep(0.5)
