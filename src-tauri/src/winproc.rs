use std::process::Command;

/// Windows shows a console window whenever a console subsystem process is
/// spawned, even briefly. afterm's SFTP and telemetry helpers are `sftp.exe`
/// and `ssh.exe`, so without this flag every new SSH session flashes a black
/// window on screen.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub fn hide_console(command: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}
