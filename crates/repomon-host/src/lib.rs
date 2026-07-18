//! repomon-agent-host: the per-agent host process for repomon's native Windows backend.
//!
//! One host per agent window — the Windows equivalent of a tmux window on the out-of-process
//! tmux server. The host owns a ConPTY + the agent child + a server-side vt100 screen with
//! scrollback, serves the length-prefixed JSON control protocol documented in `PROTOCOL.md`
//! on `\\.\pipe\repomon-<session>-<window>`, registers itself under
//! `<data_dir>\hosts\<session>\<window>.json`, and survives daemon restarts.
//!
//! Everything protocol- and screen-shaped is cross-platform and tested on every OS; only the
//! ConPTY spawn, the named-pipe server, and the pipe DACL are `cfg(windows)`.

pub mod cli;
pub mod codec;
pub mod dispatch;
pub mod keys;
pub mod protocol;
pub mod registry;
pub mod screen;

#[cfg(windows)]
pub mod dacl;
#[cfg(windows)]
pub mod pty;
#[cfg(windows)]
mod run;
#[cfg(windows)]
pub mod server;

#[cfg(windows)]
pub use run::windows_main;
