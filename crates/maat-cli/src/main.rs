//! `maat`: the command-line front end for the Ma'at contact scheduler.
//!
//! This is a thin layer over `maat-ground`. At this stage it has one real command, `codes`, which
//! prints the reason-code table straight from the flight crate, so the documentation a user sees
//! can never drift from the codes the scheduler emits.

use std::fmt::Write as _;
use std::process::ExitCode;

use maat_ground::flight::ReasonCode;

const USAGE: &str = "\
Usage: maat <COMMAND>

Commands:
  codes      Print the MAAT-xxx reason-code table

Options:
  -h, --help     Print help
  -V, --version  Print the version
";

/// What the command line asked for.
#[derive(Debug, PartialEq, Eq)]
enum Command {
    Codes,
    Help,
    Version,
}

/// Works out the command from the arguments that follow the program name.
///
/// Returns the message to print on standard error when the arguments make no sense.
fn parse(args: &[String]) -> Result<Command, String> {
    match args {
        [] => Ok(Command::Help),
        [only] => match only.as_str() {
            "codes" => Ok(Command::Codes),
            "-h" | "--help" | "help" => Ok(Command::Help),
            "-V" | "--version" => Ok(Command::Version),
            other => Err(format!("unrecognized command or option '{other}'")),
        },
        [_, extra, ..] => Err(format!("unexpected extra argument '{extra}'")),
    }
}

/// Renders the reason-code table, one row per code.
fn render_codes() -> String {
    let mut table = String::from("CODE      CLASS           MEANING\n");
    for reason in ReasonCode::ALL {
        // Writing into a String cannot fail.
        let _ = writeln!(
            table,
            "{:<8}  {:<14}  {}",
            reason.id(),
            reason.class().label(),
            reason.summary()
        );
    }
    table
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match parse(&args) {
        Ok(Command::Codes) => {
            print!("{}", render_codes());
            ExitCode::SUCCESS
        }
        Ok(Command::Help) => {
            print!("{USAGE}");
            ExitCode::SUCCESS
        }
        Ok(Command::Version) => {
            println!("maat {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("error: {message}\n\n{USAGE}");
            // 2 is the conventional exit status for a command-line usage error.
            ExitCode::from(2)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(words: &[&str]) -> Vec<String> {
        words.iter().map(|w| (*w).to_owned()).collect()
    }

    #[test]
    fn no_arguments_shows_help() {
        assert_eq!(parse(&args(&[])), Ok(Command::Help));
    }

    #[test]
    fn known_commands_and_flags_are_recognized() {
        assert_eq!(parse(&args(&["codes"])), Ok(Command::Codes));
        for help in ["-h", "--help", "help"] {
            assert_eq!(parse(&args(&[help])), Ok(Command::Help));
        }
        for version in ["-V", "--version"] {
            assert_eq!(parse(&args(&[version])), Ok(Command::Version));
        }
    }

    #[test]
    fn unknown_input_is_an_error_that_names_the_offender() {
        let unknown = parse(&args(&["schedule"])).unwrap_err();
        assert!(unknown.contains("'schedule'"), "{unknown}");

        let extra = parse(&args(&["codes", "--json"])).unwrap_err();
        assert!(extra.contains("'--json'"), "{extra}");
    }

    #[test]
    fn the_table_has_a_header_and_one_row_per_reason_code() {
        let table = render_codes();
        let mut lines = table.lines();
        assert_eq!(lines.next(), Some("CODE      CLASS           MEANING"));
        assert_eq!(lines.count(), ReasonCode::ALL.len());
    }

    #[test]
    fn every_row_shows_identifier_class_and_summary() {
        let table = render_codes();
        for reason in ReasonCode::ALL {
            let row = table
                .lines()
                .find(|line| line.starts_with(reason.id()))
                .unwrap_or_else(|| panic!("no row for {}", reason.id()));
            assert!(row.contains(reason.class().label()), "{row}");
            assert!(row.ends_with(reason.summary()), "{row}");
        }
    }
}
