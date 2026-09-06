"""Believable local repositories and dated copies of the redacted ledger fixtures."""
import datetime as dt
import json
from pathlib import Path
import subprocess


def write(path, content):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(content)


def git(path, *args):
    return subprocess.check_output(["git", "-C", str(path), "-c", "user.name=Morgan Demo",
                                    "-c", "user.email=morgan@example.test", *args], text=True, env={"PATH": "/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin",
                                                     "GIT_CONFIG_GLOBAL": "/dev/null", "GIT_CONFIG_NOSYSTEM": "1"}).strip()


def seed(root, repo):
    files = {
        "orbit-api": {"package.json": '{"name":"orbit-api","private":true}\n',
                      "src/routes/rateLimit.ts": 'export const MAX_REQUESTS = 120;\nexport function remaining(used: number) {\n  return MAX_REQUESTS - used;\n}\n'},
        "meadow-web": {"package.json": '{"name":"meadow-web","private":true}\n',
                       "src/components/MobileMenu.tsx": '''import { useState } from "react";

export function MobileMenu() {
  const [open, setOpen] = useState(false);
  return (
    <nav aria-label="Main navigation">
      <button aria-expanded={open} onClick={() => setOpen(!open)}>
        Browse meadow
      </button>
      {open && <a href="/gardens">Explore gardens</a>}
    </nav>
  );
}
''', "public/navigation.svg": '''<svg xmlns="http://www.w3.org/2000/svg" width="800" height="480" viewBox="0 0 800 480">
<rect width="800" height="480" rx="24" fill="midnightblue"/>
<text x="48" y="84" font-family="sans-serif" font-size="30" fill="wheat">Meadow navigation</text>
<rect x="48" y="130" width="704" height="60" rx="12" fill="slateblue"/>
<text x="76" y="170" font-family="sans-serif" font-size="22" fill="white">Gardens       Journal       About</text>
<text x="48" y="278" font-family="sans-serif" font-size="24" fill="wheat">A quieter place to grow.</text>
<text x="48" y="328" font-family="sans-serif" font-size="18" fill="white">Keyboard focus follows the reading order.</text></svg>\n'''},
        "forge-cli": {"Cargo.toml": '[package]\nname = "forge-cli"\nversion = "0.3.0"\nedition = "2021"\n',
                      "src/main.rs": 'fn main() {\n    println!("forge: 12 checks passed");\n}\n'},
        "atlas-docs": {"mkdocs.yml": 'site_name: Atlas developer handbook\n',
                       "docs/quickstart.md": '# Quickstart\n\nCreate a lane, review the diff, and share the result.\n'},
    }
    for name, contents in files.items():
        path = root / "repos" / name
        path.mkdir(parents=True)
        git(path, "init", "-q", "-b", "main")
        for relative, content in contents.items():
            write(path / relative, content)
        write(path / "README.md", f"# {name}\n\nRepomon showcase fixture. All agents and usage are synthetic.\n")
        git(path, "add", "--", "README.md", *contents.keys())
        git(path, "commit", "-qm", "feat: establish project foundation")
        write(path / "CHANGELOG.md", "# Release notes\n\n- Improve keyboard navigation and platform checks.\n")
        git(path, "add", "--", "CHANGELOG.md")
        git(path, "commit", "-qm", "docs: prepare the next release")
    for slug, title in [("accessible-release", "Ship the accessible navigation release"),
                        ("windows-confidence", "Verify the Windows console experience")]:
        write(root / "repomind" / "plans" / "active" / f"{slug}.md",
              f"---\ntitle: {title}\nstatus: in flight\n---\n\n# {title}\n\n"
              "- [x] Assign the demo lanes\n- [ ] Review the regression coverage\n- [ ] Publish the release checklist\n")
    (root / "ledger" / "claude").mkdir(parents=True)
    (root / "ledger" / "codex").mkdir(parents=True)
    (root / "ledger" / "agy" / "cache").mkdir(parents=True)
    write(root / "ledger" / "agy" / "cache" / "last_conversations.json", "{}")


def seed_usage(root, repo, paths):
    fixtures = repo / "crates/repomon-core/src/usage_ledger/fixtures"
    # Dates are relative to capture day, always in the past, with stable unique request IDs.
    now = dt.datetime.now().astimezone()
    for day in range(7):
        for project, cwd in enumerate(paths):
            for kind in ("claude", "codex"):
                identity = f"demo-{kind}-{day}-{project}"
                capture_day = (now - dt.timedelta(days=day)).date()
                midnight = dt.datetime.combine(capture_day, dt.time(), tzinfo=now.tzinfo)
                # Leave every row in its local day, including a capture just after midnight.
                available = min(12 * 3600, max(1, (now - midnight).total_seconds() - 1))
                floor = min(available * 0.5, 6 * 3600)
                start = midnight + dt.timedelta(seconds=floor + (available - floor) * (0.4 + project * 0.1))
                multiplier = (1, 2, 1, 3, 2, 4, 2)[day] + project
                source = fixtures / f"{kind}_usage_v0.jsonl"
                rows = []

                def transform(value, key=""):
                    if isinstance(value, dict):
                        return {k: transform(v, k) for k, v in value.items()}
                    if isinstance(value, list):
                        return [transform(v) for v in value]
                    if isinstance(value, int) and key.endswith("tokens"):
                        return value * multiplier
                    if key == "cwd":
                        return str(cwd)
                    if key in ("sessionId", "session_id", "requestId", "uuid", "id", "turn_id") and isinstance(value, str):
                        return f"{identity}-{value}"
                    if key == "timestamp":
                        original = dt.datetime.fromisoformat(value.replace("Z", "+00:00"))
                        return (start + dt.timedelta(seconds=(original.minute * 60 + original.second) * available / 7200)).isoformat().replace("+00:00", "Z")
                    return value

                for line in source.read_text().splitlines():
                    row = transform(json.loads(line))

                    if row.get("message", {}).get("model") == "<synthetic>" or row.get("type") == "system":
                        continue
                    rows.append(json.dumps(row))
                folder = root / "ledger" / kind / (f"project-{project}" if kind == "claude" else start.strftime("%Y/%m/%d"))
                write(folder / f"{'rollout-' if kind == 'codex' else ''}{identity}.jsonl", "\n".join(rows) + "\n")
