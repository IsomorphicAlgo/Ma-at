//! Prints one day of ISS contact windows over a single station.
//!
//! Run with: `cargo run -p maat-ground --example iss_windows`
//!
//! Offline and deterministic: it uses a fixed public reference element set, not the clock or the
//! network, so the output is the same every time.

use chrono::{Duration, TimeZone, Utc};
use ephemerust::{Tle, predict_passes};
use maat_ground::flight::{SatelliteId, StationId};
use maat_ground::{GroundStation, StationSite, utc_from_timestamp, window_from_pass};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let tle = Tle::parse(
        "ISS (ZARYA)\n\
         1 25544U 98067A   20194.88612269 -.00002218  00000-0 -31515-4 0  9992\n\
         2 25544  51.6461 221.2784 0001413  89.1723 280.4612 15.49507896236008",
    )?;
    let station = GroundStation::new(
        StationId::new(1),
        "Seattle",
        StationSite {
            latitude_deg: 47.6062,
            longitude_deg: -122.3321,
            altitude_m: 50.0,
        },
        10.0,
    )?;

    let start = Utc
        .with_ymd_and_hms(2020, 7, 12, 21, 0, 0)
        .single()
        .ok_or("invalid start time")?;
    let end = start + Duration::days(1);
    let passes = predict_passes(
        &tle,
        station.observer(),
        start,
        end,
        station.min_elevation_deg(),
    )?;

    let satellite = SatelliteId::new(tle.catalog_number);
    println!(
        "Satellite {satellite} over station {} \"{}\" (mask {} deg)",
        station.id(),
        station.name(),
        station.min_elevation_deg()
    );
    println!(
        "{:<20}  {:<20}  {:>8}  {:>9}",
        "AOS (UTC)", "LOS (UTC)", "LENGTH", "PEAK"
    );
    for pass in &passes {
        let window = window_from_pass(satellite, station.id(), pass)?;
        println!(
            "{:<20}  {:<20}  {:>8}  {:>9}",
            utc_from_timestamp(window.aos())?.format("%Y-%m-%d %H:%M:%S"),
            utc_from_timestamp(window.los())?.format("%Y-%m-%d %H:%M:%S"),
            window.duration().to_string(),
            window.max_elevation().to_string(),
        );
    }
    Ok(())
}
