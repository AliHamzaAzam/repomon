//! Hosts a ConPTY child and terminal screen behind the framed pipe and registry protocol, allowing
//! the child to survive daemon restarts.

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
