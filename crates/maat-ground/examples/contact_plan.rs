//! Builds an explained contact plan for five satellites over four ground stations, then compares
//! the greedy scheduler with the optimal one.
//!
//! Run with:  `cargo run -p maat-ground --example contact_plan`
//! For JSON:  `cargo run -p maat-ground --example contact_plan -- --json`
//!
//! Offline and deterministic: the satellites are synthetic element sets with a fixed epoch.

use chrono::Duration;
use maat_ground::flight::{Priority, ReasonCode, SatelliteRequest, Seconds, StationId};
use maat_ground::{
    GroundStation, PlanOptions, Scheduler, SlewLimits, StationSite, contact_windows, plan_contacts,
    plan_contacts_with, satellites_from_omm, time_until_first_pass,
};

/// Five synthetic satellites as OMM JSON. B, C and D share an orbit, so they compete for every
/// station they pass. E is in the catalog but nobody has asked for contact with it.
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

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let as_json = std::env::args().any(|argument| argument == "--json");
    let satellites = satellites_from_omm(CATALOG)?;

    let site = |latitude_deg, longitude_deg| StationSite {
        latitude_deg,
        longitude_deg,
        altitude_m: 0.0,
    };
    let stations = [
        // A slow mount that also needs two minutes to re-point between passes.
        GroundStation::new(StationId::new(1), "Seattle", site(47.61, -122.33), 10.0)?
            .with_slew_limits(SlewLimits {
                azimuth_deg_per_s: 2.0,
                elevation_deg_per_s: 2.0,
            })?
            .with_turnaround(Seconds::new(120)),
        GroundStation::new(StationId::new(2), "Quito", site(-0.18, -78.47), 10.0)?,
        GroundStation::new(StationId::new(3), "Svalbard", site(78.23, 15.41), 10.0)?
            .with_turnaround(Seconds::new(60)),
        GroundStation::new(StationId::new(4), "Cape Town", site(-33.92, 18.42), 10.0)?,
    ];

    // What each satellite is asking for. EXAMPLE-E gets no request.
    let id = |index: usize| satellites[index].id();
    let requests = [
        // Urgent, and needs twenty minutes in total.
        SatelliteRequest::new(id(0), Priority::new(9)).with_need(Seconds::new(1_200)),
        // Equal priority, and no use for a pass shorter than three minutes.
        SatelliteRequest::new(id(1), Priority::new(4)).with_min_contact(Seconds::new(180)),
        SatelliteRequest::new(id(2), Priority::new(4)).with_min_contact(Seconds::new(180)),
        // Takes whatever is left.
        SatelliteRequest::new(id(3), Priority::new(2)),
    ];

    // Six hours, planned greedily and printed in full.
    let start = satellites[0].epoch();
    let survey = contact_windows(&satellites, &stations, start, start + Duration::hours(6))?;
    let plan = plan_contacts(
        &survey,
        &satellites,
        &stations,
        &requests,
        Scheduler::Greedy,
    )?;
    if as_json {
        println!("{}", plan.to_json());
        return Ok(());
    }
    print!("{}", plan.to_text());

    // A week, planned both ways, to see what optimization gains.
    println!("\nGREEDY AGAINST OPTIMAL, ONE WEEK");
    let week = contact_windows(&satellites, &stations, start, start + Duration::days(7))?;
    let greedy = plan_contacts(&week, &satellites, &stations, &requests, Scheduler::Greedy)?;

    // The optimal scheduler searches until it has proved its plan is the best, or until a time
    // limit. A plan is needed before its first pass, so that is the longest limit that makes
    // sense. Here the planning is done an hour ahead and capped at ten seconds.
    let now = start - Duration::hours(1);
    let limit = time_until_first_pass(&week, now)
        .unwrap_or_default()
        .min(std::time::Duration::from_secs(10));
    let options = PlanOptions::new(Scheduler::Optimal).with_optimal_time_limit(limit);
    let optimal = plan_contacts_with(&week, &satellites, &stations, &requests, &options)?;
    for plan in [&greedy, &optimal] {
        println!(
            "  {:<8} {:>3} of {} passes scheduled, {:>6} s of contact, value {}",
            plan.scheduler.name(),
            plan.scheduled().count(),
            plan.entries.len(),
            plan.scheduled_contact().get(),
            plan.value
        );
    }
    println!(
        "  The optimal plan is worth {:.1} % more.",
        100.0 * (optimal.value - greedy.value) / greedy.value
    );
    if let Some(optimality) = optimal.optimality {
        println!(
            "  It is {} (time limit {} s).",
            if optimality.proven {
                "proven: no plan is worth more"
            } else {
                "the best found before the time limit, not proven"
            },
            optimality.time_limit.as_secs()
        );
    }

    // The optimal plan is better in total, but it can take contact from one satellite to give
    // to another. A floor stops that: no satellite gets less than the greedy plan gives it.
    let floored = plan_contacts_with(
        &week,
        &satellites,
        &stations,
        &requests,
        &options.with_satellite_floor(true),
    )?;
    println!("\nCONTACT PER SATELLITE, ONE WEEK (seconds)");
    println!(
        "  {:<10} {:>8} {:>8} {:>18}",
        "", "greedy", "optimal", "optimal with floor"
    );
    for ((g, o), f) in greedy
        .satellites
        .iter()
        .zip(&optimal.satellites)
        .zip(&floored.satellites)
    {
        if g.request.is_some() {
            println!(
                "  {:<10} {:>8} {:>8} {:>18}",
                g.name,
                g.contact_scheduled.get(),
                o.contact_scheduled.get(),
                f.contact_scheduled.get()
            );
        }
    }
    println!(
        "  {:<10} {:>8} {:>8} {:>18}",
        "value", greedy.value, optimal.value, floored.value
    );
    println!(
        "  With the floor the optimal plan is worth {:.1} % more than greedy.",
        100.0 * (floored.value - greedy.value) / greedy.value
    );

    // Seattle's antenna turns at 2 degrees per second, so between two passes it needs its
    // fixed 120 s and the time to turn from where one sets to where the next rises.
    println!("\nA PASS LOST TO ANTENNA TURNING TIME");
    // Look for a pass that clears the fixed 120 s and is still too close.
    let lost_to_slew = greedy.entries.iter().find(|entry| {
        let Some(blocker) = entry.blocked_by.map(|index| &greedy.entries[index]) else {
            return false;
        };
        let gap = (entry.aos - blocker.los).max(blocker.aos - entry.los);
        entry.reason == ReasonCode::StationTurnaround && gap >= Duration::seconds(120)
    });
    match lost_to_slew {
        Some(entry) => println!(
            "  {} over {}, {}\n  {}",
            entry.satellite_name,
            entry.station_name,
            entry.aos.format("%Y-%m-%d %H:%M:%S"),
            entry.explanation()
        ),
        None => println!("  (none this week)"),
    }
    Ok(())
}
