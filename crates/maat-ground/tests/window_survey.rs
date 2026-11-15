//! End-to-end tests of the Stage 1 window survey: OMM text in, sorted contact windows out.
//!
//! Deterministic and offline. The satellites are synthetic element sets with a fixed epoch
//! (2026-08-01T12:00:00 UTC), not real objects; the one real object is the public ISS reference
//! TLE used to compare TLE and OMM input. Nothing here reads the clock or the network.
//!
//! Three kinds of check:
//!
//! - **Against Ephemerust directly.** The survey must return exactly what calling Ephemerust
//!   pair by pair would, so the N x M bookkeeping adds and loses nothing.
//! - **Against a brute-force scan.** Elevation is sampled every second with no pass-finding
//!   logic at all, and every pass that scan finds must be in the survey with matching edges.
//! - **Behaviour.** Ordering, the four OMM formats, slew limits, clipping at the search window,
//!   and what happens when one satellite cannot be propagated.

use chrono::{DateTime, Duration, TimeZone, Utc};
use ephemerust::{ObserverLocation, Omm, Propagator, Tle};
use maat_ground::flight::{ContactWindow, ReasonCode, SatelliteId, StationId};
use maat_ground::{
    GroundError, GroundStation, Satellite, SlewLimits, StationSite, WindowSurvey, contact_windows,
    satellites_from_omm, timestamp_ceil, timestamp_floor, utc_from_timestamp, window_from_pass,
};

// ---------------------------------------------------------------------------------------------
// Fixtures: the same two-satellite catalog in each of the four OMM formats.
//
//   EXAMPLE-A   65001   51.64 deg inclination, 15.49 rev/day (an ISS-like orbit)
//   EXAMPLE-B  100123   97.5 deg inclination,  15.2 rev/day  (a sun-synchronous-like orbit,
//                                                             numbered beyond what a TLE holds)
// ---------------------------------------------------------------------------------------------

const CATALOG_JSON: &str = r#"[
  {"OBJECT_NAME":"EXAMPLE-A","OBJECT_ID":"2026-150A","EPOCH":"2026-08-01T12:00:00.000000",
   "MEAN_MOTION":15.49,"ECCENTRICITY":0.0002,"INCLINATION":51.64,"RA_OF_ASC_NODE":221.0,
   "ARG_OF_PERICENTER":89.0,"MEAN_ANOMALY":280.0,"EPHEMERIS_TYPE":0,"CLASSIFICATION_TYPE":"U",
   "NORAD_CAT_ID":65001,"ELEMENT_SET_NO":999,"REV_AT_EPOCH":42,"BSTAR":0.00003,
   "MEAN_MOTION_DOT":0.00002,"MEAN_MOTION_DDOT":0},
  {"OBJECT_NAME":"EXAMPLE-B","OBJECT_ID":"2026-150B","EPOCH":"2026-08-01T12:00:00.000000",
   "MEAN_MOTION":15.2,"ECCENTRICITY":0.0012,"INCLINATION":97.5,"RA_OF_ASC_NODE":120.0,
   "ARG_OF_PERICENTER":45.0,"MEAN_ANOMALY":315.0,"EPHEMERIS_TYPE":0,"CLASSIFICATION_TYPE":"U",
   "NORAD_CAT_ID":100123,"ELEMENT_SET_NO":999,"REV_AT_EPOCH":42,"BSTAR":0.0001,
   "MEAN_MOTION_DOT":0.00001,"MEAN_MOTION_DDOT":0}
]"#;

const CATALOG_CSV: &str = "\
OBJECT_NAME,OBJECT_ID,EPOCH,MEAN_MOTION,ECCENTRICITY,INCLINATION,RA_OF_ASC_NODE,ARG_OF_PERICENTER,MEAN_ANOMALY,EPHEMERIS_TYPE,CLASSIFICATION_TYPE,NORAD_CAT_ID,ELEMENT_SET_NO,REV_AT_EPOCH,BSTAR,MEAN_MOTION_DOT,MEAN_MOTION_DDOT
EXAMPLE-A,2026-150A,2026-08-01T12:00:00.000000,15.49,.0002,51.64,221.0,89.0,280.0,0,U,65001,999,42,.3E-4,.2E-4,0
EXAMPLE-B,2026-150B,2026-08-01T12:00:00.000000,15.2,.0012,97.5,120.0,45.0,315.0,0,U,100123,999,42,.1E-3,.1E-4,0
";

const CATALOG_KVN: &str = "\
CCSDS_OMM_VERS = 2.0
COMMENT Synthetic element sets for testing
OBJECT_NAME = EXAMPLE-A
OBJECT_ID = 2026-150A
CENTER_NAME = EARTH
REF_FRAME = TEME
TIME_SYSTEM = UTC
MEAN_ELEMENT_THEORY = SGP4
EPOCH = 2026-08-01T12:00:00.000000
MEAN_MOTION = 15.49 [rev/day]
ECCENTRICITY = 0.0002
INCLINATION = 51.64 [deg]
RA_OF_ASC_NODE = 221.0 [deg]
ARG_OF_PERICENTER = 89.0 [deg]
MEAN_ANOMALY = 280.0 [deg]
EPHEMERIS_TYPE = 0
CLASSIFICATION_TYPE = U
NORAD_CAT_ID = 65001
ELEMENT_SET_NO = 999
REV_AT_EPOCH = 42
BSTAR = 0.00003
MEAN_MOTION_DOT = 0.00002
MEAN_MOTION_DDOT = 0

CCSDS_OMM_VERS = 2.0
OBJECT_NAME = EXAMPLE-B
OBJECT_ID = 2026-150B
CENTER_NAME = EARTH
REF_FRAME = TEME
TIME_SYSTEM = UTC
MEAN_ELEMENT_THEORY = SGP4
EPOCH = 2026-08-01T12:00:00.000000
MEAN_MOTION = 15.2 [rev/day]
ECCENTRICITY = 0.0012
INCLINATION = 97.5 [deg]
RA_OF_ASC_NODE = 120.0 [deg]
ARG_OF_PERICENTER = 45.0 [deg]
MEAN_ANOMALY = 315.0 [deg]
EPHEMERIS_TYPE = 0
CLASSIFICATION_TYPE = U
NORAD_CAT_ID = 100123
ELEMENT_SET_NO = 999
REV_AT_EPOCH = 42
BSTAR = 0.0001
MEAN_MOTION_DOT = 0.00001
MEAN_MOTION_DDOT = 0
";

const CATALOG_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<ndm>
<!-- Synthetic element sets for testing -->
<omm id="CCSDS_OMM_VERS" version="2.0">
<header><CREATION_DATE/><ORIGINATOR/></header>
<body><segment>
<metadata><OBJECT_NAME>EXAMPLE-A</OBJECT_NAME><OBJECT_ID>2026-150A</OBJECT_ID><CENTER_NAME>EARTH</CENTER_NAME><REF_FRAME>TEME</REF_FRAME><TIME_SYSTEM>UTC</TIME_SYSTEM><MEAN_ELEMENT_THEORY>SGP4</MEAN_ELEMENT_THEORY></metadata>
<data>
<meanElements><EPOCH>2026-08-01T12:00:00.000000</EPOCH><MEAN_MOTION>15.49</MEAN_MOTION><ECCENTRICITY>.0002</ECCENTRICITY><INCLINATION>51.64</INCLINATION><RA_OF_ASC_NODE>221.0</RA_OF_ASC_NODE><ARG_OF_PERICENTER>89.0</ARG_OF_PERICENTER><MEAN_ANOMALY>280.0</MEAN_ANOMALY></meanElements>
<tleParameters><EPHEMERIS_TYPE>0</EPHEMERIS_TYPE><CLASSIFICATION_TYPE>U</CLASSIFICATION_TYPE><NORAD_CAT_ID>65001</NORAD_CAT_ID><ELEMENT_SET_NO>999</ELEMENT_SET_NO><REV_AT_EPOCH>42</REV_AT_EPOCH><BSTAR>.3E-4</BSTAR><MEAN_MOTION_DOT>.2E-4</MEAN_MOTION_DOT><MEAN_MOTION_DDOT>0</MEAN_MOTION_DDOT></tleParameters>
</data>
</segment></body>
</omm>
<omm id="CCSDS_OMM_VERS" version="2.0">
<header><CREATION_DATE/><ORIGINATOR/></header>
<body><segment>
<metadata><OBJECT_NAME>EXAMPLE-B</OBJECT_NAME><OBJECT_ID>2026-150B</OBJECT_ID><CENTER_NAME>EARTH</CENTER_NAME><REF_FRAME>TEME</REF_FRAME><TIME_SYSTEM>UTC</TIME_SYSTEM><MEAN_ELEMENT_THEORY>SGP4</MEAN_ELEMENT_THEORY></metadata>
<data>
<meanElements><EPOCH>2026-08-01T12:00:00.000000</EPOCH><MEAN_MOTION>15.2</MEAN_MOTION><ECCENTRICITY>.0012</ECCENTRICITY><INCLINATION>97.5</INCLINATION><RA_OF_ASC_NODE>120.0</RA_OF_ASC_NODE><ARG_OF_PERICENTER>45.0</ARG_OF_PERICENTER><MEAN_ANOMALY>315.0</MEAN_ANOMALY></meanElements>
<tleParameters><EPHEMERIS_TYPE>0</EPHEMERIS_TYPE><CLASSIFICATION_TYPE>U</CLASSIFICATION_TYPE><NORAD_CAT_ID>100123</NORAD_CAT_ID><ELEMENT_SET_NO>999</ELEMENT_SET_NO><REV_AT_EPOCH>42</REV_AT_EPOCH><BSTAR>.1E-3</BSTAR><MEAN_MOTION_DOT>.1E-4</MEAN_MOTION_DOT><MEAN_MOTION_DDOT>0</MEAN_MOTION_DDOT></tleParameters>
</data>
</segment></body>
</omm>
</ndm>
"#;

const EXAMPLE_A: SatelliteId = SatelliteId::new(65_001);
const EXAMPLE_B: SatelliteId = SatelliteId::new(100_123);

const SEATTLE: StationId = StationId::new(1);
const QUITO: StationId = StationId::new(2);
const SVALBARD: StationId = StationId::new(3);
const CAPE_TOWN: StationId = StationId::new(4);

/// The elevation mask used unless a test says otherwise, in degrees.
const MASK_DEG: f64 = 10.0;

fn epoch() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 8, 1, 12, 0, 0).unwrap()
}

fn satellites() -> Vec<Satellite> {
    satellites_from_omm(CATALOG_JSON).expect("the JSON catalog is valid")
}

fn station(id: StationId, name: &str, latitude_deg: f64, longitude_deg: f64) -> GroundStation {
    station_with_mask(id, name, latitude_deg, longitude_deg, MASK_DEG)
}

fn station_with_mask(
    id: StationId,
    name: &str,
    latitude_deg: f64,
    longitude_deg: f64,
    mask_deg: f64,
) -> GroundStation {
    GroundStation::new(
        id,
        name,
        StationSite {
            latitude_deg,
            longitude_deg,
            altitude_m: 0.0,
        },
        mask_deg,
    )
    .expect("the station is valid")
}

/// Four stations spread over latitude: mid-north, equator, high Arctic and mid-south.
fn stations() -> Vec<GroundStation> {
    vec![
        station(SEATTLE, "Seattle", 47.61, -122.33),
        station(QUITO, "Quito", -0.18, -78.47),
        station(SVALBARD, "Svalbard", 78.23, 15.41),
        station(CAPE_TOWN, "Cape Town", -33.92, 18.42),
    ]
}

/// A one-day survey of the whole fixture: 2 satellites x 4 stations.
fn one_day() -> WindowSurvey {
    contact_windows(
        &satellites(),
        &stations(),
        epoch(),
        epoch() + Duration::days(1),
    )
    .expect("the request is valid")
}

fn windows_for(
    survey: &WindowSurvey,
    satellite: SatelliteId,
    station: StationId,
) -> Vec<ContactWindow> {
    survey
        .windows
        .iter()
        .filter(|w| w.satellite() == satellite && w.station() == station)
        .copied()
        .collect()
}

fn observer_of(station: &GroundStation) -> ObserverLocation {
    station.observer()
}

// ---------------------------------------------------------------------------------------------
// OMM input
// ---------------------------------------------------------------------------------------------

#[test]
fn all_four_omm_formats_give_the_same_survey() {
    let reference = one_day();
    assert!(
        reference.windows.len() > 20,
        "the fixture should yield plenty of windows"
    );

    for (format, text) in [
        ("CSV", CATALOG_CSV),
        ("KVN", CATALOG_KVN),
        ("XML", CATALOG_XML),
    ] {
        let satellites = satellites_from_omm(text)
            .unwrap_or_else(|error| panic!("the {format} catalog failed to load: {error}"));
        assert_eq!(satellites.len(), 2, "{format}");
        let survey = contact_windows(
            &satellites,
            &stations(),
            epoch(),
            epoch() + Duration::days(1),
        )
        .unwrap();
        assert_eq!(
            survey.windows, reference.windows,
            "{format} differs from JSON"
        );
        assert!(survey.is_complete(), "{format}");
    }
}

#[test]
fn a_catalog_number_beyond_five_digits_reaches_the_contact_windows() {
    // The reason for OMM input: 100123 cannot be written in a plain TLE.
    let survey = one_day();
    let six_digit: Vec<_> = survey
        .windows
        .iter()
        .filter(|w| w.satellite() == EXAMPLE_B)
        .collect();
    assert!(!six_digit.is_empty());
    assert!(six_digit.iter().all(|w| w.satellite().get() == 100_123));
}

#[test]
fn a_tle_and_the_omm_for_the_same_element_set_give_the_same_windows() {
    // The public ISS reference set, as a TLE and as the OMM a provider would publish for it.
    let tle = Tle::parse(
        "ISS (ZARYA)\n\
         1 25544U 98067A   20194.88612269 -.00002218  00000-0 -31515-4 0  9992\n\
         2 25544  51.6461 221.2784 0001413  89.1723 280.4612 15.49507896236008",
    )
    .unwrap();
    let omm = Omm::parse(
        r#"{"OBJECT_NAME":"ISS (ZARYA)","OBJECT_ID":"1998-067A","NORAD_CAT_ID":25544,
            "EPOCH":"2020-07-12T21:16:01.000416","MEAN_MOTION":15.49507896,
            "ECCENTRICITY":0.0001413,"INCLINATION":51.6461,"RA_OF_ASC_NODE":221.2784,
            "ARG_OF_PERICENTER":89.1723,"MEAN_ANOMALY":280.4612,"BSTAR":-0.31515E-4,
            "MEAN_MOTION_DOT":-0.2218E-4,"MEAN_MOTION_DDOT":0}"#,
    )
    .unwrap();

    let start = tle.epoch;
    let end = start + Duration::days(2);
    let from_tle = contact_windows(
        &[Satellite::from_tle(&tle).unwrap()],
        &stations(),
        start,
        end,
    )
    .unwrap();
    let from_omm = contact_windows(
        &[Satellite::from_omm(&omm).unwrap()],
        &stations(),
        start,
        end,
    )
    .unwrap();

    assert!(from_tle.windows.len() > 10);
    assert_eq!(from_tle.windows.len(), from_omm.windows.len());
    // The two formats round the epoch differently, which moves a pass by microseconds. That
    // can shift a whole-second edge only if the true edge sits on a second boundary, so the
    // tolerance is one second. Measured: every edge identical.
    for (a, b) in from_tle.windows.iter().zip(&from_omm.windows) {
        assert_eq!((a.satellite(), a.station()), (b.satellite(), b.station()));
        let aos_gap = (a.aos().unix_seconds() - b.aos().unix_seconds()).abs();
        let los_gap = (a.los().unix_seconds() - b.los().unix_seconds()).abs();
        assert!(aos_gap <= 1 && los_gap <= 1, "{a:?} vs {b:?}");
    }
}

// ---------------------------------------------------------------------------------------------
// Against Ephemerust directly
// ---------------------------------------------------------------------------------------------

#[test]
fn the_survey_returns_exactly_what_ephemerust_finds_pair_by_pair() {
    let survey = one_day();
    let messages = Omm::parse_all(CATALOG_JSON).unwrap();
    let mut expected_total = 0;

    for message in &messages {
        let satellite = SatelliteId::new(message.catalog_number.unwrap());
        let propagator = Propagator::from_omm(message).unwrap();
        for station in &stations() {
            let passes = propagator
                .predict_passes(
                    observer_of(station),
                    epoch(),
                    epoch() + Duration::days(1),
                    station.min_elevation_deg(),
                )
                .unwrap();
            let expected: Vec<ContactWindow> = passes
                .iter()
                .map(|pass| window_from_pass(satellite, station.id(), pass).unwrap())
                .collect();
            let found = windows_for(&survey, satellite, station.id());
            // The survey adds each window's pointing; the times and elevation must be exactly
            // what the bridge gives for the same pass.
            let stripped: Vec<ContactWindow> = found
                .iter()
                .map(|window| window.without_pointing())
                .collect();
            assert_eq!(
                stripped,
                expected,
                "satellite {satellite} over {}",
                station.name()
            );

            // The pointing is Ephemerust's look angle at the window's own AOS and LOS. Azimuth
            // is stored to the nearest hundredth of a degree (so within 0.005), elevation
            // rounded down (so 0 to 0.01 below).
            for window in &found {
                for (time, pointing) in [
                    (window.aos(), window.aos_pointing().unwrap()),
                    (window.los(), window.los_pointing().unwrap()),
                ] {
                    let when = Utc.timestamp_opt(time.unix_seconds(), 0).unwrap();
                    let look = propagator.look_angles(when, observer_of(station)).unwrap();
                    let azimuth = f64::from(pointing.azimuth.centidegrees()) / 100.0;
                    let elevation = f64::from(pointing.elevation.centidegrees()) / 100.0;
                    let around = (azimuth - look.azimuth_deg).rem_euclid(360.0);
                    assert!(
                        around.min(360.0 - around) <= 0.005 + 1e-9,
                        "azimuth {azimuth} against {}",
                        look.azimuth_deg
                    );
                    let below = look.elevation_deg - elevation;
                    assert!(
                        (-1e-9..0.01 + 1e-9).contains(&below),
                        "elevation {elevation} against {}",
                        look.elevation_deg
                    );
                }
            }
            expected_total += expected.len();
        }
    }

    // Nothing extra, nothing rejected, nothing failed: the survey is the sum of its pairs.
    assert_eq!(survey.windows.len(), expected_total);
    assert!(survey.rejected.is_empty());
    assert!(survey.is_complete());
}

// ---------------------------------------------------------------------------------------------
// Against a brute-force scan
// ---------------------------------------------------------------------------------------------

/// Samples elevation once a second and returns each run of seconds at or above the mask as
/// `(first second above, first second below, highest sampled elevation)`, in seconds from
/// `start`. This shares no logic with the pass search it checks.
fn brute_force_passes(
    propagator: &Propagator,
    observer: ObserverLocation,
    start: DateTime<Utc>,
    seconds: i64,
    mask_deg: f64,
) -> Vec<(i64, i64, f64)> {
    let mut runs = Vec::new();
    let mut open: Option<(i64, f64)> = None;
    for second in 0..seconds {
        let elevation = propagator
            .look_angles(start + Duration::seconds(second), observer)
            .unwrap()
            .elevation_deg;
        match (&mut open, elevation >= mask_deg) {
            (None, true) => open = Some((second, elevation)),
            (Some((_, peak)), true) => *peak = peak.max(elevation),
            (Some((first, peak)), false) => {
                runs.push((*first, second, *peak));
                open = None;
            }
            (None, false) => {}
        }
    }
    if let Some((first, peak)) = open {
        runs.push((first, seconds, peak));
    }
    runs
}

#[test]
fn every_pass_a_one_second_scan_finds_is_in_the_survey_with_matching_edges() {
    // The scan starts on a whole second and steps by one second, so:
    //   - its first second above the mask is exactly ceil(true AOS), which is the window's AOS;
    //   - its first second below the mask is floor(true LOS) + 1, one past the window's LOS.
    // Ephemerust refines each edge to 1 ms, so an edge that falls within a millisecond of a
    // second boundary could land on either side: the tolerance is one second. Measured on this
    // set: every edge exact.
    //
    // Two masks are used because a high mask makes every pass short, which is where a pass
    // search is most likely to lose one.
    let horizon_seconds = 86_400;
    let end = epoch() + Duration::seconds(horizon_seconds);
    let test_stations = [
        station_with_mask(SEATTLE, "Seattle", 47.61, -122.33, 10.0),
        station_with_mask(CAPE_TOWN, "Cape Town", -33.92, 18.42, 40.0),
    ];
    let survey = contact_windows(&satellites(), &test_stations, epoch(), end).unwrap();
    assert!(survey.is_complete() && survey.rejected.is_empty());

    let mut compared = 0;
    for message in &Omm::parse_all(CATALOG_JSON).unwrap() {
        let satellite = SatelliteId::new(message.catalog_number.unwrap());
        let propagator = Propagator::from_omm(message).unwrap();
        for station in &test_stations {
            let scan = brute_force_passes(
                &propagator,
                observer_of(station),
                epoch(),
                horizon_seconds,
                station.min_elevation_deg(),
            );
            // A run of one or two sampled seconds may round away to nothing; leave those out.
            let scan: Vec<_> = scan.into_iter().filter(|(a, l, _)| l - a >= 3).collect();
            let windows = windows_for(&survey, satellite, station.id());
            let long_windows: Vec<_> = windows.iter().filter(|w| w.duration().get() >= 3).collect();
            assert_eq!(
                long_windows.len(),
                scan.len(),
                "satellite {satellite} over {}: survey {windows:?}, scan {scan:?}",
                station.name()
            );

            for (window, &(first_above, first_below, sampled_peak)) in
                long_windows.iter().zip(&scan)
            {
                let aos = window.aos().unix_seconds() - epoch().timestamp();
                let los = window.los().unix_seconds() - epoch().timestamp();
                assert!(
                    (aos - first_above).abs() <= 1,
                    "AOS {aos} vs scan {first_above}"
                );
                assert!(
                    (los - (first_below - 1)).abs() <= 1,
                    "LOS {los} vs scan {first_below}"
                );

                // The true peak is at least the highest sample, and the window stores it rounded
                // down by at most 0.01 degrees. It can also exceed the highest sample, because a
                // one-second scan can straddle the top of a steep pass and read low (by up to
                // about 0.3 degrees on a near-overhead pass; under 0.01 on this set).
                let stored_peak = f64::from(window.max_elevation().centidegrees()) / 100.0;
                assert!(stored_peak >= sampled_peak - 0.01 - 1e-9);
                assert!(stored_peak <= sampled_peak + 0.3);
                compared += 1;
            }
        }
    }
    assert!(compared >= 10, "only {compared} passes were compared");
}

// ---------------------------------------------------------------------------------------------
// Ordering and structure
// ---------------------------------------------------------------------------------------------

#[test]
fn windows_come_back_sorted_by_aos() {
    let survey = one_day();
    for pair in survey.windows.windows(2) {
        let key = |w: &ContactWindow| (w.aos(), w.los(), w.station(), w.satellite());
        assert!(
            key(&pair[0]) < key(&pair[1]),
            "{:?} then {:?}",
            pair[0],
            pair[1]
        );
    }
}

#[test]
fn the_order_of_the_inputs_does_not_change_the_output() {
    let reference = one_day();
    let mut reversed_satellites = satellites();
    reversed_satellites.reverse();
    let mut reversed_stations = stations();
    reversed_stations.reverse();
    let survey = contact_windows(
        &reversed_satellites,
        &reversed_stations,
        epoch(),
        epoch() + Duration::days(1),
    )
    .unwrap();
    assert_eq!(survey.windows, reference.windows);
}

#[test]
fn the_survey_is_deterministic() {
    assert_eq!(one_day().windows, one_day().windows);
}

#[test]
fn every_window_lies_inside_the_search_window() {
    let (start, end) = (epoch(), epoch() + Duration::days(1));
    for window in &one_day().windows {
        assert!(utc_from_timestamp(window.aos()).unwrap() >= start);
        assert!(utc_from_timestamp(window.los()).unwrap() <= end);
    }
}

#[test]
fn windows_for_one_satellite_and_station_never_overlap() {
    let survey = one_day();
    for satellite in [EXAMPLE_A, EXAMPLE_B] {
        for station in [SEATTLE, QUITO, SVALBARD, CAPE_TOWN] {
            let windows = windows_for(&survey, satellite, station);
            for pair in windows.windows(2) {
                assert!(pair[0].los() <= pair[1].aos());
                assert!(!pair[0].overlaps(&pair[1]));
            }
        }
    }
}

#[test]
fn two_satellites_can_want_one_station_at_the_same_time() {
    // A scheduler exists because windows compete. Over a week the two satellites must overlap
    // at some station at least once, or Stage 2 would have nothing to resolve with this fixture.
    let survey = contact_windows(
        &satellites(),
        &stations(),
        epoch(),
        epoch() + Duration::days(7),
    )
    .unwrap();
    let contested = survey
        .windows
        .iter()
        .enumerate()
        .flat_map(|(i, a)| survey.windows[i + 1..].iter().map(move |b| (a, b)))
        .filter(|(a, b)| a.station() == b.station() && a.overlaps(b))
        .count();
    assert!(
        contested >= 1,
        "expected two satellites to want one station at the same time"
    );
}

// ---------------------------------------------------------------------------------------------
// Geometry sanity
// ---------------------------------------------------------------------------------------------

#[test]
fn inclination_decides_which_stations_a_satellite_can_reach() {
    // A 51.64 degree orbit never climbs 10 degrees above the horizon at 78 degrees north; a
    // near-polar orbit passes there on nearly every revolution.
    let survey = one_day();
    assert!(windows_for(&survey, EXAMPLE_A, SVALBARD).is_empty());
    let polar = windows_for(&survey, EXAMPLE_B, SVALBARD).len();
    assert!(
        (8..=16).contains(&polar),
        "expected about ten polar passes a day, got {polar}"
    );
    // A pair with no passes is an empty list, not a failure.
    assert!(survey.is_complete());
}

#[test]
fn low_orbit_passes_last_minutes() {
    // Above a 10 degree mask a low-orbit pass lasts from a few seconds (grazing) to roughly
    // ten minutes. Anything longer points at a unit or epoch mistake.
    for window in &one_day().windows {
        let seconds = window.duration().get();
        assert!(
            (1..=900).contains(&seconds),
            "implausible pass length {seconds} s"
        );
    }
}

#[test]
fn a_higher_mask_gives_fewer_and_shorter_windows() {
    let end = epoch() + Duration::days(1);
    let total = |mask_deg: f64| {
        let station = [station_with_mask(
            SEATTLE, "Seattle", 47.61, -122.33, mask_deg,
        )];
        let survey = contact_windows(&satellites(), &station, epoch(), end).unwrap();
        let seconds: u64 = survey
            .windows
            .iter()
            .map(|w| u64::from(w.duration().get()))
            .sum();
        (survey.windows.len(), seconds)
    };
    let (low_count, low_seconds) = total(5.0);
    let (mid_count, mid_seconds) = total(20.0);
    let (high_count, high_seconds) = total(45.0);
    assert!(low_count >= mid_count && mid_count >= high_count);
    assert!(low_seconds > mid_seconds && mid_seconds > high_seconds);
    assert!(
        high_count >= 1,
        "a day should include at least one high pass"
    );
}

// ---------------------------------------------------------------------------------------------
// Clipping at the search window
// ---------------------------------------------------------------------------------------------

#[test]
fn a_pass_in_progress_is_clipped_to_the_search_window() {
    // Take a long pass from the ordinary survey and search again from its middle to its middle
    // plus a minute: the satellite is up for the whole search, so the single window must span it
    // exactly.
    let survey = one_day();
    let long = windows_for(&survey, EXAMPLE_A, SEATTLE)
        .into_iter()
        .max_by_key(|w| w.duration().get())
        .unwrap();
    assert!(long.duration().get() > 300);

    let middle = utc_from_timestamp(long.aos()).unwrap() + Duration::seconds(150);
    let end = middle + Duration::seconds(60);
    let seattle = [station(SEATTLE, "Seattle", 47.61, -122.33)];
    let clipped = contact_windows(&satellites()[..1], &seattle, middle, end).unwrap();

    assert_eq!(clipped.windows.len(), 1);
    assert_eq!(clipped.windows[0].aos(), timestamp_ceil(middle));
    assert_eq!(clipped.windows[0].los(), timestamp_floor(end));
    assert_eq!(clipped.windows[0].duration().get(), 60);
}

// ---------------------------------------------------------------------------------------------
// Slew limits
// ---------------------------------------------------------------------------------------------

/// A station placed directly under EXAMPLE-A's ground track, so that one pass goes overhead.
fn station_under_the_track(limits: Option<SlewLimits>) -> GroundStation {
    let message = &Omm::parse_all(CATALOG_JSON).unwrap()[0];
    let propagator = Propagator::from_omm(message).unwrap();
    let below = propagator.subpoint(epoch() + Duration::hours(5)).unwrap();
    let station = station(
        StationId::new(9),
        "Under the track",
        below.latitude_deg,
        below.longitude_deg,
    );
    match limits {
        Some(limits) => station.with_slew_limits(limits).unwrap(),
        None => station,
    }
}

const ROTATOR: SlewLimits = SlewLimits {
    azimuth_deg_per_s: 6.0,
    elevation_deg_per_s: 2.5,
};

#[test]
fn an_overhead_pass_is_rejected_for_slew_and_the_rest_are_kept() {
    let end = epoch() + Duration::days(1);
    let one_satellite = &satellites()[..1];
    let free = contact_windows(
        one_satellite,
        &[station_under_the_track(None)],
        epoch(),
        end,
    )
    .unwrap();
    let limited = contact_windows(
        one_satellite,
        &[station_under_the_track(Some(ROTATOR))],
        epoch(),
        end,
    )
    .unwrap();

    // Without a limit nothing is rejected and one pass goes almost straight overhead.
    assert!(free.rejected.is_empty());
    let overhead = free
        .windows
        .iter()
        .max_by_key(|w| w.max_elevation().centidegrees())
        .unwrap();
    assert!(
        overhead.max_elevation().centidegrees() > 8_900,
        "{overhead:?}"
    );

    // With a 6 deg/s azimuth limit that pass, and only passes like it, is rejected.
    assert!(!limited.rejected.is_empty());
    for rejected in &limited.rejected {
        assert_eq!(rejected.reason, ReasonCode::SlewRateExceeded);
        assert_eq!(rejected.reason.id(), "MAAT-105");
        assert!(
            rejected.max_elevation_deg > 78.0,
            "rejected a {} deg pass",
            rejected.max_elevation_deg
        );
        assert!(
            rejected.detail.contains("deg/s in azimuth"),
            "{}",
            rejected.detail
        );
        assert!(
            rejected.detail.contains("the station allows 6.00"),
            "{}",
            rejected.detail
        );
    }
    assert!(limited.rejected.iter().any(|r| r.max_elevation_deg > 89.0));

    // Every pass is accounted for: the windows that survive plus the rejections are exactly
    // the windows found without a limit.
    assert_eq!(
        limited.windows.len() + limited.rejected.len(),
        free.windows.len()
    );
    for window in &limited.windows {
        assert!(free.windows.contains(window));
        assert!(
            window.max_elevation().centidegrees() < 8_200,
            "{window:?} should need a fast turn"
        );
    }
    assert!(limited.is_complete(), "a rejected pass is not a failure");
}

#[test]
fn rejected_passes_are_sorted_and_keep_their_unrounded_times() {
    // A very slow mount rejects almost everything, which gives a list long enough to check.
    let crawl = SlewLimits {
        azimuth_deg_per_s: 0.3,
        elevation_deg_per_s: 0.05,
    };
    let stations: Vec<GroundStation> = stations()
        .into_iter()
        .map(|s| s.with_slew_limits(crawl).unwrap())
        .collect();
    let survey = contact_windows(
        &satellites(),
        &stations,
        epoch(),
        epoch() + Duration::days(1),
    )
    .unwrap();

    assert!(survey.rejected.len() > 10);
    for pair in survey.rejected.windows(2) {
        let key = |r: &maat_ground::RejectedPass| (r.aos, r.station, r.satellite);
        assert!(key(&pair[0]) <= key(&pair[1]));
    }
    for rejected in &survey.rejected {
        assert!(rejected.aos < rejected.los);
        assert!(rejected.max_elevation_deg >= MASK_DEG);
    }
    // Predicted times are refined to the millisecond, so at least one carries a fraction that
    // a whole-second window would have lost.
    assert!(
        survey
            .rejected
            .iter()
            .any(|r| r.aos.timestamp_subsec_millis() != 0)
    );
    // Both axes are over the limit on a steep pass, and the detail says so.
    assert!(
        survey
            .rejected
            .iter()
            .any(|r| r.detail.contains("in azimuth") && r.detail.contains("in elevation"))
    );
}

#[test]
fn generous_slew_limits_reject_nothing() {
    let fast = SlewLimits {
        azimuth_deg_per_s: 180.0,
        elevation_deg_per_s: 180.0,
    };
    let stations: Vec<GroundStation> = stations()
        .into_iter()
        .map(|s| s.with_slew_limits(fast).unwrap())
        .collect();
    let survey = contact_windows(
        &satellites(),
        &stations,
        epoch(),
        epoch() + Duration::days(1),
    )
    .unwrap();
    assert!(survey.rejected.is_empty());
    assert_eq!(survey.windows, one_day().windows);
}

// ---------------------------------------------------------------------------------------------
// One satellite failing does not take the others down
// ---------------------------------------------------------------------------------------------

/// A real-looking element set that decays: a 190 km perigee with a very large drag term. SGP4
/// starts from it, then diverges about a day after the epoch.
const DECAYING_JSON: &str = r#"{"OBJECT_NAME":"FALLING","NORAD_CAT_ID":70000,
    "EPOCH":"2026-08-01T12:00:00","MEAN_MOTION":16.3,"ECCENTRICITY":0.001,"INCLINATION":97.5,
    "RA_OF_ASC_NODE":120.0,"ARG_OF_PERICENTER":45.0,"MEAN_ANOMALY":315.0,"BSTAR":0.5}"#;

#[test]
fn a_satellite_that_cannot_be_propagated_is_reported_and_the_rest_still_surveyed() {
    let mut fleet = satellites();
    fleet.push(Satellite::from_omm(&Omm::parse(DECAYING_JSON).unwrap()).unwrap());
    let end = epoch() + Duration::days(3);

    let survey = contact_windows(&fleet, &stations(), epoch(), end).unwrap();
    let healthy_only = contact_windows(&satellites(), &stations(), epoch(), end).unwrap();

    // The failure is explicit: one entry per station for the decaying satellite, none for the
    // healthy ones.
    assert!(!survey.is_complete());
    assert_eq!(survey.failures.len(), stations().len());
    for failure in &survey.failures {
        assert_eq!(failure.satellite, SatelliteId::new(70_000));
        assert!(
            matches!(
                &failure.error,
                GroundError::PassPrediction { satellite, station, .. }
                    if *satellite == failure.satellite && *station == failure.station
            ),
            "{:?}",
            failure.error
        );
        let message = failure.error.to_string();
        assert!(
            message.starts_with("pass prediction failed for satellite 70000"),
            "{message}"
        );
        assert!(message.contains("decayed"), "{message}");
    }

    // The healthy satellites are untouched by their neighbour's failure.
    let healthy_windows: Vec<_> = survey
        .windows
        .iter()
        .filter(|w| w.satellite() != SatelliteId::new(70_000))
        .copied()
        .collect();
    assert_eq!(healthy_windows, healthy_only.windows);
    // And no window is offered for the satellite whose orbit could not be followed.
    assert!(
        survey
            .windows
            .iter()
            .all(|w| w.satellite() != SatelliteId::new(70_000))
    );
}
