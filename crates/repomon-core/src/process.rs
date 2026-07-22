//! Child-process setup shared by daemon-facing helpers.

use std::ffi::OsStr;
use std::process::Command;

/// Start a utility process without allocating a visible console on Windows. The daemon invokes
/// Git and OS helpers during ordinary polling, so leaving the default creation flags can flash a
/// console even though the daemon itself is windowless. Other platforms use a normal command.
pub fn background_command<S: AsRef<OsStr>>(program: S) -> Command {
    let command = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let mut command = command;
        command.creation_flags(WINDOWS_CREATE_NO_WINDOW);
        command
    }
    #[cfg(not(windows))]
    command
}

pub const WINDOWS_CREATE_NO_WINDOW: u32 = 0x0800_0000;
pub const WINDOWS_CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creation_flags_match_winbase_contract() {
        assert_eq!(WINDOWS_CREATE_NO_WINDOW, 0x0800_0000);
        assert_eq!(WINDOWS_CREATE_NEW_PROCESS_GROUP, 0x0000_0200);
    }
}
