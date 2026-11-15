//! End-to-end check of the ground/flight boundary: a public element set goes through Ephemerust's
//! pass prediction and comes out as flight `ContactWindow`s.
//!
//! Deterministic and offline: a fixed public reference TLE (ISS, epoch 2020-07-12) and fixed
//! timestamps. Nothing here reads the clock or the network.
//!
//! What is checked is the *conversion*, not the astrodynamics. Ephemerust's own accuracy is
//! validated in its repository (`tests/external_reference.rs`); this file takes its passes as
//! given and verifies Ma'at carries them across the boundary without overstating them.

use chrono::{DateTime, Duration, TimeZone, Utc};
use ephemerust::{Pass, Tle, predict_passes};
use maat_ground::flight::{ContactWindow, SatelliteId, StationId};
use maat_ground::{GroundStation, StationSite, utc_from_timestamp, window_from_pass};

const ISS_TLE: &str = "ISS (ZARYA)\n\
    1 25544U 98067A   20194.88612269 -.00002218  00000-0 -31515-4 0  9992\n\
    2 25544  51.6461 221.2784 0001413  89.1723 280.4612 15.49507896236008";

/// The elevation mask used throughout, in degrees.
const MASK_DEG: f64 = 10.0;

fn iss() -> Tle {
    Tle::parse(ISS_TLE).expect("the reference TLE is valid")
}

fn seattle() -> GroundStation {
    GroundStation::new(
        StationId::new(1),
        "Seattle",
        StationSite {
            latitude_deg: 47.6062,
            longitude_deg: -122.3321,
            altitude_m: 50.0,
        },
        MASK_DEG,
    )
    .expect("the station is valid")
}

/// The search window: 24 hours starting just before the element set's epoch.
fn search_window() -> (DateTime<Utc>, DateTime<Utc>) {
    let start = Utc.with_ymd_and_hms(2020, 7, 12, 21, 0, 0).unwrap();
    (start, start + Duration::days(1))
}

fn predicted_passes() -> Vec<Pass> {
    let (start, end) = search_window();
    let station = seattle();
    predict_passes(
        &iss(),
        station.observer(),
        start,
        end,
        station.min_elevation_deg(),
    )
    .expect("pass prediction succeeds for the reference TLE")
}

fn converted_windows() -> Vec<(Pass, ContactWindow)> {
    let satellite = SatelliteId::new(iss().catalog_number);
    let station = seattle().id();
    predicted_passes()
        .into_iter()
        .map(|pass| {
            let window = window_from_pass(satellite, station, &pass)
                .expect("every predicted pass converts to a contact window");
            (pass, window)
        })
        .collect()
}

#[test]
fn the_iss_makes_several_passes_over_the_station_in_a_day() {
    // At 51.6 degrees inclination the ISS passes a mid-latitude site a handful of times a day
    // above a 10 degree mask. The exact count depends on the geometry; a sane range guards
    // against the pipeline returning nothing or nonsense.
    let count = predicted_passes().len();
    assert!(
        (2..=8).contains(&count),
        "expected a handful of passes, got {count}"
    );
}

#[test]
fn every_window_carries_the_satellite_and_station_it_was_built_for() {
    for (_, window) in converted_windows() {
        assert_eq!(window.satellite(), SatelliteId::new(25544));
        assert_eq!(window.station(), StationId::new(1));
    }
}

#[test]
fn every_window_lies_inside_its_pass() {
    // The inward-rounding rule: the integer window must never start before the real AOS or end
    // after the real LOS.
    for (pass, window) in converted_windows() {
        let aos = utc_from_timestamp(window.aos()).unwrap();
        let los = utc_from_timestamp(window.los()).unwrap();
        assert!(
            aos >= pass.aos,
            "window AOS {aos} precedes pass AOS {}",
            pass.aos
        );
        assert!(
            los <= pass.los,
            "window LOS {los} follows pass LOS {}",
            pass.los
        );
    }
}

#[test]
fn rounding_costs_less_than_two_seconds_of_contact() {
    // Tolerance: AOS rounds up by less than 1 s and LOS rounds down by less than 1 s, so the
    // window is shorter than the pass by at least 0 s and strictly less than 2 s.
    for (pass, window) in converted_windows() {
        let pass_seconds = (pass.los - pass.aos).num_milliseconds() as f64 / 1000.0;
        let lost = pass_seconds - f64::from(window.duration().get());
        assert!(
            (0.0..2.0).contains(&lost),
            "lost {lost} s of a {pass_seconds} s pass"
        );
    }
}

#[test]
fn maximum_elevation_is_rounded_down_by_less_than_a_hundredth_of_a_degree() {
    // Tolerance: floor to 0.01 degrees loses at most 0.01; 1e-9 absorbs floating-point error in
    // the comparison itself.
    for (pass, window) in converted_windows() {
        let stored = f64::from(window.max_elevation().centidegrees()) / 100.0;
        let lost = pass.max_elevation_deg - stored;
        assert!(
            (0.0..0.01 + 1e-9).contains(&lost),
            "stored {stored} for a peak of {}",
            pass.max_elevation_deg
        );
    }
}

#[test]
fn every_window_peaks_above_the_station_mask() {
    for (_, window) in converted_windows() {
        let peak = f64::from(window.max_elevation().centidegrees()) / 100.0;
        // The floor can take up to 0.01 degrees off a pass that barely clears the mask.
        assert!(peak >= MASK_DEG - 0.01, "peak {peak} is below the mask");
    }
}

#[test]
fn passes_last_minutes_not_seconds_or_hours() {
    // A low-Earth-orbit pass above a 10 degree mask lasts from under a minute (a grazing pass)
    // to roughly ten minutes (overhead). Anything outside 10 s to 15 min points at a unit or
    // epoch mistake in the conversion.
    for (_, window) in converted_windows() {
        let seconds = window.duration().get();
        assert!(
            (10..=900).contains(&seconds),
            "implausible pass length {seconds} s"
        );
    }
}

#[test]
fn windows_for_one_satellite_and_station_are_in_order_and_never_overlap() {
    let windows: Vec<ContactWindow> = converted_windows().into_iter().map(|(_, w)| w).collect();
    for pair in windows.windows(2) {
        assert!(
            pair[0].los() <= pair[1].aos(),
            "{:?} then {:?}",
            pair[0],
            pair[1]
        );
        assert!(!pair[0].overlaps(&pair[1]));
    }
}

#[test]
fn every_window_falls_within_the_search_window() {
    let (start, end) = search_window();
    for (_, window) in converted_windows() {
        assert!(utc_from_timestamp(window.aos()).unwrap() >= start);
        assert!(utc_from_timestamp(window.los()).unwrap() <= end);
    }
}

#[test]
fn the_conversion_is_deterministic() {
    let first: Vec<ContactWindow> = converted_windows().into_iter().map(|(_, w)| w).collect();
    let second: Vec<ContactWindow> = converted_windows().into_iter().map(|(_, w)| w).collect();
    assert_eq!(first, second);
}
