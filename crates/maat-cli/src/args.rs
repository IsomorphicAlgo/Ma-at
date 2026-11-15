//! The arguments of `maat plan`.
//!
//! Hand-written, like the rest of the command line: there are a dozen options, and a parser of
//! this size is easier to test exhaustively than a dependency is to audit.

use std::path::PathBuf;
use std::time::Duration;

use chrono::{DateTime, Utc};
use maat_ground::Scheduler;

/// What `maat plan` writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// The plan for a person: every pass, scheduled or not, with its reason.
    Text,
    /// The same plan as JSON (`maat.contact-plan/1`).
    Json,
    /// The scheduled passes as SatNOGS Network observations.
    Satnogs,
    /// One station's scheduled passes (`maat.station-schedule/1`).
    Station,
}

/// Everything `maat plan` was asked to do.
#[derive(Debug, Clone, PartialEq)]
pub struct PlanArgs {
    /// The satellite catalog: OMM (JSON, KVN, XML or CSV) or two-line element sets.
    pub catalog: PathBuf,
    /// The TOML file of `[[station]]` tables. It may also hold `[[request]]` tables.
    pub stations: PathBuf,
    /// A second TOML file, usually of `[[request]]` tables.
    pub requests: Option<PathBuf>,
    /// When the search begins. `None` means now.
    pub start: Option<DateTime<Utc>>,
    /// How long the search runs, in hours.
    pub hours: f64,
    /// Which scheduler builds the plan.
    pub scheduler: Scheduler,
    /// How long the optimal scheduler may search. `None` means the library's default.
    pub time_limit: Option<Duration>,
    /// Whether the optimal scheduler keeps every satellite at its greedy share.
    pub floor: bool,
    /// What to write.
    pub format: Format,
    /// Which station `--format station` is for.
    pub station: Option<u16>,
    /// Where to write. `None` means standard output.
    pub output: Option<PathBuf>,
}

/// The longest search the command accepts: ten years, far beyond the life of an element set.
const LONGEST_SEARCH_HOURS: f64 = 87_660.0;

/// Reads the words that follow `plan`.
///
/// # Errors
///
/// A message that names the option at fault and says what it takes.
pub fn parse_plan(words: &[&str]) -> Result<PlanArgs, String> {
    let (mut catalog, mut stations, mut requests, mut output) = (None, None, None, None);
    let (mut start, mut hours, mut scheduler, mut time_limit) = (None, None, None, None);
    let (mut format, mut station, mut floor) = (None, None, false);

    let mut rest = words.iter().copied();
    while let Some(word) = rest.next() {
        // Accept both `--name value` and `--name=value`.
        let (name, inline) = match word.split_once('=') {
            Some((name, value)) if name.starts_with("--") => (name, Some(value)),
            _ => (word, None),
        };
        if name == "--floor" {
            if inline.is_some() {
                return Err("--floor takes no value".to_owned());
            }
            if floor {
                return Err("--floor was given more than once".to_owned());
            }
            floor = true;
            continue;
        }
        let known = [
            "--catalog",
            "--stations",
            "--requests",
            "--start",
            "--hours",
            "--scheduler",
            "--time-limit",
            "--format",
            "--station",
            "--output",
        ];
        if !known.contains(&name) {
            return Err(if name.starts_with('-') {
                format!("unrecognized option '{name}' for 'plan'")
            } else {
                format!("unexpected argument '{word}' for 'plan'; every value follows an option")
            });
        }
        let value = match inline.or_else(|| rest.next()) {
            Some(value) if !value.is_empty() => value,
            _ => return Err(format!("{name} needs a value")),
        };
        match name {
            "--catalog" => set(&mut catalog, PathBuf::from(value), name)?,
            "--stations" => set(&mut stations, PathBuf::from(value), name)?,
            "--requests" => set(&mut requests, PathBuf::from(value), name)?,
            "--output" => set(&mut output, PathBuf::from(value), name)?,
            "--start" => set(&mut start, parse_start(value)?, name)?,
            "--hours" => set(&mut hours, parse_hours(value)?, name)?,
            "--scheduler" => set(&mut scheduler, parse_scheduler(value)?, name)?,
            "--time-limit" => set(&mut time_limit, parse_time_limit(value)?, name)?,
            "--format" => set(&mut format, parse_format(value)?, name)?,
            _ => set(&mut station, parse_station(value)?, name)?,
        }
    }

    let scheduler = scheduler.unwrap_or(Scheduler::Greedy);
    if scheduler == Scheduler::Greedy {
        if time_limit.is_some() {
            return Err(
                "--time-limit applies to the optimal scheduler; add --scheduler optimal, or \
                 leave it out (the greedy scheduler always finishes quickly)"
                    .to_owned(),
            );
        }
        if floor {
            return Err(
                "--floor applies to the optimal scheduler; add --scheduler optimal, or leave it \
                 out (the greedy plan is the floor)"
                    .to_owned(),
            );
        }
    }
    let format = format.unwrap_or(Format::Text);
    if station.is_some() && format != Format::Station {
        return Err("--station is only used with --format station".to_owned());
    }
    Ok(PlanArgs {
        catalog: catalog.ok_or_else(|| missing("--catalog", "a satellite catalog file"))?,
        stations: stations.ok_or_else(|| missing("--stations", "a TOML station file"))?,
        requests,
        start,
        hours: hours.unwrap_or(24.0),
        scheduler,
        time_limit,
        floor,
        format,
        station,
        output,
    })
}

/// Stores an option's value, refusing a second one.
fn set<T>(slot: &mut Option<T>, value: T, name: &str) -> Result<(), String> {
    if slot.is_some() {
        return Err(format!("{name} was given more than once"));
    }
    *slot = Some(value);
    Ok(())
}

fn missing(name: &str, what: &str) -> String {
    format!("'plan' needs {name} <FILE>: {what}")
}

fn parse_start(value: &str) -> Result<DateTime<Utc>, String> {
    DateTime::parse_from_rfc3339(value)
        .map(|time| time.with_timezone(&Utc))
        .map_err(|_| {
            format!(
                "--start '{value}' is not a date and time; write it as 2026-08-01T12:00:00Z \
                 (the Z, or an offset such as +02:00, is required)"
            )
        })
}

fn parse_hours(value: &str) -> Result<f64, String> {
    match value.parse::<f64>() {
        Ok(hours) if hours > 0.0 && hours <= LONGEST_SEARCH_HOURS => Ok(hours),
        _ => Err(format!(
            "--hours '{value}' is not a search length; give a number of hours above 0, for \
             example 24 or 0.5"
        )),
    }
}

fn parse_scheduler(value: &str) -> Result<Scheduler, String> {
    match value {
        "greedy" => Ok(Scheduler::Greedy),
        "optimal" => Ok(Scheduler::Optimal),
        _ => Err(format!(
            "--scheduler '{value}' is not a scheduler; choose greedy or optimal"
        )),
    }
}

fn parse_time_limit(value: &str) -> Result<Duration, String> {
    match value.parse::<f64>() {
        Ok(seconds) if seconds >= 0.0 && seconds.is_finite() => {
            Duration::try_from_secs_f64(seconds).map_err(|_| too_long(value))
        }
        _ => Err(format!(
            "--time-limit '{value}' is not a time; give a number of seconds, 0 or more, for \
             example 10 or 0.5"
        )),
    }
}

fn too_long(value: &str) -> String {
    format!("--time-limit '{value}' is too long to represent; give a smaller number of seconds")
}

fn parse_format(value: &str) -> Result<Format, String> {
    match value {
        "text" => Ok(Format::Text),
        "json" => Ok(Format::Json),
        "satnogs" => Ok(Format::Satnogs),
        "station" => Ok(Format::Station),
        _ => Err(format!(
            "--format '{value}' is not a format; choose text, json, satnogs or station"
        )),
    }
}

fn parse_station(value: &str) -> Result<u16, String> {
    value.parse::<u16>().map_err(|_| {
        format!(
            "--station '{value}' is not a station number; give the id of a station in the \
             station file (0 to 65535)"
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    const MINIMAL: [&str; 4] = ["--catalog", "sats.json", "--stations", "stations.toml"];

    fn with(extra: &[&str]) -> Result<PlanArgs, String> {
        let mut words = MINIMAL.to_vec();
        words.extend_from_slice(extra);
        parse_plan(&words)
    }

    #[test]
    fn the_two_files_are_enough_and_everything_else_has_a_default() {
        let args = parse_plan(&MINIMAL).unwrap();
        assert_eq!(
            args,
            PlanArgs {
                catalog: PathBuf::from("sats.json"),
                stations: PathBuf::from("stations.toml"),
                requests: None,
                start: None,
                hours: 24.0,
                scheduler: Scheduler::Greedy,
                time_limit: None,
                floor: false,
                format: Format::Text,
                station: None,
                output: None,
            }
        );
    }

    #[test]
    fn every_option_is_read() {
        let args = with(&[
            "--requests",
            "requests.toml",
            "--start",
            "2026-08-01T12:00:00Z",
            "--hours",
            "6.5",
            "--scheduler",
            "optimal",
            "--time-limit",
            "2.5",
            "--floor",
            "--format",
            "station",
            "--station",
            "3",
            "--output",
            "plan.json",
        ])
        .unwrap();
        assert_eq!(args.requests, Some(PathBuf::from("requests.toml")));
        assert_eq!(
            args.start,
            Some(Utc.with_ymd_and_hms(2026, 8, 1, 12, 0, 0).unwrap())
        );
        assert_eq!(args.hours, 6.5);
        assert_eq!(args.scheduler, Scheduler::Optimal);
        assert_eq!(args.time_limit, Some(Duration::from_millis(2_500)));
        assert!(args.floor);
        assert_eq!((args.format, args.station), (Format::Station, Some(3)));
        assert_eq!(args.output, Some(PathBuf::from("plan.json")));
    }

    #[test]
    fn options_can_be_written_with_an_equals_sign_and_in_any_order() {
        let args = parse_plan(&[
            "--format=json",
            "--stations=stations.toml",
            "--hours=48",
            "--catalog=sats.json",
        ])
        .unwrap();
        assert_eq!((args.format, args.hours), (Format::Json, 48.0));
        assert_eq!(args.catalog, PathBuf::from("sats.json"));
        // A path may itself contain an equals sign after the first one.
        let args = parse_plan(&["--catalog=a=b.json", "--stations", "s.toml"]).unwrap();
        assert_eq!(args.catalog, PathBuf::from("a=b.json"));
    }

    #[test]
    fn a_start_time_with_an_offset_is_converted_to_utc() {
        let args = with(&["--start", "2026-08-01T14:00:00+02:00"]).unwrap();
        assert_eq!(
            args.start,
            Some(Utc.with_ymd_and_hms(2026, 8, 1, 12, 0, 0).unwrap())
        );
    }

    #[test]
    fn the_required_files_are_asked_for_by_name() {
        let error = parse_plan(&["--stations", "s.toml"]).unwrap_err();
        assert_eq!(
            error,
            "'plan' needs --catalog <FILE>: a satellite catalog file"
        );
        let error = parse_plan(&["--catalog", "c.json"]).unwrap_err();
        assert_eq!(error, "'plan' needs --stations <FILE>: a TOML station file");
        assert!(parse_plan(&[]).unwrap_err().contains("--catalog"));
    }

    #[test]
    fn an_option_without_its_value_says_so() {
        assert_eq!(with(&["--hours"]).unwrap_err(), "--hours needs a value");
        assert_eq!(with(&["--output="]).unwrap_err(), "--output needs a value");
    }

    #[test]
    fn an_option_given_twice_is_refused() {
        assert_eq!(
            with(&["--hours", "1", "--hours", "2"]).unwrap_err(),
            "--hours was given more than once"
        );
        assert_eq!(
            with(&["--catalog", "other.json"]).unwrap_err(),
            "--catalog was given more than once"
        );
        assert_eq!(
            with(&["--scheduler", "optimal", "--floor", "--floor"]).unwrap_err(),
            "--floor was given more than once"
        );
    }

    #[test]
    fn unknown_options_and_stray_words_are_named() {
        assert_eq!(
            with(&["--houres", "3"]).unwrap_err(),
            "unrecognized option '--houres' for 'plan'"
        );
        assert_eq!(
            with(&["extra"]).unwrap_err(),
            "unexpected argument 'extra' for 'plan'; every value follows an option"
        );
        assert_eq!(
            with(&["--floor=yes"]).unwrap_err(),
            "--floor takes no value"
        );
    }

    #[test]
    fn values_that_make_no_sense_are_refused_with_an_example() {
        for (words, expected) in [
            (["--start", "tomorrow"], "write it as 2026-08-01T12:00:00Z"),
            (["--start", "2026-08-01T12:00:00"], "the Z, or an offset"),
            (["--start", "2026-08-01"], "is not a date and time"),
            (["--hours", "0"], "above 0"),
            (["--hours", "-3"], "above 0"),
            (["--hours", "NaN"], "is not a search length"),
            (["--hours", "inf"], "is not a search length"),
            (["--hours", "a day"], "is not a search length"),
            (["--hours", "1e9"], "is not a search length"),
            (["--scheduler", "best"], "choose greedy or optimal"),
            (["--format", "xml"], "choose text, json, satnogs or station"),
        ] {
            let error = with(&words).unwrap_err();
            assert!(error.contains(expected), "{words:?}: {error}");
            assert!(
                error.contains(words[1]),
                "{words:?} should be quoted: {error}"
            );
        }
    }

    #[test]
    fn a_time_limit_must_be_a_number_of_seconds_from_zero_up() {
        let optimal =
            |limit: &'static str| with(&["--scheduler", "optimal", "--time-limit", limit]);
        assert_eq!(optimal("0").unwrap().time_limit, Some(Duration::ZERO));
        assert_eq!(
            optimal("90").unwrap().time_limit,
            Some(Duration::from_secs(90))
        );
        for bad in ["-1", "soon", "NaN", "inf"] {
            let error = optimal(bad).unwrap_err();
            assert!(error.contains("is not a time"), "{bad}: {error}");
        }
        assert!(optimal("1e300").unwrap_err().contains("too long"));
    }

    #[test]
    fn options_for_the_optimal_scheduler_are_refused_with_greedy() {
        let error = with(&["--time-limit", "5"]).unwrap_err();
        assert!(
            error.starts_with("--time-limit applies to the optimal scheduler"),
            "{error}"
        );
        let error = with(&["--scheduler", "greedy", "--floor"]).unwrap_err();
        assert!(
            error.starts_with("--floor applies to the optimal scheduler"),
            "{error}"
        );
        assert!(with(&["--scheduler", "optimal", "--floor"]).unwrap().floor);
    }

    #[test]
    fn a_station_number_goes_only_with_the_station_format() {
        assert_eq!(
            with(&["--station", "1"]).unwrap_err(),
            "--station is only used with --format station"
        );
        let error = with(&["--format", "station", "--station", "70000"]).unwrap_err();
        assert!(error.contains("is not a station number"), "{error}");
        // Without --station the format is still accepted: a single-station file needs none.
        assert_eq!(with(&["--format", "station"]).unwrap().station, None);
    }
}
