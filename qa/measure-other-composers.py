#!/usr/bin/env python3
"""Inspect advertised paste mode in isolated clients; never submit the synthetic input."""
import json, os, shlex, shutil, subprocess, tempfile, time
from pathlib import Path
for kind, flags in [('codex',['-c','mcp_servers={}','--no-alt-screen']), ('agy',[])]:
    server=f'repomon-f1-{kind}-{os.getpid()}'
    def tmux(*args, **kw):
        return subprocess.run(['tmux','-L',server,*args],capture_output=True,check=True,**kw).stdout
    try:
        with tempfile.TemporaryDirectory(prefix=f'repomon-f1-{kind}-') as root:
            raw=Path(root)/'output.raw'
            command='sleep 1; env REPOMON_MCP_SOCKET='+shlex.quote(str(Path(root)/'unused.sock'))+' '+shlex.join([shutil.which(kind),*flags])+'; sleep 30'
            tmux('new-session','-d','-s',server,'-x','220','-y','50','-c',root,command)
            tmux('pipe-pane','-t',server,'cat > '+shlex.quote(str(raw)))
            deadline=time.monotonic()+20
            ready=False
            while time.monotonic()<deadline:
                pane=tmux('capture-pane','-p','-t',server).decode()
                if 'Yes, continue' in pane and 'trust' in pane.lower():
                    time.sleep(1);tmux('send-keys','-t',server,'1');time.sleep(.3);tmux('send-keys','-t',server,'Enter')
                if any(line.strip().startswith(('› ','❯ ','> ')) or line.strip() in ('›','❯','>') for line in pane.splitlines()) and 'trust' not in pane.lower():
                    ready=True;break
                time.sleep(.2)
            output=raw.read_bytes() if raw.exists() else b''
            result={'kind':kind,'ready':ready,'advertised_bracketed_paste':b'\x1b[?2004h' in output}
            if ready:
                task='F1_HEAD_MARKER_'+('x'*8192)+'_F1_END_MARKER'
                tmux('load-buffer','-b','f1','-',input=task.encode())
                tmux('paste-buffer','-p','-d','-r','-b','f1','-t',server)
                time.sleep(.8)
                pane=tmux('capture-pane','-p','-S','-2000','-t',server).decode()
                result.update({'literal_wrapper_visible':'[200~' in pane or '[201~' in pane,'head_visible':'F1_HEAD_MARKER' in pane,'tail_visible':'_F1_END_MARKER' in pane})
            print(json.dumps(result),flush=True)
    finally:
        subprocess.run(['tmux','-L',server,'kill-server'],capture_output=True)
