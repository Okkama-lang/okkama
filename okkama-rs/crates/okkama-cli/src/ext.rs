//! Editable declarations from a validated native extension descriptor.
use super::{Outcome, EXIT_IO};

pub const HELP: &str = "okkama ext — native extension declarations

USAGE:
    okkama ext declare LIB

Loads a trusted native library and prints an Okkama module skeleton to stdout.
Save it as a .ok file, then add documentation and hidden declarations as needed.
Library loading runs its OS constructors. No native callback is invoked.
";

pub fn command(args: &[String]) -> Outcome {
    if args.iter().any(|a| a == "-h" || a == "--help") { return Outcome::success(HELP.into()); }
    match args {
        [command, path] if command == "declare" => {
            match sparrow_core::ext::load(path).and_then(|extension| extension.declarations()) {
                Ok(text) => Outcome::success(text),
                Err(error) => Outcome::failure(format!("okkama: {}\n", error.0), EXIT_IO),
            }
        }
        _ => Outcome::usage("expected `okkama ext declare LIB`"),
    }
}
