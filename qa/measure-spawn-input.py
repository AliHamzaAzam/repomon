#!/usr/bin/env python3
"""Measure raw tmux delivery on a unique test server. Never contacts a daemon."""
import json
import os
from pathlib import Path
import shlex
import subprocess
import sys
import tempfile
import time

session = f'repomon-f1-measure-{os.getpid()}'
task = ("FIRST LINE: preserve the complete task.\n\n" + "Quoted 'task' $value `literal` 🦀\n" * 70 + "FINAL LINE").encode()

def tmux(*args, **kw):
    return subprocess.run(['tmux', '-L', session, *args], check=True, capture_output=True, **kw)

def wait_file(path):
    deadline = time.monotonic() + 5
    while not path.exists():
        if time.monotonic() >= deadline:
            print((path.parent / 'error').read_text(), flush=True)
            print(tmux('capture-pane', '-p', '-t', session).stdout.decode(), flush=True)
            raise RuntimeError(f'timed out: {path}')
        time.sleep(.02)

try:
    with tempfile.TemporaryDirectory(prefix='repomon-f1-') as root:
        root = Path(root)
        for mode in ['send-keys', 'paste-buffer', 'argv-claude', 'argv-codex']:
            out, ready = root / mode, root / (mode + '.ready')
            if mode.startswith('argv'):
                script = "import pathlib,sys,time;pathlib.Path(sys.argv[1]).write_bytes(sys.argv[2].encode());time.sleep(10)"
                args = [sys.executable, '-c', script, str(out), task.decode()]
            else:
                script = """import os,pathlib,sys,tty,time,select
tty.setraw(0)
os.write(1,b'\x1b[?2004h')
pathlib.Path(sys.argv[2]).touch()
data=b''
while True:
 if select.select([0],[],[],.3)[0]: data+=os.read(0,65536)
 elif data: break
pathlib.Path(sys.argv[1]).write_bytes(data)
time.sleep(10)
"""
                args = [sys.executable, '-c', script, str(out), str(ready)]
            command = shlex.join(args) + ' 2>' + shlex.quote(str(root / 'error')) + '; sleep 10'
            if mode == 'send-keys':
                tmux('new-session', '-d', '-s', session, '-n', mode, command)
            else:
                tmux('new-window', '-t', session, '-n', mode, command)
            target = f'{session}:={mode}'
            if not mode.startswith('argv'):
                wait_file(ready)
                if mode == 'send-keys':
                    tmux('send-keys', '-t', target, '-l', task.decode())
                else:
                    tmux('load-buffer', '-b', 'f1', '-', input=task)
                    tmux('paste-buffer', '-p', '-d', '-r', '-b', 'f1', '-t', target)
            wait_file(out)
            got = out.read_bytes()
            expected = b'\x1b[200~' + task + b'\x1b[201~' if mode == 'paste-buffer' else task
            print(json.dumps({'mode':mode, 'task_bytes':len(task), 'received_bytes':len(got), 'exact':got==expected, 'head_present':task[:40] in got, 'newlines':got.count(b'\n'), 'carriage_returns':got.count(b'\r')}), flush=True)
            if mode != 'send-keys':
                assert got == expected
finally:
    subprocess.run(['tmux','-L',session,'kill-server'], capture_output=True)
