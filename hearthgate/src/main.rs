use std::process::ExitCode;

use clap::Parser;
use hearthgate::cli::Cli;

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.run() {
        Ok(code) => code,
        Err(err) => {
            eprintln!("error: {err:#}");
            ExitCode::FAILURE
        }
    }
}
