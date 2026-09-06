"""Framed RPC, deliberately restricted to this run's private sandbox socket."""
import json
import socket
import struct
from pathlib import Path


def call(root, method, params=None):
    root = Path(root).resolve()
    assert root.name.startswith("repomon-gui-demo.") and root.parent == Path("/private/tmp")
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as client:
        client.settimeout(20)
        client.connect(str(root / "demo.sock"))
        request = {"jsonrpc": "2.0", "id": 1, "method": method, "params": params or {}}
        body = json.dumps(request).encode()
        client.sendall(struct.pack("<I", len(body)) + body)

        def read(count):
            result = b""
            while len(result) < count:
                part = client.recv(count - len(result))
                if not part:
                    raise ConnectionError("demo socket closed mid-frame")
                result += part
            return result

        while True:
            response = json.loads(read(struct.unpack("<I", read(4))[0]))
            if response.get("id") == 1:
                if "error" in response:
                    raise RuntimeError(f"{method}: {response['error']}")
                return response["result"]
