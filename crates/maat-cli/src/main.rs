//! `maat`: the command-line front end for the Ma'at contact scheduler.
//!
//! This is a thin layer over `maat-ground`. It has two real commands so far, both about reason
//! codes: `codes` prints the table and `explain` prints the guidance for one code. Both read
//! straight from the library, so what a user sees cannot drift from what the scheduler emits.

use std::fmt::Write as _;
use std::process::ExitCode;

use maat_ground::flight::ReasonCode;
use maat_ground::{guidance, parse_reason_code};

const USAGE: &str = "\
Usage: maat <COMMAND>

Commands:
  codes           Print the MAAT-xxx reason-code table
  explain <CODE>  Explain one reason code and what would change the outcome

Options:
  -h, --help     Print help
  -V, --version  Print the version
";

/// The widest line `explain` prints.
const WRAP_COLUMNS: usize = 78;

/// What the command line asked for.
#[derive(Debug, PartialEq, Eq)]
enum Command {
    Codes,
    Explain(ReasonCode),
    Help,
    Version,
}

/// Works out the command from the arguments that follow the program name.
///
/// Returns the message to print on standard error when the arguments make no sense.
fn parse(args: &[String]) -> Result<Command, String> {
    let words: Vec<&str> = args.iter().map(String::as_str).collect();
    match words.as_slice() {
        [] | ["-h" | "--help" | "help"] => Ok(Command::Help),
        ["-V" | "--version"] => Ok(Command::Version),
        ["codes"] => Ok(Command::Codes),
        ["explain"] => {
            Err("'explain' needs a reason code, for example: maat explain MAAT-101".to_owned())
        }
        ["explain", code] => parse_reason_code(code)
            .map(Command::Explain)
            .ok_or_else(|| {
                format!("'{code}' is not a MAAT reason code; run 'maat codes' for the list")
            }),
        ["codes" | "explain", .., extra] => Err(format!("unexpected extra argument '{extra}'")),
        [other, ..] => Err(format!("unrecognized command or option '{other}'")),
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

/// Renders the explanation of one reason code: identifier and class, summary, then guidance.
fn render_explanation(reason: ReasonCode) -> String {
    format!(
        "{}  ({})\n{}\n\n{}\n",
        reason.id(),
        reason.class().label(),
        reason.summary(),
        wrap(guidance(reason), WRAP_COLUMNS)
    )
}

/// Breaks text into lines of at most `columns` characters at spaces. A word longer than a line
/// gets a line to itself.
fn wrap(text: &str, columns: usize) -> String {
    let mut out = String::new();
    let mut line_length = 0;
    for word in text.split_whitespace() {
        if line_length > 0 && line_length + 1 + word.len() > columns {
            out.push('\n');
            line_length = 0;
        }
        if line_length > 0 {
            out.push(' ');
            line_length += 1;
        }
        out.push_str(word);
        line_length += word.len();
    }
    out
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match parse(&args) {
        Ok(Command::Codes) => {
            print!("{}", render_codes());
            ExitCode::SUCCESS
        }
        Ok(Command::Explain(reason)) => {
            print!("{}", render_explanation(reason));
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
    fn explain_takes_a_code_however_it_is_typed() {
        for code in ["MAAT-101", "maat-101", "101"] {
            assert_eq!(
                parse(&args(&["explain", code])),
                Ok(Command::Explain(ReasonCode::StationBusy))
            );
        }
    }

    #[test]
    fn explain_without_a_code_says_what_is_missing() {
        let message = parse(&args(&["explain"])).unwrap_err();
        assert!(message.contains("needs a reason code"), "{message}");
        assert!(message.contains("maat explain MAAT-101"), "{message}");
    }

    #[test]
    fn explain_with_an_unknown_code_points_at_the_list() {
        let message = parse(&args(&["explain", "MAAT-999"])).unwrap_err();
        assert!(
            message.contains("'MAAT-999' is not a MAAT reason code"),
            "{message}"
        );
        assert!(message.contains("maat codes"), "{message}");
    }

    #[test]
    fn unknown_input_is_an_error_that_names_the_offender() {
        let unknown = parse(&args(&["schedule"])).unwrap_err();
        assert!(
            unknown.contains("unrecognized") && unknown.contains("'schedule'"),
            "{unknown}"
        );

        let extra = parse(&args(&["codes", "--json"])).unwrap_err();
        assert!(
            extra.contains("unexpected extra") && extra.contains("'--json'"),
            "{extra}"
        );

        let extra = parse(&args(&["explain", "101", "102"])).unwrap_err();
        assert!(extra.contains("'102'"), "{extra}");
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

    #[test]
    fn an_explanation_leads_with_the_code_then_summary_then_guidance() {
        let text = render_explanation(ReasonCode::StationTurnaround);
        let mut lines = text.lines();
        assert_eq!(lines.next(), Some("MAAT-108  (rejected)"));
        assert_eq!(lines.next(), Some(ReasonCode::StationTurnaround.summary()));
        assert_eq!(lines.next(), Some(""));
        assert!(
            lines
                .next()
                .is_some_and(|line| line.starts_with("The pass does not overlap"))
        );
    }

    #[test]
    fn every_explanation_fits_the_terminal() {
        for reason in ReasonCode::ALL {
            for line in render_explanation(*reason).lines() {
                // The summary line is allowed its 80 columns; everything else wraps at 78.
                assert!(line.len() <= 80, "{}: {line:?}", reason.id());
            }
        }
    }

    #[test]
    fn wrap_breaks_at_spaces_and_loses_no_words() {
        let text = "one two three four five six seven eight nine ten";
        let wrapped = wrap(text, 14);
        assert_eq!(
            wrapped,
            "one two three\nfour five six\nseven eight\nnine ten"
        );
        assert!(wrapped.lines().all(|line| line.len() <= 14));
        assert_eq!(wrapped.replace('\n', " "), text);
    }

    #[test]
    fn wrap_gives_an_overlong_word_its_own_line() {
        assert_eq!(
            wrap("a incomprehensibilities b", 10),
            "a\nincomprehensibilities\nb"
        );
        assert_eq!(wrap("", 10), "");
        assert_eq!(wrap("   spaced    out   ", 80), "spaced out");
    }
}
