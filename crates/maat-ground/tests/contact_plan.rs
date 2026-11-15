//! End-to-end tests of Stages 2 and 3: OMM text in, an explained contact plan out.
//!
//! Deterministic and offline. The satellites are synthetic element sets with a fixed epoch
//! (2026-08-01T12:00:00 UTC). Each test runs the whole chain: load the catalog, survey a week of
//! windows over four stations, schedule them, and check the plan against the rules by means that
//! do not share code with the scheduler.

use chrono::{DateTime, Duration, TimeZone, Utc};
use ephemerust::Omm;
use maat_ground::flight::schedule::{conflict, schedule_greedy};
use maat_ground::flight::{
    ContactWindow, Decision, Priority, ReasonClass, ReasonCode, SatelliteId, SatelliteRequest,
    Seconds, StationId, StationRule,
};
use maat_ground::{
    ContactPlan, DecidedBy, GroundStation, JSON_SCHEMA, Satellite, Scheduler, SlewLimits,
    StationSite, WindowSurvey, contact_windows, plan_contacts, satellites_from_omm,
};

const CATALOG: &str = r#"[
  {"OBJECT_NAME":"EXAMPLE-A","NORAD_CAT_ID":65001,"EPOCH":"2026-08-01T12:00:00",
   "MEAN_MOTION":15.49,"ECCENTRICITY":0.0002,"INCLINATION":51.64,"RA_OF_ASC_NODE":221.0,
   "ARG_OF_PERICENTER":89.0,"MEAN_ANOMALY":280.0,"BSTAR":0.00003},
  {"OBJECT_NAME":"EXAMPLE-B","NORAD_CAT_ID":100123,"EPOCH":"2026-08-01T12:00:00",
   "MEAN_MOTION":15.2,"ECCENTRICITY":0.0012,"INCLINATION":97.5,"RA_OF_ASC_NODE":120.0,
   "ARG_OF_PERICENTER":45.0,"MEAN_ANOMALY":315.0,"BSTAR":0.0001},
  {"OBJECT_NAME":"EXAMPLE-C","NORAD_CAT_ID":100124,"EPOCH":"2026-08-01T12:00:00",
   "MEAN_MOTION":15.2,"ECCENTRICITY":0.0012,"INCLINATION":97.5,"RA_OF_ASC_NODE":120.0,
   "ARG_OF_PERICENTER":45.0,"MEAN_ANOMALY":312.0,"BSTAR":0.0001},
  {"OBJECT_NAME":"EXAMPLE-D","NORAD_CAT_ID":100125,"EPOCH":"2026-08-01T12:00:00",
   "MEAN_MOTION":15.2,"ECCENTRICITY":0.0012,"INCLINATION":97.5,"RA_OF_ASC_NODE":120.0,
   "ARG_OF_PERICENTER":45.0,"MEAN_ANOMALY":290.0,"BSTAR":0.0001},
  {"OBJECT_NAME":"EXAMPLE-E","NORAD_CAT_ID":100126,"EPOCH":"2026-08-01T12:00:00",
   "MEAN_MOTION":14.9,"ECCENTRICITY":0.001,"INCLINATION":64.0,"RA_OF_ASC_NODE":300.0,
   "ARG_OF_PERICENTER":10.0,"MEAN_ANOMALY":40.0,"BSTAR":0.00005}
]"#;

const A: SatelliteId = SatelliteId::new(65_001);
const B: SatelliteId = SatelliteId::new(100_123);
const C: SatelliteId = SatelliteId::new(100_124);
const D: SatelliteId = SatelliteId::new(100_125);
const E: SatelliteId = SatelliteId::new(100_126);

const SEATTLE: StationId = StationId::new(1);

fn epoch() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 8, 1, 12, 0, 0).unwrap()
}

fn satellites() -> Vec<Satellite> {
    satellites_from_omm(CATALOG).expect("the catalog is valid")
}

fn station(id: u16, name: &str, latitude_deg: f64, longitude_deg: f64) -> GroundStation {
    GroundStation::new(
        StationId::new(id),
        name,
        StationSite {
            latitude_deg,
            longitude_deg,
            altitude_m: 0.0,
        },
        10.0,
    )
    .expect("the station is valid")
}

/// Four stations. Seattle has a slow mount and needs two minutes between passes; Svalbard needs
/// one minute.
///
/// EXAMPLE-B, EXAMPLE-C and EXAMPLE-D fly the same orbit. C trails B by 3 degrees (under a
/// minute), so their passes always overlap. D trails by 25 degrees (about six and a half
/// minutes), so its pass begins around the time theirs ends: sometimes overlapping, sometimes
/// clear, and sometimes inside a station's turnaround time.
fn stations() -> Vec<GroundStation> {
    vec![
        station(1, "Seattle", 47.61, -122.33)
            .with_slew_limits(SlewLimits {
                azimuth_deg_per_s: 2.0,
                elevation_deg_per_s: 2.0,
            })
            .expect("the limits are valid")
            .with_turnaround(Seconds::new(120)),
        station(2, "Quito", -0.18, -78.47),
        station(3, "Svalbard", 78.23, 15.41).with_turnaround(Seconds::new(60)),
        station(4, "Cape Town", -33.92, 18.42),
    ]
}

/// EXAMPLE-A is urgent and needs four hours. EXAMPLE-B and EXAMPLE-C share a priority and want
/// passes of at least three minutes. EXAMPLE-D ranks below them. EXAMPLE-E has no request.
fn requests() -> Vec<SatelliteRequest> {
    vec![
        SatelliteRequest::new(A, Priority::new(9)).with_need(Seconds::new(NEED_A)),
        SatelliteRequest::new(B, Priority::new(4)).with_min_contact(Seconds::new(180)),
        SatelliteRequest::new(C, Priority::new(4)).with_min_contact(Seconds::new(180)),
        SatelliteRequest::new(D, Priority::new(2)),
    ]
}

/// The contact time EXAMPLE-A asks for, in seconds.
const NEED_A: u32 = 14_400;

fn survey() -> WindowSurvey {
    contact_windows(
        &satellites(),
        &stations(),
        epoch(),
        epoch() + Duration::days(7),
    )
    .expect("the request is valid")
}

fn plan(scheduler: Scheduler) -> ContactPlan {
    plan_contacts(
        &survey(),
        &satellites(),
        &stations(),
        &requests(),
        scheduler,
    )
    .expect("planning succeeds")
}

fn rules() -> Vec<StationRule> {
    stations().iter().map(GroundStation::rule).collect()
}

fn turnaround_of(station: StationId) -> Seconds {
    rules()
        .iter()
        .find(|rule| rule.station() == station)
        .map_or(Seconds::ZERO, StationRule::turnaround)
}

fn scheduled_windows(plan: &ContactPlan) -> Vec<ContactWindow> {
    plan.scheduled()
        .map(|entry| entry.window.expect("a scheduled entry has a window"))
        .collect()
}

// ---------------------------------------------------------------------------------------------
// The fixture is rich enough to mean something
// ---------------------------------------------------------------------------------------------

#[test]
fn the_week_produces_every_kind_of_outcome() {
    // If the fixture stopped producing one of these, the tests below would pass while proving
    // less. Counts are of the greedy plan.
    let plan = plan(Scheduler::Greedy);
    let count = |reason: ReasonCode| plan.entries.iter().filter(|e| e.reason == reason).count();
    assert!(
        plan.entries.len() > 400,
        "a week over four stations: {}",
        plan.entries.len()
    );
    for reason in [
        ReasonCode::Scheduled,
        ReasonCode::StationBusy,
        ReasonCode::StationTieLost,
        ReasonCode::StationTurnaround,
        ReasonCode::BelowMinimumDuration,
        ReasonCode::NeedAlreadyMet,
        ReasonCode::SlewRateExceeded,
        ReasonCode::NoRequest,
    ] {
        assert!(
            count(reason) >= 10,
            "{} came up {} times",
            reason.id(),
            count(reason)
        );
    }
}

// ---------------------------------------------------------------------------------------------
// Every pass is accounted for and explained
// ---------------------------------------------------------------------------------------------

#[test]
fn every_pass_the_survey_found_is_in_the_plan_exactly_once() {
    let survey = survey();
    for scheduler in [Scheduler::Greedy, Scheduler::Optimal] {
        let plan = plan(scheduler);
        assert_eq!(
            plan.entries.len(),
            survey.windows.len() + survey.rejected.len()
        );
        for window in &survey.windows {
            let matches = plan
                .entries
                .iter()
                .filter(|e| e.window.as_ref() == Some(window));
            assert_eq!(matches.count(), 1, "{window:?}");
        }
        let from_survey = plan
            .entries
            .iter()
            .filter(|e| e.decided_by == DecidedBy::Survey);
        assert_eq!(from_survey.count(), survey.rejected.len());
    }
}

#[test]
fn every_entry_has_a_code_of_the_right_class_and_a_detail_with_substance() {
    for scheduler in [Scheduler::Greedy, Scheduler::Optimal] {
        for entry in &plan(scheduler).entries {
            let expected = if entry.is_scheduled() {
                ReasonClass::Accepted
            } else {
                ReasonClass::Rejected
            };
            assert_eq!(entry.reason.class(), expected, "{:?}", entry.reason);
            assert!(entry.detail.len() > 20, "thin detail: {:?}", entry.detail);
            assert_ne!(entry.detail, entry.reason.summary());
            assert!(entry.explanation().starts_with(entry.reason.id()));
        }
    }
}

#[test]
fn details_use_real_names_not_numbers() {
    let plan = plan(Scheduler::Greedy);
    let lost: Vec<_> = plan
        .entries
        .iter()
        .filter(|e| e.reason == ReasonCode::StationBusy)
        .collect();
    assert!(lost.len() >= 10);
    for entry in &lost {
        let winner = &plan.entries[entry.blocked_by.unwrap()];
        let expected = format!(
            "{} (priority {}) has {} from ",
            winner.satellite_name,
            winner.priority.unwrap(),
            entry.station_name
        );
        assert!(entry.detail.starts_with(&expected), "{}", entry.detail);
        assert!(winner.satellite_name.starts_with("EXAMPLE-"));
        assert!(!entry.detail.contains("satellite 1"), "{}", entry.detail);
    }
    // The urgent satellite is among the winners.
    assert!(
        lost.iter()
            .any(|e| e.detail.starts_with("EXAMPLE-A (priority 9) has "))
    );
}

#[test]
fn survey_rejections_keep_their_reason_and_numbers() {
    let plan = plan(Scheduler::Optimal);
    let slew: Vec<_> = plan
        .entries
        .iter()
        .filter(|e| e.reason == ReasonCode::SlewRateExceeded)
        .collect();
    assert!(!slew.is_empty());
    for entry in slew {
        assert_eq!(entry.decided_by, DecidedBy::Survey);
        assert_eq!(entry.station, SEATTLE, "only Seattle has slew limits");
        assert!(
            entry.detail.starts_with("tracking needs "),
            "{}",
            entry.detail
        );
        assert!(
            entry.detail.contains("the station allows 2.00"),
            "{}",
            entry.detail
        );
        assert!(entry.window.is_none() && entry.blocked_by.is_none());
    }
}

// ---------------------------------------------------------------------------------------------
// The plan obeys the rules
// ---------------------------------------------------------------------------------------------

#[test]
fn no_two_scheduled_passes_conflict() {
    for scheduler in [Scheduler::Greedy, Scheduler::Optimal] {
        let windows = scheduled_windows(&plan(scheduler));
        assert!(windows.len() > 100);
        for (i, a) in windows.iter().enumerate() {
            for b in &windows[i + 1..] {
                let clash = conflict(a, b, turnaround_of(a.station()));
                assert_eq!(clash, None, "{} plan: {a:?} and {b:?}", scheduler.name());
            }
        }
    }
}

#[test]
fn stations_with_a_turnaround_time_get_it_between_passes() {
    for scheduler in [Scheduler::Greedy, Scheduler::Optimal] {
        let plan = plan(scheduler);
        for (station, seconds) in [(1_u16, 120_i64), (3, 60)] {
            let mut passes: Vec<_> = plan
                .scheduled()
                .filter(|e| e.station.get() == station)
                .collect();
            passes.sort_by_key(|e| e.aos);
            assert!(passes.len() > 5);
            for pair in passes.windows(2) {
                let gap = (pair[1].aos - pair[0].los).num_seconds();
                assert!(
                    gap >= seconds,
                    "station {station}: only {gap} s between passes"
                );
            }
        }
    }
}

#[test]
fn scheduled_passes_respect_each_request() {
    for scheduler in [Scheduler::Greedy, Scheduler::Optimal] {
        let plan = plan(scheduler);
        for entry in plan.scheduled() {
            assert_ne!(entry.satellite, E, "EXAMPLE-E has no request");
            if entry.satellite == B || entry.satellite == C {
                assert!(
                    entry.duration_seconds() >= 180.0,
                    "{} s",
                    entry.duration_seconds()
                );
            }
        }
        // Every EXAMPLE-E pass that reached the scheduler says nobody asked for it.
        for entry in plan.entries.iter().filter(|e| e.satellite == E) {
            if entry.decided_by == DecidedBy::Scheduler {
                assert_eq!(entry.reason, ReasonCode::NoRequest);
            }
        }
    }
}

#[test]
fn the_urgent_satellite_gets_what_it_asked_for_and_no_surplus_pass() {
    for scheduler in [Scheduler::Greedy, Scheduler::Optimal] {
        let plan = plan(scheduler);
        let a = plan.satellites.iter().find(|s| s.satellite == A).unwrap();
        assert_eq!(a.need_met(), Some(true), "{} plan", scheduler.name());
        let granted = a.contact_scheduled.get();
        assert!(granted >= NEED_A);
        // Passes are used whole, so the total can exceed the need, but by less than one pass:
        // no scheduled pass could be removed and still leave the need met.
        let shortest = plan
            .scheduled()
            .filter(|e| e.satellite == A)
            .map(|e| e.window.unwrap().duration().get())
            .min()
            .unwrap();
        assert!(
            granted - shortest < NEED_A,
            "a {shortest} s pass of {granted} s is surplus"
        );
    }
}

#[test]
fn blocked_by_always_points_at_a_scheduled_pass_that_conflicts() {
    for scheduler in [Scheduler::Greedy, Scheduler::Optimal] {
        let plan = plan(scheduler);
        let mut seen = 0;
        for entry in &plan.entries {
            let Some(blocker) = entry.blocked_by else {
                continue;
            };
            let other = &plan.entries[blocker];
            assert!(other.is_scheduled());
            let clash = conflict(
                &entry.window.unwrap(),
                &other.window.unwrap(),
                turnaround_of(entry.station),
            );
            assert!(
                clash.is_some(),
                "{:?} is not blocked by {:?}",
                entry.window,
                other.window
            );
            seen += 1;
        }
        assert!(
            seen > 20,
            "only {seen} blocked passes in the {} plan",
            scheduler.name()
        );
    }
}

// ---------------------------------------------------------------------------------------------
// Greedy on the ground is the flight scheduler
// ---------------------------------------------------------------------------------------------

#[test]
fn the_greedy_plan_is_exactly_what_the_flight_scheduler_decides() {
    // The claim is that one scheduling core runs on the ground and onboard. Run the flight
    // function directly on the surveyed windows, with a fixed-size array as it would have
    // onboard, and compare decision by decision.
    let survey = survey();
    let mut decisions = [Decision::PENDING; 1_024];
    assert!(survey.windows.len() <= decisions.len());
    let summary = schedule_greedy(&survey.windows, &requests(), &rules(), &mut decisions)
        .expect("the flight scheduler accepts the request");

    let plan = plan(Scheduler::Greedy);
    for (window, decision) in survey.windows.iter().zip(&decisions) {
        let entry = plan
            .entries
            .iter()
            .find(|e| e.window.as_ref() == Some(window))
            .unwrap();
        assert_eq!(Some(entry.reason), decision.reason(), "{window:?}");
    }
    assert_eq!(summary.scheduled, plan.scheduled().count());
    assert_eq!(summary.contact, plan.scheduled_contact());
}

// ---------------------------------------------------------------------------------------------
// Greedy against optimal
// ---------------------------------------------------------------------------------------------

#[test]
fn the_optimal_plan_is_worth_more_than_the_greedy_plan_on_this_week() {
    let (greedy, optimal) = (plan(Scheduler::Greedy), plan(Scheduler::Optimal));
    assert!(
        optimal.value > greedy.value,
        "optimal {} against greedy {}",
        optimal.value,
        greedy.value
    );
    // A sanity bound on the size of the gain: the optimizer rearranges contested passes; it
    // cannot double the plan. Measured: 2.3 % (425 042 against 415 347).
    assert!(optimal.value < greedy.value * 1.25);
}

#[test]
fn with_equal_priorities_value_is_useful_contact_time() {
    // All priorities equal and no limits: the value is (priority + 1) x total contact seconds,
    // so the optimal plan is the one with the most contact time.
    let equal: Vec<SatelliteRequest> = [A, B, C, D, E]
        .into_iter()
        .map(|satellite| SatelliteRequest::new(satellite, Priority::new(0)))
        .collect();
    let build = |scheduler| {
        plan_contacts(&survey(), &satellites(), &stations(), &equal, scheduler).unwrap()
    };
    let (greedy, optimal) = (build(Scheduler::Greedy), build(Scheduler::Optimal));
    assert_eq!(greedy.value, f64::from(greedy.scheduled_contact().get()));
    assert_eq!(optimal.value, f64::from(optimal.scheduled_contact().get()));
    assert!(optimal.scheduled_contact() >= greedy.scheduled_contact());
}

#[test]
fn both_schedulers_screen_the_same_passes() {
    let (greedy, optimal) = (plan(Scheduler::Greedy), plan(Scheduler::Optimal));
    for (g, o) in greedy.entries.iter().zip(&optimal.entries) {
        assert_eq!(
            (g.aos, g.station, g.satellite),
            (o.aos, o.station, o.satellite)
        );
        for reason in [
            ReasonCode::NoRequest,
            ReasonCode::BelowMinimumDuration,
            ReasonCode::SlewRateExceeded,
        ] {
            assert_eq!(
                g.reason == reason,
                o.reason == reason,
                "{:?} / {:?}",
                g.reason,
                o.reason
            );
        }
    }
}

#[test]
fn planning_is_deterministic() {
    for scheduler in [Scheduler::Greedy, Scheduler::Optimal] {
        assert_eq!(plan(scheduler), plan(scheduler));
    }
}

// ---------------------------------------------------------------------------------------------
// Output
// ---------------------------------------------------------------------------------------------

#[test]
fn json_output_agrees_with_the_plan() {
    for scheduler in [Scheduler::Greedy, Scheduler::Optimal] {
        let plan = plan(scheduler);
        let json: serde_json::Value =
            serde_json::from_str(&plan.to_json()).expect("the output is valid JSON");
        assert_eq!(json["schema"], JSON_SCHEMA);
        assert_eq!(json["scheduler"], scheduler.name());
        assert_eq!(json["complete"], true);
        assert_eq!(json["totals"]["passes"], plan.entries.len());
        assert_eq!(json["totals"]["scheduled"], plan.scheduled().count());
        assert_eq!(
            json["totals"]["contact_scheduled_s"],
            plan.scheduled_contact().get()
        );

        let entries = json["entries"].as_array().unwrap();
        assert_eq!(entries.len(), plan.entries.len());
        for (entry, value) in plan.entries.iter().zip(entries) {
            assert_eq!(value["reason"]["code"], entry.reason.id());
            assert_eq!(value["reason"]["number"], entry.reason.code());
            assert_eq!(value["scheduled"], entry.is_scheduled());
            assert_eq!(value["satellite"]["id"], entry.satellite.get());
            assert_eq!(value["satellite"]["name"], entry.satellite_name);
            assert_eq!(value["detail"], entry.detail);
        }
        // The six-digit catalog number survives to the output a program reads.
        assert!(entries.iter().any(|e| e["satellite"]["id"] == 100_123));
    }
}

#[test]
fn text_output_shows_the_schedule_the_reasons_and_the_summaries() {
    let plan = plan(Scheduler::Greedy);
    let text = plan.to_text();
    assert!(text.starts_with("Contact plan (greedy scheduler)\n"));
    for needle in [
        "\nSCHEDULED\n",
        "\nNOT SCHEDULED\n",
        "\nSATELLITES\n",
        "\nSTATIONS\n",
        "EXAMPLE-A",
        "Svalbard",
        "MAAT-101: dropped: overlaps a higher-priority pass at the same station",
        "MAAT-105: dropped: tracking the pass needs a faster antenna slew",
    ] {
        assert!(text.contains(needle), "the report lacks {needle:?}");
    }
    assert!(!text.contains("INCOMPLETE"));
    for line in text.lines() {
        assert_eq!(line, line.trim_end());
    }
    // One row per pass in the two pass tables, plus two explanation lines per unscheduled pass.
    let rows = text
        .lines()
        .filter(|line| line.starts_with("  2026-08-"))
        .count();
    assert_eq!(rows, plan.entries.len());
}

// ---------------------------------------------------------------------------------------------
// An incomplete survey makes an incomplete plan, and says so
// ---------------------------------------------------------------------------------------------

#[test]
fn a_satellite_that_cannot_be_propagated_marks_the_plan_incomplete() {
    let mut fleet = satellites();
    let decaying = Omm::parse(
        r#"{"OBJECT_NAME":"FALLING","NORAD_CAT_ID":70000,"EPOCH":"2026-08-01T12:00:00",
            "MEAN_MOTION":16.3,"ECCENTRICITY":0.001,"INCLINATION":97.5,"RA_OF_ASC_NODE":120.0,
            "ARG_OF_PERICENTER":45.0,"MEAN_ANOMALY":315.0,"BSTAR":0.5}"#,
    )
    .unwrap();
    fleet.push(Satellite::from_omm(&decaying).unwrap());
    let survey =
        contact_windows(&fleet, &stations(), epoch(), epoch() + Duration::days(3)).unwrap();
    let plan = plan_contacts(&survey, &fleet, &stations(), &requests(), Scheduler::Greedy).unwrap();

    assert!(!plan.is_complete());
    assert_eq!(plan.failures.len(), stations().len());
    assert!(
        plan.failures[0].contains("satellite 70000"),
        "{}",
        plan.failures[0]
    );
    assert!(
        plan.scheduled().count() > 0,
        "the other satellites are still planned"
    );

    let text = plan.to_text();
    assert!(text.contains("INCOMPLETE: 4 satellite/station pairs could not be searched"));
    assert!(text.contains("\nNOT SEARCHED\n"));
    assert_eq!(plan.to_json_value()["complete"], false);
}
