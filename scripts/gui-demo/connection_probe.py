"""Observe the demo desktop's RPC only; forward original frames to its private daemon."""
import json
from pathlib import Path
import socket
import struct
import threading
import time

MAX_FRAME = 64 * 1024 * 1024  # Matches repomon_core::protocol::MAX_FRAME_BYTES.


class DesktopProbe:
    def __init__(self, root, daemon_pid):
        self.root = Path(root).resolve()
        assert self.root.parent == Path('/private/tmp') and self.root.name.startswith('repomon-gui-demo.')
        self.endpoint = self.root / 'app.sock'
        self.backend = self.root / 'demo.sock'
        self.daemon_pid = daemon_pid
        self.app_pid = None
        self.ready = threading.Event()
        self.stopped = threading.Event()
        self.lock = threading.Lock()
        self.streams = []
        self.records = []
        self.log_path = self.root / 'data/logs/desktop-rpc.jsonl'
        self.log_path.parent.mkdir(parents=True, exist_ok=True)
        self.log_file = self.log_path.open('w')
        self.listener = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.listener.bind(str(self.endpoint))
        self.listener.listen()
        self.listener.settimeout(0.2)
        self.thread = threading.Thread(target=self.accept, daemon=True)
        self.thread.start()

    def record(self, **record):
        record['at'] = time.time()
        with self.lock:
            self.records.append(record)
            self.log_file.write(json.dumps(record) + '\n')
            self.log_file.flush()

    def expect_app(self, pid):
        self.app_pid = pid
        self.ready.set()

    def accept(self):
        while not self.stopped.is_set():
            try:
                client, _ = self.listener.accept()
            except socket.timeout:
                continue
            except OSError:
                return
            with self.lock:
                self.streams.append(client)
            try:
                # Darwin sys/un.h: SOL_LOCAL=0, LOCAL_PEERPID=2. These are kernel identities,
                # not process names or a PID supplied in a JSON request.
                peer = client.getsockopt(0, 2)
                if not self.ready.wait(5) or peer != self.app_pid:
                    raise RuntimeError(f'unexpected desktop endpoint peer: {peer}')
                backend = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
                with self.lock:
                    self.streams.append(backend)
                backend.connect(str(self.backend))
                daemon_peer = backend.getsockopt(0, 2)
                if daemon_peer != self.daemon_pid:
                    raise RuntimeError(f'unexpected sandbox daemon peer: {daemon_peer}')
                self.record(event='connected', app_pid=peer, daemon_pid=daemon_peer,
                            app_endpoint=str(self.endpoint), daemon_endpoint=str(self.backend))
                pending = {}
                for source, target, requests in ((client, backend, True), (backend, client, False)):
                    threading.Thread(target=self.forward, args=(source, target, pending, requests), daemon=True).start()
            except (OSError, RuntimeError) as error:
                self.record(event='error', message=str(error))
                client.close()

    def forward(self, source, target, pending, requests):
        def read(size):
            data = bytearray()
            while len(data) < size:
                chunk = source.recv(size - len(data))
                if not chunk:
                    raise EOFError
                data.extend(chunk)
            return bytes(data)
        try:
            while not self.stopped.is_set():
                header = read(4)
                size = struct.unpack('<I', header)[0]
                if size > MAX_FRAME:
                    raise RuntimeError(f'demo RPC frame exceeds {MAX_FRAME} bytes')
                body = read(size)
                message = json.loads(body)
                if requests:
                    method = message.get('method')
                    pending[message.get('id')] = method
                    evidence = {}
                    if method == 'viewport.set':
                        params = message.get('params') or {}
                        evidence = {key: params[key] for key in ('lane_ids', 'focus_lane') if key in params}
                    self.record(event='request', method=method, **evidence)
                elif 'id' in message:
                    method = pending.pop(message['id'], None)
                    result = message.get('result')
                    evidence = {}
                    if method in ('lane.list', 'repo.list') and isinstance(result, list):
                        evidence = {'count': len(result), 'ids': [row['id'] for row in result]}
                    if method == 'daemon.status' and isinstance(result, dict):
                        evidence = {key: result.get(key) for key in ('repos', 'lanes', 'version')}
                    if 'error' in message:
                        evidence['error'] = message['error'].get('message', 'RPC error')
                    self.record(event='response', method=method, **evidence)
                # Preserve every byte, request id, notification and response. Never log payloads,
                # terminal output, message bodies, config objects or identity tokens.
                target.sendall(header + body)
        except (EOFError, OSError):
            pass
        except (ValueError, KeyError, RuntimeError) as error:
            self.record(event='error', message=str(error))
        finally:
            for stream in (source, target):
                try:
                    stream.shutdown(socket.SHUT_RDWR)
                except OSError:
                    pass

    def evidence(self, lanes):
        with self.lock:
            records = list(self.records)
        errors = [row for row in records if row['event'] == 'error']
        if errors:
            raise RuntimeError(f'desktop RPC observer failed: {errors}')
        lane_ids = {lane['id'] for lane in lanes}
        repo_ids = {lane['repo']['id'] for lane in lanes}
        connected = next((row for row in records if row['event'] == 'connected'), None)
        def has_list(method, ids):
            return any(row['event'] == 'response' and row.get('method') == method
                       and set(row.get('ids', [])) == ids for row in records)
        viewport = next((row for row in records if row['event'] == 'request'
                         and row.get('method') == 'viewport.set' and row.get('lane_ids')
                         and set(row['lane_ids']) <= lane_ids), None)
        if connected and has_list('repo.list', repo_ids) and has_list('lane.list', lane_ids) and viewport:
            return {**connected, 'repos': len(repo_ids), 'lanes': len(lane_ids), 'viewport': viewport}
        return None

    def close(self):
        self.stopped.set()
        self.listener.close()
        self.thread.join(timeout=1)
        with self.lock:
            streams = list(self.streams)
        for stream in streams:
            try:
                stream.shutdown(socket.SHUT_RDWR)
            except OSError:
                pass
            stream.close()
        # Forwarders are daemon threads. Keep the log open until process exit so an in-flight
        # diagnostic can finish without writing to a closed handle.
        self.endpoint.unlink(missing_ok=True)
