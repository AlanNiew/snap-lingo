// Prevents an extra console window from flashing up on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    snap_lingo_lib::run()
}
