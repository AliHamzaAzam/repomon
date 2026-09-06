"""Reproduce the one-off audit measurements in a fresh fixture directory under qa/."""
from pathlib import Path
import subprocess
import sys

root = Path(__file__).resolve().parents[2]
output = (root / (sys.argv[1] if len(sys.argv) > 1 else 'qa/evidence/query-recheck')).resolve()
if not output.is_relative_to(root / 'qa'):
    raise SystemExit('Fixture output must stay below this worktree\'s qa directory')
if (output / 'session-query-fixture.db').exists():
    raise SystemExit('Choose a fresh output directory; an existing fixture will not be overwritten')
output.mkdir(parents=True, exist_ok=True)
subprocess.run(['cargo', 'build', '-p', 'repomon-core'], cwd=root, check=True)
args = ['rustc', '--edition=2024', 'qa/audit-round-4/measure.rs', '-L',
        'dependency=target/debug/deps', '-o', str(output / 'measure')]
for name in ['rusqlite', 'serde_json']:
    artifact = max((root / 'target/debug/deps').glob('lib' + name + '-*.rlib'),
                   key=lambda path: path.stat().st_mtime)
    args += ['--extern', name + '=' + str(artifact)]
subprocess.run(args, cwd=root, check=True)
with (output / 'session-query-measure.log').open('w') as log:
    subprocess.run([str(output / 'measure'), str(output)], cwd=root,
                   stdout=log, check=True)
print(output / 'session-query-measure.json')
