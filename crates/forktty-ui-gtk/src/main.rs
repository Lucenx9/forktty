mod cli;
mod socket_cli;

#[cfg(feature = "gtk-vte")]
mod gtk_app;

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
        cli::CliAction::Doctor => ExitCode::from(cli::run_doctor() as u8),
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

#[cfg(feature = "gtk-vte")]
fn launch_app() -> ExitCode {
    gtk_app::run();
    ExitCode::SUCCESS
}

#[cfg(not(feature = "gtk-vte"))]
fn launch_app() -> ExitCode {
    println!(
        "forktty-ui-gtk built without GTK/VTE. Rebuild with `--features gtk-vte` after installing GTK4, libadwaita, and vte-2.91-gtk4 development packages."
    );
    ExitCode::SUCCESS
}
