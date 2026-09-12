# Shared Cargo target directory

Every Repomon lane is a separate `git worktree`, and by default each one carries its own
`target/`. Measured on the operator's machine: 20 to 28 GB per lane. Five lanes plus the main
checkout filled the disk mid-release (`No space left on device`), and once even failed
`cargo test --workspace` with `StorageFull`, which looks exactly like a test regression until
you read the error.

This makes a repo's lanes share one Cargo build directory instead.

## The mechanism

`scripts/setup-shared-cargo-target.sh` writes a single generated file:

```
<lanes-root>/.cargo/config.toml
```

`<lanes-root>` is the directory that holds every lane's worktree - by default
`~/code/{repo}-wt`, Repomon's `worktree_root_template` default (`~/code/{repo}-wt/{branch}`).
That directory is one level *above* every lane, is not itself a git worktree, and is never
touched by `git worktree remove`.

Cargo walks the directory tree from wherever it is invoked up to the filesystem root,
collecting every `.cargo/config.toml` it passes and merging them (closer files win on a key
they both set). So this one generated file is picked up automatically by `cargo` run from
inside *any* lane under that root - no matter whether the invocation is the daemon spawning an
agent's window, or the operator typing `cargo test` by hand in a lane - with no env var, no
shell rc file, no per-lane setup, and no change to Repomon's daemon or client code. That is
also why this lives entirely in `scripts/` and `docs/`: build configuration and tooling, not
`crates/` or `apps/desktop/src`.

Run once per machine (idempotent, safe to re-run):

```sh
scripts/setup-shared-cargo-target.sh
```

## The five decisions

**1. Where it lives.** The shared directory is `<cache root>/<repo>`, one subdirectory per
repo: `~/Library/Caches/repomon-cargo-target/<repo>` on macOS, or
`${XDG_CACHE_HOME:-~/.cache}/repomon-cargo-target/<repo>` elsewhere (override the parent with
`REPOMON_CARGO_TARGET_ROOT`). `<repo>` comes from the lanes-root directory name
(`{repo}-wt` with the suffix stripped), so two different repos' lanes roots
(`repomon-wt`, `repomon-ios-wt`, ...) always resolve to different subdirectories. The
directory itself sits outside every worktree - deleting a lane only ever removes that lane's
own checkout, never the shared cache.

**2. Concurrency.** Verified directly, not assumed: two `cargo build` invocations started
concurrently against the same `CARGO_TARGET_DIR` do not corrupt each other. Cargo holds a file
lock on the build directory for the duration of the build; a build that starts while another
build already holds the lock prints exactly:

```
Blocking waiting for file lock on build directory
```

and then proceeds once the first build releases the lock (finishes, or errors out). Two lanes
building at the same time serialize; they never race on the same target directory. The
operator sees the build progress bar pause with that one line for however long the other
lane's build takes.

**3. Who gets the variable.** Nobody needs one. This isn't an environment variable injected
into the daemon's spawn or the operator's shell - it is a `.cargo/config.toml` that Cargo
itself finds by walking up the directory tree from wherever `cargo` is invoked. That is true
of the daemon-spawned agent's own `cargo test`, and equally true of a worker who opens a
terminal in that lane and runs `cargo test` by hand: both start from a cwd under the lanes
root, and both find the same generated file the same way, because it is Cargo's own config
resolution doing the work, not anything Repomon does per invocation.

**4. Opt out.** For a one-off (e.g. bisecting a build issue that must not touch the shared
cache), override per invocation - Cargo's `CARGO_TARGET_DIR` env var and `--target-dir` flag
both take precedence over any `.cargo/config.toml`, including this generated one:

```sh
cargo build --target-dir target      # or: CARGO_TARGET_DIR=target cargo build
```

For a standing opt-out scoped to one lane, add a `.cargo/config.toml` inside that lane (closer
to the crate root than the lanes-root one, so it wins on `target-dir`):

```toml
[build]
target-dir = "target"
```

Repomon's repo already tracks a `.cargo/config.toml` for the Windows `rustflags` (see that
file); adding a `[build]` table to it locally does the same thing. Don't commit that local
addition - it's a per-worktree escape hatch, not a repo-wide default.

**5. Existing lanes.** Nothing is deleted automatically - that decision belongs to the
operator, every time (see fix rules). A lane already has its own `target/` sitting inside its
worktree; after `setup-shared-cargo-target.sh` runs, that lane's *next* `cargo` invocation
starts using the shared directory instead, and the old `target/` is simply dead weight from
then on. There is no migration step beyond running the setup script once - old directories are
reclaimed only when the operator chooses to, with an ordinary:

```sh
du -sh ~/code/repomon-wt/*/target   # see what's reclaimable
rm -rf ~/code/repomon-wt/<lane>/target
```

## Verifying it

`scripts/test-shared-cargo-target.sh` exercises the mechanism end to end against throwaway
directories (never the operator's real `~/code` layout): two lanes of one repo resolve to the
identical target directory, two different repos resolve to different ones, re-running the
setup script is a no-op, and both `cargo metadata`-observed defaults and the lane-local opt-out
are checked live against a real `cargo` binary when one is on `PATH`. It is not one of the
required gates (it's shell, not Rust, and it touches no tracked source), so it isn't wired into
`cargo test --workspace`; run it by hand:

```sh
bash scripts/test-shared-cargo-target.sh
```

Measured before/after disk use and a second lane's first-build wall clock, taken the same way
on this workspace, are recorded in the (gitignored) QA report rather than here, since they are
a point-in-time measurement, not a standing fact about the mechanism.
