use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;

/// Secure-by-default gateway for home servers.
#[derive(Debug, Parser)]
#[command(name = "hearthgate", version = hearthgate::VERSION, about, long_about = None)]
struct Cli {
    /// Path to the configuration file.
    #[arg(
        short,
        long,
        env = "HEARTHGATE_CONFIG",
        default_value = "/etc/hearthgate/hearthgate.toml"
    )]
    config: PathBuf,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    println!("{}", hearthgate::banner());
    println!(
        "Config path is {}. Nothing is exposed yet: the gateway is not implemented in this build.",
        cli.config.display()
    );
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_is_well_formed() {
        Cli::command().debug_assert();
    }
}
