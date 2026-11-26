use std::process::ExitCode;

use blade::Cli;

fn main() -> ExitCode {
    let cli = Cli::new(std::env::args());
    let cmd = cli.command();
    cmd.execute()
}
