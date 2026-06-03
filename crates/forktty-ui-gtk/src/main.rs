mod cli;
mod socket_cli;

#[cfg(any(feature = "gtk-vte", feature = "gtk-ghostty"))]
mod gtk_app;

#[cfg(feature = "browser")]
mod browser_pane;

#[cfg(feature = "browser")]
mod browser_session;

use std::process::ExitCode;

fn main() -> ExitCode {
    match cli::parse(std::env::args_os()) {
        cli::CliAction::PrintVersion => {
            cli::print_version();
            ExitCode::SUCCESS
        }
        cli::CliAction::PrintHelp => {
            cli::print_help();
            ExitCode::SUCCESS
        }
        cli::CliAction::Doctor(options) => ExitCode::from(cli::run_doctor(options) as u8),
        cli::CliAction::SocketCli(args) => {
            ExitCode::from(socket_cli::run(args).clamp(0, 255) as u8)
        }
        cli::CliAction::Unknown(arg) => {
            eprintln!("forktty: unknown argument: {arg}");
            eprintln!("Run `forktty --help` for usage.");
            ExitCode::from(2)
        }
        cli::CliAction::LaunchApp => launch_app(),
    }
}

#[cfg(any(feature = "gtk-vte", feature = "gtk-ghostty"))]
fn launch_app() -> ExitCode {
    gtk_app::run();
    ExitCode::SUCCESS
}

#[cfg(not(any(feature = "gtk-vte", feature = "gtk-ghostty")))]
fn launch_app() -> ExitCode {
    println!(
        "forktty-ui-gtk built without GTK/Ghostty. Rebuild with `--features gtk-ghostty` after installing GTK4, libadwaita, git, and Zig for the vendored libghostty-vt build."
    );
    ExitCode::SUCCESS
}
