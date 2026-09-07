"""Sixty-second problem-first tour; captures only owned windows across a real app restart."""
import json
import os
import shlex
import shutil
import subprocess as sp
import time

from rpc import call
import webview_probe

BEATS = (
    (10, 'Your agents are working in different repos. Which one needs you?'),
    (15, 'Needs you. Jump with Cmd+G and answer from the keyboard.'),
    (15, 'Cmd+K. Switch projects without hunting through terminal tabs.'),
    (18, 'App relaunched. Same agents, same terminal history.'),
    (2, 'One fleet. Desktop and terminal.'),
)
MAX_BYTES = 8_000_000


def wait(check, label, timeout=15):
    end = time.monotonic() + timeout
    while time.monotonic() < end:
        result = check()
        if result:
            print(f'[workflow] PASS {label}', flush=True)
            return result
        time.sleep(.2)
    raise RuntimeError(f'Workflow timed out: {label}')


def actor_state(root, **states):
    target = root / 'out/workflow-state.json'
    data = json.loads(target.read_text()) if target.exists() else {}
    data.update(states)
    pending = target.with_suffix('.pending')
    pending.write_text(json.dumps(data))
    pending.replace(target)


def status(root, lane):
    row = next(row for row in call(root, 'lane.list') if row['id'] == lane)
    (root / f'out/workflow-lane-{lane}.json').write_text(json.dumps(row, indent=2))
    (root / f'out/workflow-pane-{lane}.txt').write_text(call(root, 'agent.capture', {'lane_id': lane})['content'])
    return row['agent_sessions'][0]['status']


def sessions(root):
    # Stable backend identity, not display text, proves the app restart kept the actors alive.
    rows = call(root, 'lane.list')
    return {str(row['id']): [(s['id'], s['agent'], s.get('tmux_window'))
                            for s in row['agent_sessions']] for row in rows}


def run(root, helpers, repo, assignments, app, relaunch, dry_run, children):
    def phase(name):
        webview_probe.run_tour(root, 'workflow-' + name)
    def content(lane, window=None):
        return call(root, 'agent.capture', {'lane_id': lane, 'lines': 200, **({'window': window} if window else {})})['content']
    actor_state(root, **{actor: 'running' for actor in ('permission', 'hero')})
    # Disable automatic approval in this fixture: only the recorded Enter can answer it.
    call(root, 'supervision.set', {'lane_id': assignments['permission'], 'enabled': False})
    wait(lambda: all(status(root, assignments[a]) == 'running' for a in ('permission', 'hero')), 'actors running before the permission transition', 30)
    phase('start')
    baseline = sessions(root)
    def backend_panes():
        return sorted(sp.check_output(['tmux', '-L', root.name.replace('.', '-'), 'list-panes', '-a',
                                       '-F', '#{window_name}:#{pane_pid}:#{pane_id}'],
                                     env={'PATH': os.environ['PATH'], 'TMUX_TMPDIR': str(root / 'cache')},
                                     text=True).splitlines())
    panes_before = backend_panes()
    marker = 'Earlier result: navigation regression tests passed before app restart.'
    wait(lambda: marker in content(assignments['hero']), 'terminal content before restart')
    evidence = {'before_app_pid': app.pid, 'sessions_before': baseline, 'beats': [],
                'restart_edit': 'Window capture stops before quit and resumes after relaunch; startup wait is cut, with no replacement app frames.'}
    chunks = []
    if not dry_run:
        sp.run(['xcrun', 'swiftc', '-module-cache-path', str(root / 'cache/swift'), str(helpers / 'caption.swift'), '-o', str(root / 'bin/demo-caption')], check=True)
        # Compile once so starting a later window capture does not add Swift compiler waits.
        sp.run(['xcrun', 'swiftc', '-module-cache-path', str(root / 'cache/swift'), str(helpers / 'record.swift'), '-o', str(root / 'bin/demo-record')], check=True)
    def scene(index, action):
        seconds, caption = BEATS[index]
        recorder = None
        frames = root / f'out/workflow-{index}-frames'
        stop = root / f'out/workflow-{index}-stop'
        ready = root / f'out/workflow-{index}-ready'
        if not dry_run:
            window = json.loads(sp.check_output(['swift', str(helpers / 'window.swift'), str(app.pid), '--json', '--park-cursor'], text=True))
            assert window['pid'] == app.pid and window['id'] > 0
            assert abs(window['width'] / window['height'] - 1440 / 900) < .001, 'Unexpected window aspect ratio'
            recorder = sp.Popen([str(root / 'bin/demo-record'), str(app.pid), str(window['id']), str(frames), str(stop), str(ready)])
            children.append(recorder)
            def recording_ready():
                if recorder.poll() is not None:
                    raise RuntimeError('Owned-window capture failed; no full-display fallback')
                return ready.exists()
            wait(recording_ready, 'window capture ready', 30)
        started = time.monotonic()
        action()
        action_seconds = time.monotonic() - started
        if action_seconds > seconds:
            raise RuntimeError(f'Workflow beat {index} exceeded {seconds}s: {action_seconds:.2f}s; preserve logs and retry')
        while time.monotonic() - started < seconds:
            time.sleep(max(0, min(.2, seconds - (time.monotonic() - started))))
        if recorder:
            stop.write_text('finish')
            recorder.wait(timeout=30)
            if recorder.returncode:
                raise RuntimeError('Workflow window recording failed')
            overlay = root / f'out/caption-{index}.png'
            sp.run([str(root / 'bin/demo-caption'), str(helpers / 'fonts/SpaceGrotesk.ttf'), str(overlay), caption], check=True)
            chunk = root / f'out/workflow-{index}.mkv'
            # Captions occupy a separate bottom band, never cover terminal content.
            sp.run(['ffmpeg', '-y', '-v', 'error', '-f', 'concat', '-safe', '0', '-i', str(frames / 'frames.ffconcat'),
                    '-i', str(overlay), '-filter_complex',
                    scene_filter(seconds),
                    '-t', str(seconds), '-c:v', 'ffv1', str(chunk)], check=True)
            chunks.append(chunk)
        evidence['beats'].append({'start': sum(b[0] for b in BEATS[:index]), 'duration': seconds,
                                  'caption': caption, 'action_seconds': round(action_seconds, 3)})
    scene(0, lambda: None)
    def answer():
        actor_state(root, permission='permission')
        wait(lambda: status(root, assignments['permission']) == 'waiting', 'permission actor changed to NEEDS YOU')
        phase('answer')
        wait(lambda: (root / 'out/workflow-answer.txt').exists(), 'Enter reached the fake actor')
        wait(lambda: status(root, assignments['permission']) == 'running', 'answered agent continues')
    scene(1, answer)
    scene(2, lambda: phase('switch'))
    before_pid = app.pid
    app = relaunch()
    assert app.pid != before_pid
    assert sessions(root) == baseline, 'Session identities changed during app restart'
    assert backend_panes() == panes_before, 'Backend pane processes changed during app restart'
    evidence.update(after_app_pid=app.pid, sessions_after=sessions(root), backend_panes=panes_before)
    wait(lambda: marker in content(assignments['hero']), 'terminal history survives app restart')
    scene(3, lambda: phase('resume'))
    # Prepare the TUI before the final two-second dwell. It runs only on demo.sock.
    phase('terminal')
    terms = wait(lambda: call(root, 'terminal.list', {'lane_id': assignments['hero']}), 'sandbox terminal opened')
    terminal = terms[-1]
    command = f'{shlex.quote(str(root / "bin/repomon"))} --socket {shlex.quote(str(root / "demo.sock"))}'
    call(root, 'agent.send_input', {'lane_id': assignments['hero'], 'window': terminal, 'text': command, 'enter': True})
    tui = wait(lambda: (value if 'orbit-api' in (value := content(assignments['hero'], terminal)) and 'meadow-web' in value else None), 'TUI shows the same two repositories', 20)
    (root / 'out/workflow-tui.txt').write_text(tui)
    scene(4, lambda: phase('tui'))
    evidence['terminal'] = terminal
    (root / 'out/workflow-check.json').write_text(json.dumps(evidence, indent=2))
    if not dry_run:
        encode(root, chunks, repo / 'docs/workflow-demo.gif')
    print(f'[workflow] PASS 60 s tour; evidence: {root}/out/workflow-check.json', flush=True)


def scene_filter(seconds):
    # ScreenCaptureKit emits changed frames. Hold its final actual frame through quiet
    # periods so every scene keeps its allotted duration, including the static TUI.
    return (f'[0:v]tpad=stop_mode=clone:stop_duration={seconds},trim=duration={seconds},'
            'setpts=PTS-STARTPTS,fps=12,scale=1200:750:flags=lanczos,setsar=1,'
            'pad=1200:846:0:0:color=black[v];[v][1:v]overlay=0:750:eof_action=repeat,format=rgb24')


def encode(root, chunks, destination):
    manifest = root / 'out/workflow.ffconcat'
    manifest.write_text('ffconcat version 1.0\n' + ''.join(f"file '{p}'\n" for p in chunks))
    inputs = ['-f', 'concat', '-safe', '0', '-i', str(manifest)]
    palette, candidate = root / 'out/workflow-palette.png', root / 'out/workflow.gif'
    for fps, colors in ((12, 256), (10, 256), (8, 192), (6, 128)):
        base = f'fps={fps},format=rgb24'
        sp.run(['ffmpeg', '-y', '-v', 'error', *inputs, '-vf', f'{base},palettegen=max_colors={colors}', '-frames:v', '1', str(palette)], check=True)
        sp.run(['ffmpeg', '-y', '-v', 'error', *inputs, '-i', str(palette), '-filter_complex',
                f'[0:v]{base}[v];[v][1:v]paletteuse=dither=none:diff_mode=rectangle', '-loop', '0', str(candidate)], check=True)
        metadata = json.loads(sp.check_output(['ffprobe', '-v', 'error', '-show_entries', 'stream=width:format=duration', '-of', 'json', str(candidate)], text=True))
        assert metadata['streams'][0]['width'] == 1200 and abs(float(metadata['format']['duration']) - 60) < .3, metadata
        if candidate.stat().st_size < MAX_BYTES:
            shutil.copy2(candidate, destination)
            print(f'[workflow] Wrote {destination}: {candidate.stat().st_size} bytes, {fps} fps, {colors} colors', flush=True)
            return
    raise RuntimeError('Workflow GIF exceeds 8 MB; lossless frames retained for tuning')
