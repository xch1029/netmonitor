// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    if let Some(exit_code) = netmonitor_lib::helper::run_if_requested(std::env::args_os()) {
        std::process::exit(exit_code);
    }

    netmonitor_lib::run()
}
