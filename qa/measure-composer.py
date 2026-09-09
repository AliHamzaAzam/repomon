#!/usr/bin/env python3
"""Inspect synthetic input in an isolated real CLI composer without submitting a task."""
import json, os, shlex, shutil, subprocess, tempfile, time
from pathlib import Path
session = f'repomon-f1-composer-{os.getpid()}'

def tmux(*args, **kw):
    return subprocess.run(['tmux','-L',session,*args],capture_output=True,check=True,**kw).stdout.decode()

def capture():
    return tmux('capture-pane','-p','-S','-2000','-t',session)

try:
    with tempfile.TemporaryDirectory(prefix='repomon-f1-composer-') as root:
        exported = Path(root) / 'composer.txt'
        editor = Path(root) / 'capture-editor'
        editor.write_text('#!/bin/sh\ncp "$1" ' + shlex.quote(str(exported)) + '\n: > "$1"\n')
        editor.chmod(0o700)
        command = 'env EDITOR=' + shlex.quote(str(editor)) + ' VISUAL=' + shlex.quote(str(editor)) + ' ' + shlex.join([shutil.which('claude'), '--strict-mcp-config', '--mcp-config', '{"mcpServers":{}}']) + '; sleep 30'
        tmux('new-session','-d','-x','220','-y','50','-s',session,'-c',root,command)
        deadline = time.monotonic()+20
        ready = False
        trusted = False
        while time.monotonic()<deadline:
            pane=capture()
            if not trusted and 'Yes, I trust this folder' in pane:
                time.sleep(1)
                tmux('send-keys','-t',session,'Down')
                time.sleep(.4)
                selected=capture()
                if '❯ Yes, I trust this folder' in selected:
                    tmux('send-keys','-t',session,'Enter')
                    trusted = True
            if any(line.strip()=='❯' for line in pane.splitlines()):
                ready=True
                break
            time.sleep(.2)
        if not ready:
            print(json.dumps({'ready':False,'pane_tail':pane[-1200:]}))
            raise SystemExit(1)
        # No Enter is sent after a payload. The test editor exports and clears the composer.
        for size in [1000,1500,1800,2000,2100,2500,4096,8192]:
            for mode in ['send-keys','paste-buffer']:
                task='F1_HEAD_MARKER_'+('x'*(size-28))+'_F1_END_MARKER'
                exported.unlink(missing_ok=True)
                if mode=='send-keys': tmux('send-keys','-t',session,'-l',task)
                else:
                    tmux('load-buffer','-b','f1','-',input=task.encode())
                    tmux('paste-buffer','-p','-d','-r','-b','f1','-t',session)
                time.sleep(.7)
                pane=capture()
                tmux('send-keys','-t',session,'C-g')
                export_deadline=time.monotonic()+5
                while not exported.exists() and time.monotonic()<export_deadline: time.sleep(.05)
                got=exported.read_text() if exported.exists() else None
                print(json.dumps({'mode':mode,'bytes':len(task),'exported_bytes':len(got.encode()) if got is not None else None,'head_intact':got.startswith('F1_HEAD_MARKER') if got else False,'exact':got.rstrip('\n')==task if got else False,'paste_placeholder':'[Pasted text' in pane}),flush=True)
                if got is None: raise SystemExit('External editor did not export composer')
                if mode == 'paste-buffer': assert got.rstrip('\n') == task
                time.sleep(.4)
finally:
    subprocess.run(['tmux','-L',session,'kill-server'],capture_output=True)
