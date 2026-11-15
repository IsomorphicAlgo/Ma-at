//! Surveys one day of contact windows for two satellites over three ground stations.
//!
//! Run with: `cargo run -p maat-ground --example window_survey`
//!
//! Offline and deterministic: the satellites are synthetic element sets with a fixed epoch, given
//! as OMM key-value notation, so the output is the same every time. One station has antenna slew
//! limits, which shows a pass being rejected with its reason.

use chrono::Duration;
use maat_ground::flight::StationId;
use maat_ground::{
    GroundStation, SlewLimits, StationSite, contact_windows, satellites_from_omm,
    utc_from_timestamp,
};

/// Two synthetic satellites. The second catalog number does not fit in a two-line element set.
const CATALOG: &str = "\
CCSDS_OMM_VERS = 2.0
OBJECT_NAME = EXAMPLE-A
NORAD_CAT_ID = 65001
EPOCH = 2026-08-01T12:00:00
MEAN_MOTION = 15.49
ECCENTRICITY = 0.0002
INCLINATION = 51.64
RA_OF_ASC_NODE = 221.0
ARG_OF_PERICENTER = 89.0
MEAN_ANOMALY = 280.0
BSTAR = 0.00003

CCSDS_OMM_VERS = 2.0
OBJECT_NAME = EXAMPLE-B
NORAD_CAT_ID = 100123
EPOCH = 2026-08-01T12:00:00
MEAN_MOTION = 15.2
ECCENTRICITY = 0.0012
INCLINATION = 97.5
RA_OF_ASC_NODE = 120.0
ARG_OF_PERICENTER = 45.0
MEAN_ANOMALY = 315.0
BSTAR = 0.0001
";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let satellites = satellites_from_omm(CATALOG)?;

    let site = |latitude_deg, longitude_deg| StationSite {
        latitude_deg,
        longitude_deg,
        altitude_m: 0.0,
    };
    let stations = [
        // A large dish on a slow mount: one degree per second about each axis. Its mask is low,
        // so it sees a lot, but it cannot follow a pass that climbs high, where the antenna has
        // to swing round quickly at the top.
        GroundStation::new(StationId::new(1), "Seattle", site(47.61, -122.33), 5.0)?
            .with_slew_limits(SlewLimits {
                azimuth_deg_per_s: 1.0,
                elevation_deg_per_s: 1.0,
            })?,
        GroundStation::new(StationId::new(2), "Quito", site(-0.18, -78.47), 10.0)?,
        GroundStation::new(StationId::new(3), "Svalbard", site(78.23, 15.41), 10.0)?,
    ];

    let start = satellites[0].epoch();
    let survey = contact_windows(&satellites, &stations, start, start + Duration::days(1))?;

    let name_of = |id| {
        stations
            .iter()
            .find(|s| s.id() == id)
            .map_or("?", GroundStation::name)
    };

    println!(
        "{} satellites x {} stations, 24 h from {}",
        satellites.len(),
        stations.len(),
        start.format("%Y-%m-%d %H:%M:%S UTC")
    );
    println!();
    println!(
        "{:<9}  {:<9}  {:<8}  {:<8}  {:>6}  {:>9}",
        "SATELLITE", "STATION", "AOS", "LOS", "LENGTH", "PEAK"
    );
    for window in &survey.windows {
        println!(
            "{:<9}  {:<9}  {:<8}  {:<8}  {:>6}  {:>9}",
            window.satellite().to_string(),
            name_of(window.station()),
            utc_from_timestamp(window.aos())?.format("%H:%M:%S"),
            utc_from_timestamp(window.los())?.format("%H:%M:%S"),
            window.duration().to_string(),
            window.max_elevation().to_string(),
        );
    }

    println!();
    println!("{} usable windows", survey.windows.len());
    println!("{} passes rejected", survey.rejected.len());
    for rejected in &survey.rejected {
        println!(
            "  {} over {} at {}, peak {:.1} deg",
            rejected.satellite,
            name_of(rejected.station),
            rejected.aos.format("%H:%M:%S"),
            rejected.max_elevation_deg
        );
        println!("    {}", rejected.reason);
        println!("    {}", rejected.detail);
    }
    println!("{} satellite/station pairs failed", survey.failures.len());
    Ok(())
}
