mod agents;
mod bar;
mod cli;
mod clipboard;
mod config;
mod daemon;
mod doctor;
mod hooks;
mod model;
mod notify;
mod paths;
mod process;
mod pty;
mod session;
mod telemetry;
mod upgrade;
mod version;

use clap::Parser;
use cli::Cli;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() >= 2 && args[1] == "notify" && notify::try_fast_notify(&args[2..]).is_ok() {
        return;
    }

    let cli = Cli::parse();
    if let Err(e) = cli::dispatch(cli.command) {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}