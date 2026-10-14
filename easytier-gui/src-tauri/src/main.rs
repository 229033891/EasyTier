#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

/// Flags that still launch the GUI (tray) process instead of the core CLI.
fn is_gui_only_arg(arg: &str) -> bool {
    matches!(arg, "--startup" | "--hidden")
}

fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if !args.is_empty() && !args.iter().all(|a| is_gui_only_arg(a)) {
        app_lib::run_cli()
    }
    else {
        app_lib::run_gui()
    }
}
