// Do not allocate a console window on Windows. Without this the GUI binary is
// linked as a console subsystem app and Windows opens a terminal behind afterm.
#![cfg_attr(windows, windows_subsystem = "windows")]

fn main() {
    afterm_lib::run();
}
