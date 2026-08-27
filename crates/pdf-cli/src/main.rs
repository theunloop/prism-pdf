//! The `prismpdf` binary (EPIC 15).
//!
//! Everything of substance lives in the `pdf_cli` library — see its module docs. This file is only
//! the shell: parse argv, run the command against stdout, map the outcome to an exit code.

use std::io::Write;
use std::process::ExitCode;

use clap::Parser;
use pdf_cli::Cli;

fn main() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        // `--help` and `--version` arrive here too: they print to stdout and succeed.
        Err(error) => {
            let _ = error.print();
            return if error.use_stderr() {
                ExitCode::FAILURE
            } else {
                ExitCode::SUCCESS
            };
        }
    };

    let stdout = std::io::stdout();
    let mut out = std::io::BufWriter::new(stdout.lock());
    let result = cli
        .run(&mut out)
        .and_then(|()| out.flush().map_err(|e| format!("cannot write output: {e}")));

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("prismpdf: {message}");
            ExitCode::FAILURE
        }
    }
}
