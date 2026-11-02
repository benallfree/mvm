//! Repo tooling, run as `cargo xtask`.

use std::process::ExitCode;

const USAGE: &str = "usage: cargo xtask <command>

No commands yet. `bench` arrives in 0.5 and `guards` in 4.12.";

fn main() -> ExitCode {
    match std::env::args().nth(1).as_deref() {
        None | Some("help" | "-h" | "--help") => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        Some(command) => {
            eprintln!("unknown command `{command}`\n\n{USAGE}");
            ExitCode::from(2)
        }
    }
}
