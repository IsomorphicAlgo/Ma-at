//! Runs the real `maat` binary and checks what a user would see: output, error stream and exit
//! status. Cargo builds the binary before these tests and passes its path in `CARGO_BIN_EXE_maat`.

use std::process::{Command, Output};

fn maat(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_maat"))
        .args(args)
        .output()
        .expect("the maat binary runs")
}

fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).expect("stdout is UTF-8")
}

fn stderr(output: &Output) -> String {
    String::from_utf8(output.stderr.clone()).expect("stderr is UTF-8")
}

#[test]
fn codes_prints_the_reason_table_and_succeeds() {
    let output = maat(&["codes"]);
    assert!(output.status.success());
    let text = stdout(&output);
    assert!(text.contains(
        "MAAT-101  rejected        dropped: overlaps a higher-priority pass at the same station"
    ));
    assert!(text.contains("MAAT-001  accepted"));
    assert!(text.contains("MAAT-201  invalid input"));
    assert!(text.contains("MAAT-301  resource limit"));
    assert!(text.contains("MAAT-109  rejected"));
    assert!(stderr(&output).is_empty());
}

#[test]
fn explain_prints_the_code_its_summary_and_guidance() {
    let output = maat(&["explain", "maat-105"]);
    assert!(output.status.success());
    let text = stdout(&output);
    assert!(text.starts_with("MAAT-105  (rejected)\n"), "{text}");
    assert!(
        text.contains("faster antenna slew than the station allows"),
        "{text}"
    );
    assert!(text.contains("slew limit"), "{text}");
    assert!(stderr(&output).is_empty());
}

#[test]
fn explain_with_an_unknown_code_fails_with_status_2() {
    let output = maat(&["explain", "MAAT-999"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(stdout(&output).is_empty());
    assert!(stderr(&output).contains("'MAAT-999' is not a MAAT reason code"));
}

#[test]
fn version_matches_the_package_version() {
    let output = maat(&["--version"]);
    assert!(output.status.success());
    assert_eq!(
        stdout(&output).trim(),
        format!("maat {}", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn help_and_no_arguments_print_usage_and_succeed() {
    for args in [&["--help"][..], &[][..]] {
        let output = maat(args);
        assert!(output.status.success());
        assert!(stdout(&output).starts_with("Usage: maat <COMMAND>"));
    }
}

#[test]
fn an_unknown_command_fails_with_status_2_and_explains_on_stderr() {
    let output = maat(&["launch"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(stdout(&output).is_empty(), "errors must not go to stdout");
    let text = stderr(&output);
    assert!(
        text.contains("unrecognized command or option 'launch'"),
        "{text}"
    );
    assert!(text.contains("Usage: maat <COMMAND>"), "{text}");
}
