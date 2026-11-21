//! `maat plan`: files in, a contact plan out.
//!
//! Everything here is a thin use of `maat-ground`: read the catalog, read the stations and
//! requests, survey, schedule, and render. What goes to standard output is the plan and nothing
//! else, so it can be piped; everything a person should also know goes to the notes, which the
//! caller prints on standard error.

use std::path::Path;

use chrono::{DateTime, Duration, Utc};
use maat_ground::{
    ContactPlan, GroundError, PlanOptions, Satellite, contact_windows, plan_contacts_with,
    satellites_from_catalog, satnogs_schedule, station_schedule_json,
};

use crate::args::{Format, PlanArgs};
use maat_ground::scenario::Scenario;

/// What a command produced.
#[derive(Debug, PartialEq, Eq)]
pub struct Report {
    /// The plan, in the format asked for. Ends with a newline.
    pub output: String,
    /// Things worth telling the person that are not part of the plan.
    pub notes: Vec<String>,
}

/// Runs `maat plan`. `now` is used as the start of the search when none was given.
///
/// # Errors
///
/// A message that says which file or value is at fault and, where the library offers one, a
/// hint on how to fix it.
pub fn plan(args: &PlanArgs, now: DateTime<Utc>) -> Result<Report, String> {
    let mut notes = Vec::new();

    let catalog_text = read(&args.catalog, "catalog")?;
    let satellites = satellites_from_catalog(&catalog_text)
        .map_err(|error| format!("{}: {}", args.catalog.display(), explain(&error)))?;

    let mut scenario = Scenario::default();
    scenario.add_toml(
        &args.stations.display().to_string(),
        &read(&args.stations, "station")?,
    )?;
    if let Some(requests) = &args.requests {
        scenario.add_toml(&requests.display().to_string(), &read(requests, "request")?)?;
    }
    if scenario.stations.is_empty() {
        return Err(format!(
            "{} has no [[station]] table; a plan needs at least one ground station",
            args.stations.display()
        ));
    }
    if scenario.requests.is_empty() {
        return Err(
            "no [[request]] table was found, so no satellite would be scheduled; add one for \
             each satellite to the station file or to a file given with --requests, for \
             example:\n\n[[request]]\nsatellite = 25544\npriority = 5"
                .to_owned(),
        );
    }
    for request in &scenario.requests {
        if !satellites.iter().any(|s| s.id() == request.satellite()) {
            notes.push(format!(
                "note: a request names satellite {}, which is not in the catalog; it has no \
                 passes to schedule",
                request.satellite()
            ));
        }
    }

    // Whole seconds, so that the same command gives the same window however it is run.
    let start = args.start.unwrap_or(now).with_nanosecond_zero();
    let end = start + Duration::milliseconds((args.hours * 3_600_000.0).round() as i64);
    notes.push(format!(
        "search window: {} to {} UTC",
        start.format("%Y-%m-%d %H:%M:%S"),
        end.format("%Y-%m-%d %H:%M:%S")
    ));

    if let Some(note) = age_warning(&satellites, start, end) {
        notes.push(note);
    }

    let survey = contact_windows(&satellites, &scenario.stations, start, end)
        .map_err(|error| explain(&error))?;
    let mut options = PlanOptions::new(args.scheduler).with_satellite_floor(args.floor);
    if let Some(limit) = args.time_limit {
        options = options.with_optimal_time_limit(limit);
    }
    let plan = plan_contacts_with(
        &survey,
        &satellites,
        &scenario.stations,
        &scenario.requests,
        &options,
    )
    .map_err(|error| explain(&error))?;

    if !plan.is_complete() {
        notes.push(format!(
            "warning: the plan is INCOMPLETE: {} satellite/station pairs could not be searched",
            plan.failures.len()
        ));
    }
    if plan.optimality.is_some_and(|optimality| !optimality.proven) {
        notes.push(
            "warning: the search reached its time limit; the plan is the best found, not proven \
             optimal (raise --time-limit to search longer)"
                .to_owned(),
        );
    }

    let output = render(&plan, args, &scenario, &mut notes)?;
    Ok(Report { output, notes })
}

/// Renders the plan in the format asked for.
fn render(
    plan: &ContactPlan,
    args: &PlanArgs,
    scenario: &Scenario,
    notes: &mut Vec<String>,
) -> Result<String, String> {
    match args.format {
        Format::Text => Ok(plan.to_text()),
        Format::Json => Ok(format!("{}\n", plan.to_json())),
        Format::Satnogs => {
            let schedule = satnogs_schedule(plan, &scenario.satnogs);
            for skipped in &schedule.skipped {
                notes.push(format!("skipped: {}", skipped.reason));
            }
            if !schedule.skipped.is_empty() {
                notes.push(format!(
                    "warning: {} of {} scheduled passes are not in the SatNOGS output; give \
                     each station a satnogs_id and each request a satnogs_transmitter",
                    schedule.skipped.len(),
                    schedule.skipped.len() + schedule.observations.len()
                ));
            }
            Ok(schedule.to_json())
        }
        Format::Station => {
            let station = match (args.station, scenario.stations.as_slice()) {
                (None, [only]) => only,
                (None, _) => {
                    return Err(format!(
                        "--format station writes one station's schedule and the station file \
                         has {}; choose one with --station <ID> ({})",
                        scenario.stations.len(),
                        station_list(scenario)
                    ));
                }
                (Some(id), stations) => stations
                    .iter()
                    .find(|station| station.id().get() == id)
                    .ok_or_else(|| {
                        format!(
                            "--station {id} is not in the station file; it has {}",
                            station_list(scenario)
                        )
                    })?,
            };
            Ok(station_schedule_json(plan, station))
        }
    }
}

/// Beyond this many days from an element set's epoch, the plan carries a warning.
///
/// A rule of thumb, not a measured bound: the along-track error of a low-orbit two-line element
/// set typically grows by a kilometre or more a day, which moves AOS and LOS by a growing
/// fraction of a second. Two weeks is where operators customarily fetch new elements.
const ELEMENT_AGE_WARNING_DAYS: i64 = 14;

/// A warning if any satellite's elements are far from the search window, naming the worst.
///
/// An element set describes the orbit at its epoch, and predictions drift as the search moves
/// away from it in either direction. The age is measured to whichever end of the search is
/// further from the epoch.
fn age_warning(
    satellites: &[Satellite],
    start: DateTime<Utc>,
    end: DateTime<Utc>,
) -> Option<String> {
    let age_days = |satellite: &Satellite| {
        let epoch = satellite.epoch();
        (start - epoch).abs().max((end - epoch).abs()).num_days()
    };
    let old = satellites
        .iter()
        .filter(|satellite| age_days(satellite) > ELEMENT_AGE_WARNING_DAYS)
        .count();
    // `max_by_key` keeps the last of equals; reversed, that is the first in the catalog.
    let worst = satellites
        .iter()
        .rev()
        .max_by_key(|satellite| age_days(satellite))?;
    (old > 0).then(|| {
        format!(
            "warning: the elements of {old} of {} satellites are more than \
             {ELEMENT_AGE_WARNING_DAYS} days from the search window (furthest: {}, {} days); \
             pass times drift as elements age, so fetch a newer catalog if you can",
            satellites.len(),
            worst.name(),
            age_days(worst)
        )
    })
}

/// "1 Seattle, 2 Quito"
fn station_list(scenario: &Scenario) -> String {
    scenario
        .stations
        .iter()
        .map(|station| format!("{} {}", station.id(), station.name()))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Reads a text file, saying which kind of file it was if it cannot be read.
fn read(path: &Path, kind: &str) -> Result<String, String> {
    std::fs::read_to_string(path)
        .map_err(|error| format!("cannot read the {kind} file '{}': {error}", path.display()))
}

/// A library error with its hint, when it has one, on a line of its own.
fn explain(error: &GroundError) -> String {
    match error.hint() {
        Some(hint) => format!("{error}\nHint: {hint}"),
        None => error.to_string(),
    }
}

/// Drops the sub-second part of a time.
trait WholeSeconds {
    fn with_nanosecond_zero(self) -> Self;
}

impl WholeSeconds for DateTime<Utc> {
    fn with_nanosecond_zero(self) -> Self {
        DateTime::from_timestamp(self.timestamp(), 0).unwrap_or(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use maat_ground::satellites_from_omm;

    /// Two synthetic satellites with epochs a month apart.
    fn fleet() -> Vec<Satellite> {
        let omm = |name: &str, number: u32, epoch: &str| {
            format!(
                r#"{{"OBJECT_NAME":"{name}","NORAD_CAT_ID":{number},"EPOCH":"{epoch}",
                    "MEAN_MOTION":15.2,"ECCENTRICITY":0.0012,"INCLINATION":97.5,
                    "RA_OF_ASC_NODE":120.0,"ARG_OF_PERICENTER":45.0,"MEAN_ANOMALY":315.0,
                    "BSTAR":0.0001}}"#
            )
        };
        satellites_from_omm(&format!(
            "[{},{}]",
            omm("FRESH", 1, "2026-08-01T12:00:00"),
            omm("STALE", 2, "2026-07-01T12:00:00")
        ))
        .unwrap()
    }

    fn day(day: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 8, day, 12, 0, 0).unwrap()
    }

    #[test]
    fn fresh_elements_give_no_warning() {
        let fresh = &fleet()[..1];
        // A two-week search from the epoch is at the limit, not beyond it.
        assert_eq!(age_warning(fresh, day(1), day(15)), None);
        assert_eq!(age_warning(fresh, day(1), day(2)), None);
        assert_eq!(age_warning(&[], day(1), day(2)), None);
    }

    #[test]
    fn old_elements_are_counted_and_the_oldest_is_named() {
        // STALE is 31 days old at the start and 32 at the end of this one-day search.
        assert_eq!(
            age_warning(&fleet(), day(1), day(2)).unwrap(),
            "warning: the elements of 1 of 2 satellites are more than 14 days from the search \
             window (furthest: STALE, 32 days); pass times drift as elements age, so fetch a \
             newer catalog if you can"
        );
    }

    #[test]
    fn age_is_measured_to_the_far_end_of_the_search() {
        let fresh = &fleet()[..1];
        // Starting at the epoch but running for 16 days: the far end is 16 days out.
        assert!(
            age_warning(fresh, day(1), day(17))
                .unwrap()
                .contains("FRESH, 16 days")
        );
        // A search wholly before the epoch counts too: elements are no better backwards.
        let before = Utc.with_ymd_and_hms(2026, 7, 1, 12, 0, 0).unwrap();
        assert!(
            age_warning(fresh, before, before + Duration::days(1))
                .unwrap()
                .contains("FRESH, 31 days")
        );
    }

    #[test]
    fn whole_seconds_drops_only_the_fraction() {
        let time = day(1) + Duration::milliseconds(750);
        assert_eq!(time.with_nanosecond_zero(), day(1));
        assert_eq!(day(1).with_nanosecond_zero(), day(1));
    }
}
