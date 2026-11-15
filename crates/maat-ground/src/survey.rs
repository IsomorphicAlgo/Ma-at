//! The window survey: every contact opportunity for N satellites over M ground stations.
//!
//! This is the input to scheduling. It answers "when *could* each satellite talk to each
//! station?" and leaves "which of those contacts *should* happen?" to the scheduler.
//!
//! Nothing is dropped silently. Every pass Ephemerust finds ends up in exactly one of three
//! lists in the [`WindowSurvey`]:
//!
//! - [`windows`](WindowSurvey::windows): usable contact opportunities, as flight
//!   [`ContactWindow`]s;
//! - [`rejected`](WindowSurvey::rejected): passes that exist but cannot be used, each with a
//!   `MAAT-xxx` reason code and the numbers behind it;
//! - [`failures`](WindowSurvey::failures): satellite/station pairs whose search could not be
//!   completed at all.
//!
//! ## Completeness
//!
//! The pass search is Ephemerust's. From 1.2.1 it advances in steps that provably cannot jump
//! over a pass, so every pass at least as long as its smallest step (one second in low Earth
//! orbit) is found. Ma'at checks that independently: its tests
//! compare a survey with a one-second brute-force scan of elevation and require every pass the
//! scan finds to appear, with matching edges (`tests/window_survey.rs`).
//!
//! ## Known limits
//!
//! - **The elevation mask is one number per station.** A horizon that varies with azimuth
//!   (terrain, buildings) is not modelled.
//! - **Slew limits are tested, not worked around.** A pass that needs a faster turn than the
//!   mount allows is rejected whole; the usable parts either side of the fast section are not
//!   offered as separate windows.
//! - **Accuracy is Ephemerust's and the element set's.** A search far from the element epoch
//!   returns confident-looking windows for an orbit that is no longer true.

use chrono::{DateTime, Duration, Utc};
use ephemerust::{AstroError, ObserverLocation, Pass, Propagator};
use maat_flight::{
    Azimuth, ContactWindow, Elevation, Pointing, ReasonCode, SatelliteId, StationId, Timestamp,
    WindowError,
};

use crate::error::GroundError;
use crate::satellite::{Satellite, check_unique_satellites};
use crate::station::{GroundStation, SlewLimits};
use crate::time::utc_from_timestamp;
use crate::window::window_from_pass;

/// The result of a window survey. See the [module docs](self).
#[derive(Debug, Default)]
#[non_exhaustive]
pub struct WindowSurvey {
    /// Usable contact opportunities, in order of AOS, then LOS, station and satellite.
    pub windows: Vec<ContactWindow>,
    /// Passes that exist but cannot be used, in order of AOS, then station and satellite.
    pub rejected: Vec<RejectedPass>,
    /// Satellite/station pairs whose search failed. Empty when the survey is complete.
    pub failures: Vec<SurveyFailure>,
}

impl WindowSurvey {
    /// `true` if every satellite/station pair was searched successfully.
    ///
    /// When this is `false`, the windows that were found are still valid, but the list is not
    /// the whole picture: check [`failures`](Self::failures) before scheduling from it.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.failures.is_empty()
    }
}

/// A pass that was found but is not offered as a contact window, and why.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct RejectedPass {
    /// The satellite.
    pub satellite: SatelliteId,
    /// The ground station.
    pub station: StationId,
    /// Acquisition of signal as predicted, before any rounding.
    pub aos: DateTime<Utc>,
    /// Loss of signal as predicted, before any rounding.
    pub los: DateTime<Utc>,
    /// The highest elevation of the pass, in degrees.
    pub max_elevation_deg: f64,
    /// The stable reason code.
    pub reason: ReasonCode,
    /// The numbers behind the decision, for a person to read.
    pub detail: String,
}

/// A satellite/station pair that could not be searched.
#[derive(Debug)]
#[non_exhaustive]
pub struct SurveyFailure {
    /// The satellite.
    pub satellite: SatelliteId,
    /// The ground station.
    pub station: StationId,
    /// What went wrong.
    pub error: GroundError,
}

/// Finds every contact opportunity for each satellite over each ground station between `start`
/// (inclusive) and `end` (exclusive).
///
/// Each station's own elevation mask and slew limits apply. A pass already in progress at
/// `start`, or still in progress at `end`, is clipped to the search window.
///
/// A problem with one satellite/station pair does not abort the survey: it is recorded in
/// [`WindowSurvey::failures`] and the remaining pairs are still searched. Only problems with the
/// request itself are returned as an error.
///
/// # Errors
///
/// - [`GroundError::InvalidSearchWindow`] if `end` is not after `start`.
/// - [`GroundError::DuplicateSatellite`] or [`GroundError::DuplicateStation`] if two inputs share
///   an identifier, since a contact window refers to its satellite and station by number alone.
///
/// # Example
///
/// ```
/// use chrono::Duration;
/// use maat_ground::flight::StationId;
/// use maat_ground::{GroundStation, StationSite, contact_windows, satellites_from_omm};
///
/// // Two satellites as OMM key-value notation. The second number does not fit in a TLE.
/// let satellites = satellites_from_omm(
///     "CCSDS_OMM_VERS = 2.0\n\
///      OBJECT_NAME = EXAMPLE-A\n\
///      NORAD_CAT_ID = 65001\n\
///      EPOCH = 2026-08-01T12:00:00\n\
///      MEAN_MOTION = 15.49\n\
///      ECCENTRICITY = 0.0002\n\
///      INCLINATION = 51.64\n\
///      RA_OF_ASC_NODE = 221.0\n\
///      ARG_OF_PERICENTER = 89.0\n\
///      MEAN_ANOMALY = 280.0\n\
///      BSTAR = 0.00003\n\
///      CCSDS_OMM_VERS = 2.0\n\
///      OBJECT_NAME = EXAMPLE-B\n\
///      NORAD_CAT_ID = 100123\n\
///      EPOCH = 2026-08-01T12:00:00\n\
///      MEAN_MOTION = 15.2\n\
///      ECCENTRICITY = 0.0012\n\
///      INCLINATION = 97.5\n\
///      RA_OF_ASC_NODE = 120.0\n\
///      ARG_OF_PERICENTER = 45.0\n\
///      MEAN_ANOMALY = 315.0\n\
///      BSTAR = 0.0001\n",
/// )?;
///
/// let site = |latitude_deg, longitude_deg| StationSite { latitude_deg, longitude_deg, altitude_m: 0.0 };
/// let stations = [
///     GroundStation::new(StationId::new(1), "Seattle", site(47.61, -122.33), 10.0)?,
///     GroundStation::new(StationId::new(2), "Svalbard", site(78.23, 15.41), 10.0)?,
/// ];
///
/// let start = satellites[0].epoch();
/// let survey = contact_windows(&satellites, &stations, start, start + Duration::days(1))?;
///
/// assert!(survey.is_complete());
/// assert!(!survey.windows.is_empty());
/// // Sorted by AOS, ready for a scheduler.
/// assert!(survey.windows.windows(2).all(|pair| pair[0].aos() <= pair[1].aos()));
/// # Ok::<(), maat_ground::GroundError>(())
/// ```
pub fn contact_windows(
    satellites: &[Satellite],
    stations: &[GroundStation],
    start: DateTime<Utc>,
    end: DateTime<Utc>,
) -> Result<WindowSurvey, GroundError> {
    if end <= start {
        return Err(GroundError::InvalidSearchWindow { start, end });
    }
    check_unique_satellites(satellites)?;
    check_unique_stations(stations)?;

    let mut survey = WindowSurvey::default();
    for satellite in satellites {
        for station in stations {
            if let Err(source) = survey_pair(&mut survey, satellite, station, start, end) {
                survey.failures.push(SurveyFailure {
                    satellite: satellite.id(),
                    station: station.id(),
                    error: GroundError::PassPrediction {
                        satellite: satellite.id(),
                        station: station.id(),
                        source,
                    },
                });
            }
        }
    }

    // A fixed order makes the output reproducible and independent of the order of the inputs.
    survey
        .windows
        .sort_by_key(|w| (w.aos(), w.los(), w.station(), w.satellite()));
    survey
        .rejected
        .sort_by_key(|r| (r.aos, r.station, r.satellite));
    Ok(survey)
}

/// Searches one satellite over one station and files every pass into `survey`.
///
/// An `Err` means the search itself failed.
fn survey_pair(
    survey: &mut WindowSurvey,
    satellite: &Satellite,
    station: &GroundStation,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
) -> Result<(), AstroError> {
    let passes = satellite.propagator().predict_passes(
        station.observer(),
        start,
        end,
        station.min_elevation_deg(),
    )?;
    for pass in &passes {
        file_pass(survey, satellite, station, pass)?;
    }
    Ok(())
}

/// Decides what one predicted pass is and puts it in the matching list of `survey`: a usable
/// window, a rejected pass with its reason, or a failure. A pass never vanishes.
fn file_pass(
    survey: &mut WindowSurvey,
    satellite: &Satellite,
    station: &GroundStation,
    pass: &Pass,
) -> Result<(), AstroError> {
    let reject = |reason: ReasonCode, detail: String| RejectedPass {
        satellite: satellite.id(),
        station: station.id(),
        aos: pass.aos,
        los: pass.los,
        max_elevation_deg: pass.max_elevation_deg,
        reason,
        detail,
    };

    if let Some(limits) = station.slew_limits() {
        let demand = slew_demand(
            satellite.propagator(),
            station.observer(),
            pass.aos,
            pass.los,
        )?;
        if let Some(detail) = demand.exceeds(limits) {
            survey
                .rejected
                .push(reject(ReasonCode::SlewRateExceeded, detail));
            return Ok(());
        }
    }

    match window_from_pass(satellite.id(), station.id(), pass) {
        // Record where the antenna points at each end of the window, so that a scheduler can
        // work out how long the station needs to turn from one pass to the next.
        Ok(window) => match pointing_at_ends(satellite.propagator(), station.observer(), &window) {
            Ok(Ok((aos, los))) => survey.windows.push(window.with_pointing(aos, los)),
            Ok(Err(error)) => survey.failures.push(SurveyFailure {
                satellite: satellite.id(),
                station: station.id(),
                error,
            }),
            Err(source) => return Err(source),
        },
        // Rounding inward left nothing: the pass is shorter than the one-second resolution of a
        // contact window. That is a pass too short to use, not malformed input.
        Err(GroundError::Window(WindowError::LosNotAfterAos { .. })) => {
            survey.rejected.push(reject(
                ReasonCode::BelowMinimumDuration,
                format!(
                    "the pass lasts {:.3} s, and nothing is left of it after rounding to whole \
                     seconds",
                    seconds_between(pass.aos, pass.los)
                ),
            ));
        }
        Err(error) => survey.failures.push(SurveyFailure {
            satellite: satellite.id(),
            station: station.id(),
            error,
        }),
    }
    Ok(())
}

/// Where the antenna points at a window's AOS and LOS.
///
/// The look angles are taken at the window's own whole-second times, not at the pass's
/// sub-second ones, so that they describe the window the scheduler will actually use.
///
/// The outer error is a propagation failure; the inner one is a time or angle that cannot be
/// represented, which is reported as a failure for the pair and does not stop the survey.
fn pointing_at_ends(
    propagator: &Propagator,
    observer: ObserverLocation,
    window: &ContactWindow,
) -> Result<Result<(Pointing, Pointing), GroundError>, AstroError> {
    let at = |time: Timestamp| -> Result<Result<Pointing, GroundError>, AstroError> {
        let utc = match utc_from_timestamp(time) {
            Ok(utc) => utc,
            Err(error) => return Ok(Err(error)),
        };
        let look = propagator.look_angles(utc, observer)?;
        Ok(pointing_from_degrees(look.azimuth_deg, look.elevation_deg))
    };
    let (aos, los) = (at(window.aos())?, at(window.los())?);
    Ok(aos.and_then(|aos| los.map(|los| (aos, los))))
}

/// Converts look angles in degrees to a flight [`Pointing`].
///
/// Azimuth is rounded to the nearest hundredth of a degree and wrapped into `[0, 360)`.
/// Elevation is rounded down. An elevation a hair below the horizon, which rounding at the very
/// edge of a pass can produce, is taken as the horizon; anything further out is an error.
fn pointing_from_degrees(azimuth_deg: f64, elevation_deg: f64) -> Result<Pointing, GroundError> {
    if !azimuth_deg.is_finite() || !(-0.5..=90.0).contains(&elevation_deg) {
        return Err(GroundError::InvalidElevation {
            degrees: elevation_deg,
        });
    }
    // `rem_euclid` puts any finite angle in [0, 360); rounding can then land exactly on 36000,
    // which is north again.
    let azimuth = (azimuth_deg.rem_euclid(360.0) * 100.0).round() as u32 % 36_000;
    let elevation = (elevation_deg.max(0.0) * 100.0).floor() as u16;
    Ok(Pointing {
        azimuth: Azimuth::from_centidegrees(azimuth as u16)?,
        elevation: Elevation::from_centidegrees(elevation)?,
    })
}

/// Fails if two stations share an identifier.
fn check_unique_stations(stations: &[GroundStation]) -> Result<(), GroundError> {
    for (index, later) in stations.iter().enumerate() {
        if let Some(earlier) = stations[..index].iter().find(|s| s.id() == later.id()) {
            return Err(GroundError::DuplicateStation {
                id: later.id(),
                first: earlier.name().to_owned(),
                second: later.name().to_owned(),
            });
        }
    }
    Ok(())
}

fn seconds_between(from: DateTime<Utc>, to: DateTime<Utc>) -> f64 {
    (to - from).num_milliseconds() as f64 / 1000.0
}

// ---------------------------------------------------------------------------------------------
// Slew demand
// ---------------------------------------------------------------------------------------------

/// The step between look-angle samples when measuring slew rate, for an ordinary pass.
const SLEW_STEP_MS: i64 = 1_000;

/// The most samples taken along one pass. A low-orbit pass is under 1000 s and is sampled every
/// second. A pass longer than this many seconds (a medium or geostationary orbit, where the
/// satellite crawls across the sky) is sampled at a proportionally longer step.
const MAX_SLEW_SAMPLES: i64 = 2_000;

/// The fastest turn a pass asks of an antenna mount, about each axis.
///
/// Each rate is the largest *average* rate over any one sampling step (one second for an
/// ordinary pass). That is the figure a mount has to meet to stay on target, and it is slightly
/// below the instantaneous peak.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct SlewDemand {
    pub(crate) azimuth_deg_per_s: f64,
    pub(crate) elevation_deg_per_s: f64,
}

impl SlewDemand {
    /// Describes how the demand exceeds `limits`, or returns `None` if the mount can keep up.
    fn exceeds(&self, limits: SlewLimits) -> Option<String> {
        let azimuth = self.azimuth_deg_per_s > limits.azimuth_deg_per_s;
        let elevation = self.elevation_deg_per_s > limits.elevation_deg_per_s;
        let describe = |axis: &str, needed: f64, allowed: f64| {
            format!("{needed:.2} deg/s in {axis} (the station allows {allowed:.2})")
        };
        let azimuth_text = describe("azimuth", self.azimuth_deg_per_s, limits.azimuth_deg_per_s);
        let elevation_text = describe(
            "elevation",
            self.elevation_deg_per_s,
            limits.elevation_deg_per_s,
        );
        match (azimuth, elevation) {
            (false, false) => None,
            (true, false) => Some(format!("tracking needs {azimuth_text}")),
            (false, true) => Some(format!("tracking needs {elevation_text}")),
            (true, true) => Some(format!(
                "tracking needs {azimuth_text} and {elevation_text}"
            )),
        }
    }
}

/// Measures the slew demand of a pass by sampling its look angles from AOS to LOS.
fn slew_demand(
    propagator: &Propagator,
    observer: ObserverLocation,
    aos: DateTime<Utc>,
    los: DateTime<Utc>,
) -> Result<SlewDemand, AstroError> {
    slew_demand_with_step(propagator, observer, aos, los, SLEW_STEP_MS)
}

/// [`slew_demand`] with the sampling step exposed, so tests can check that the result does not
/// depend on it.
fn slew_demand_with_step(
    propagator: &Propagator,
    observer: ObserverLocation,
    aos: DateTime<Utc>,
    los: DateTime<Utc>,
    step_ms: i64,
) -> Result<SlewDemand, AstroError> {
    let mut demand = SlewDemand {
        azimuth_deg_per_s: 0.0,
        elevation_deg_per_s: 0.0,
    };
    let total_ms = (los - aos).num_milliseconds();
    if total_ms <= 0 || step_ms <= 0 {
        return Ok(demand);
    }

    // Round the number of steps up so no step is longer than asked, then cap it.
    let steps = ((total_ms + step_ms - 1) / step_ms).clamp(1, MAX_SLEW_SAMPLES);
    let mut previous_time = aos;
    let mut previous = propagator.look_angles(aos, observer)?;
    for index in 1..=steps {
        // Spread the samples evenly, landing exactly on LOS at the end.
        let time = aos + Duration::milliseconds(total_ms * index / steps);
        let current = propagator.look_angles(time, observer)?;
        let dt = seconds_between(previous_time, time);
        if dt > 0.0 {
            let azimuth_rate =
                azimuth_change_deg(previous.azimuth_deg, current.azimuth_deg).abs() / dt;
            let elevation_rate = (current.elevation_deg - previous.elevation_deg).abs() / dt;
            demand.azimuth_deg_per_s = demand.azimuth_deg_per_s.max(azimuth_rate);
            demand.elevation_deg_per_s = demand.elevation_deg_per_s.max(elevation_rate);
        }
        previous_time = time;
        previous = current;
    }
    Ok(demand)
}

/// The signed change in azimuth from `from` to `to`, taking the short way round.
///
/// Azimuth wraps at north: going from 359 degrees to 1 degree is a turn of +2, not -358.
/// The result is in `[-180, 180)`.
fn azimuth_change_deg(from: f64, to: f64) -> f64 {
    (to - from + 180.0).rem_euclid(360.0) - 180.0
}

/// The slew demand of a predicted pass, for tests elsewhere in the crate.
#[cfg(test)]
pub(crate) fn slew_demand_of(
    satellite: &Satellite,
    station: &GroundStation,
    pass: &Pass,
) -> SlewDemand {
    slew_demand(
        satellite.propagator(),
        station.observer(),
        pass.aos,
        pass.los,
    )
    .expect("look angles are available throughout a predicted pass")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::station::StationSite;
    use ephemerust::Tle;

    const ISS_TLE: &str = "ISS (ZARYA)\n\
        1 25544U 98067A   20194.88612269 -.00002218  00000-0 -31515-4 0  9992\n\
        2 25544  51.6461 221.2784 0001413  89.1723 280.4612 15.49507896236008";

    fn iss() -> Satellite {
        Satellite::from_tle(&Tle::parse(ISS_TLE).unwrap()).unwrap()
    }

    fn station(id: u16, latitude_deg: f64, longitude_deg: f64) -> GroundStation {
        GroundStation::new(
            StationId::new(id),
            format!("Station {id}"),
            StationSite {
                latitude_deg,
                longitude_deg,
                altitude_m: 0.0,
            },
            10.0,
        )
        .unwrap()
    }

    // --- Pointing ------------------------------------------------------------------------------

    #[test]
    fn pointing_rounds_azimuth_to_the_nearest_hundredth_and_elevation_down() {
        let pointing = pointing_from_degrees(123.456, 10.019).unwrap();
        assert_eq!(pointing.azimuth.centidegrees(), 12_346);
        assert_eq!(pointing.elevation.centidegrees(), 1_001);
        let pointing = pointing_from_degrees(123.454, 10.0).unwrap();
        assert_eq!(pointing.azimuth.centidegrees(), 12_345);
        assert_eq!(pointing.elevation.centidegrees(), 1_000);
    }

    #[test]
    fn azimuth_wraps_into_one_turn() {
        let azimuth = |degrees: f64| {
            pointing_from_degrees(degrees, 10.0)
                .unwrap()
                .azimuth
                .centidegrees()
        };
        assert_eq!(azimuth(0.0), 0);
        assert_eq!(azimuth(359.99), 35_999);
        // Rounds up to a full turn, which is north again.
        assert_eq!(azimuth(359.996), 0);
        assert_eq!(azimuth(360.0), 0);
        assert_eq!(azimuth(-90.0), 27_000);
        assert_eq!(azimuth(725.5), 550);
    }

    #[test]
    fn an_elevation_a_hair_below_the_horizon_is_the_horizon() {
        assert_eq!(
            pointing_from_degrees(10.0, -0.2)
                .unwrap()
                .elevation
                .centidegrees(),
            0
        );
        assert_eq!(
            pointing_from_degrees(10.0, 90.0)
                .unwrap()
                .elevation
                .centidegrees(),
            9_000
        );
    }

    #[test]
    fn angles_that_are_not_a_direction_in_the_sky_are_refused() {
        for (azimuth, elevation) in [
            (10.0, -0.6),
            (10.0, 90.01),
            (10.0, f64::NAN),
            (f64::NAN, 10.0),
            (f64::INFINITY, 10.0),
        ] {
            assert!(
                pointing_from_degrees(azimuth, elevation).is_err(),
                "{azimuth} {elevation}"
            );
        }
    }

    #[test]
    fn every_surveyed_window_records_its_pointing() {
        let (satellite, station, passes) = seattle_passes();
        let start = satellite.epoch();
        let survey =
            contact_windows(&[satellite], &[station], start, start + Duration::days(3)).unwrap();
        assert_eq!(survey.windows.len(), passes.len());
        for window in &survey.windows {
            assert!(window.aos_pointing().is_some() && window.los_pointing().is_some());
        }
    }

    /// Three days of ISS passes over Seattle, straight from Ephemerust: 16 passes peaking from
    /// 10 to 86 degrees, so both grazing and near-overhead geometry are covered.
    fn seattle_passes() -> (Satellite, GroundStation, Vec<Pass>) {
        let satellite = iss();
        let seattle = station(1, 47.6062, -122.3321);
        let start = satellite.epoch();
        let passes = satellite
            .propagator()
            .predict_passes(seattle.observer(), start, start + Duration::days(3), 10.0)
            .unwrap();
        assert_eq!(
            passes.len(),
            16,
            "the reference set has 16 passes in three days"
        );
        assert!(passes.iter().any(|p| p.max_elevation_deg > 80.0));
        (satellite, seattle, passes)
    }

    // --- azimuth_change_deg --------------------------------------------------------------------

    #[test]
    fn azimuth_change_takes_the_short_way_through_north() {
        assert_eq!(azimuth_change_deg(359.0, 1.0), 2.0);
        assert_eq!(azimuth_change_deg(1.0, 359.0), -2.0);
        assert_eq!(azimuth_change_deg(350.0, 10.0), 20.0);
    }

    #[test]
    fn azimuth_change_is_plain_subtraction_away_from_north() {
        assert_eq!(azimuth_change_deg(90.0, 100.0), 10.0);
        assert_eq!(azimuth_change_deg(100.0, 90.0), -10.0);
        assert_eq!(azimuth_change_deg(45.0, 45.0), 0.0);
    }

    #[test]
    fn azimuth_change_stays_within_half_a_turn() {
        // Every pair on a 7.5 degree grid: the answer is always in [-180, 180) and always
        // lands back on `to` when added to `from`.
        let grid: Vec<f64> = (0..48).map(|i| f64::from(i) * 7.5).collect();
        for &from in &grid {
            for &to in &grid {
                let change = azimuth_change_deg(from, to);
                assert!(
                    (-180.0..180.0).contains(&change),
                    "{from} -> {to} = {change}"
                );
                let landed = (from + change).rem_euclid(360.0);
                assert!((landed - to).abs() < 1e-9, "{from} + {change} != {to}");
            }
        }
    }

    // --- SlewDemand::exceeds -------------------------------------------------------------------

    const LIMITS: SlewLimits = SlewLimits {
        azimuth_deg_per_s: 6.0,
        elevation_deg_per_s: 2.5,
    };

    fn demand(azimuth: f64, elevation: f64) -> SlewDemand {
        SlewDemand {
            azimuth_deg_per_s: azimuth,
            elevation_deg_per_s: elevation,
        }
    }

    #[test]
    fn demand_within_both_limits_is_accepted() {
        assert_eq!(demand(5.99, 2.49).exceeds(LIMITS), None);
        assert_eq!(demand(0.0, 0.0).exceeds(LIMITS), None);
    }

    #[test]
    fn demand_exactly_at_the_limit_is_accepted() {
        // The limit is the fastest the mount can turn, so needing exactly that is achievable.
        assert_eq!(demand(6.0, 2.5).exceeds(LIMITS), None);
    }

    #[test]
    fn demand_over_a_limit_names_the_axis_and_both_numbers() {
        let azimuth = demand(12.345, 1.0).exceeds(LIMITS).unwrap();
        assert_eq!(
            azimuth,
            "tracking needs 12.35 deg/s in azimuth (the station allows 6.00)"
        );

        let elevation = demand(1.0, 3.0).exceeds(LIMITS).unwrap();
        assert_eq!(
            elevation,
            "tracking needs 3.00 deg/s in elevation (the station allows 2.50)"
        );

        let both = demand(9.0, 3.0).exceeds(LIMITS).unwrap();
        assert!(
            both.contains("in azimuth") && both.contains("in elevation"),
            "{both}"
        );
    }

    // --- slew_demand on real passes ------------------------------------------------------------

    #[test]
    fn slew_demand_is_zero_for_an_empty_interval() {
        let (satellite, seattle, _) = seattle_passes();
        let t = satellite.epoch();
        let none = slew_demand(satellite.propagator(), seattle.observer(), t, t).unwrap();
        assert_eq!(none, demand(0.0, 0.0));
    }

    #[test]
    fn iss_elevation_rate_stays_under_the_orbital_angular_rate() {
        // Physics bound. Seen from directly below, the ISS crosses the sky at about v / h:
        // 7.66 km/s at about 420 km is 0.0182 rad/s, or 1.05 deg/s. That is the fastest the
        // line of sight can turn, so no pass can ask for more elevation rate than that.
        // Bound: 1.2 deg/s. Measured worst case on these passes: 0.91 deg/s, on the 86 degree
        // pass.
        let (satellite, seattle, passes) = seattle_passes();
        for pass in &passes {
            let d = slew_demand_of(&satellite, &seattle, pass);
            assert!(
                d.elevation_deg_per_s > 0.0 && d.elevation_deg_per_s < 1.2,
                "elevation rate {} deg/s on a pass peaking at {} deg",
                d.elevation_deg_per_s,
                pass.max_elevation_deg
            );
        }
    }

    #[test]
    fn azimuth_rate_grows_with_peak_elevation() {
        // The higher a pass climbs, the faster the antenna must swing round at the top. Any
        // pass that peaks at least 2 degrees higher than another must need a faster azimuth
        // turn. (Passes with nearly equal peaks are left out: their small differences in range
        // and direction can outweigh a fraction of a degree of elevation.)
        let (satellite, seattle, passes) = seattle_passes();
        let rated: Vec<(f64, f64)> = passes
            .iter()
            .map(|pass| {
                let rate = slew_demand_of(&satellite, &seattle, pass).azimuth_deg_per_s;
                (pass.max_elevation_deg, rate)
            })
            .collect();
        let mut compared = 0;
        for &(low_peak, low_rate) in &rated {
            for &(high_peak, high_rate) in &rated {
                if high_peak >= low_peak + 2.0 {
                    assert!(
                        high_rate > low_rate,
                        "a {high_peak:.1} degree pass needs {high_rate:.3} deg/s, but a lower \
                         {low_peak:.1} degree pass needs {low_rate:.3} deg/s"
                    );
                    compared += 1;
                }
            }
        }
        assert!(compared > 50, "only {compared} pairs were compared");
    }

    #[test]
    fn a_near_overhead_iss_pass_outruns_an_amateur_rotator() {
        // The figure quoted in the `SlewLimits` docs: about 6 deg/s is a common azimuth limit,
        // and the ISS exceeds it only on passes above roughly 80 degrees.
        let (satellite, seattle, passes) = seattle_passes();
        for pass in &passes {
            let rate = slew_demand_of(&satellite, &seattle, pass).azimuth_deg_per_s;
            if pass.max_elevation_deg > 82.0 {
                assert!(
                    rate > 6.0,
                    "peak {} needs only {rate}",
                    pass.max_elevation_deg
                );
            }
            if pass.max_elevation_deg < 78.0 {
                assert!(rate < 6.0, "peak {} needs {rate}", pass.max_elevation_deg);
            }
        }
    }

    #[test]
    fn azimuth_rate_matches_the_overhead_geometry_formula() {
        // Cross-check against a closed form. At culmination the satellite moves at right angles
        // to the line of sight, so the line of sight turns at v_rel / range, and the azimuth
        // rate is that divided by cos(elevation): the same sideways motion sweeps more azimuth
        // the closer the target is to the zenith.
        //
        //   azimuth rate at culmination  =  v_rel / (range * cos(elevation))
        //
        // v_rel is the satellite's speed relative to the rotating ground, about 7.4 km/s for
        // the ISS (7.66 km/s orbital, less the ground's own eastward motion).
        //
        // Tolerance 3 %: v_rel varies a little with pass direction, and the sampled rate is a
        // one-second average, which reads low when the peak is sharp. Measured on these passes:
        // within 0.8 % for peaks below 80 degrees, 2.2 % low on the 86 degree pass.
        let (satellite, seattle, passes) = seattle_passes();
        let mut checked = 0;
        for pass in passes.iter().filter(|p| p.max_elevation_deg > 30.0) {
            let top = satellite
                .propagator()
                .look_angles(pass.culmination, seattle.observer())
                .unwrap();
            let v_rel_km_s = 7.4;
            let expected =
                (v_rel_km_s / (top.range_km * top.elevation_deg.to_radians().cos())).to_degrees();
            let measured = slew_demand_of(&satellite, &seattle, pass).azimuth_deg_per_s;
            let error = (measured - expected).abs() / expected;
            assert!(
                error < 0.03,
                "peak {:.1} deg: measured {measured:.3} deg/s, formula {expected:.3} deg/s",
                pass.max_elevation_deg
            );
            checked += 1;
        }
        assert!(
            checked >= 6,
            "only {checked} passes above 30 degrees were checked"
        );
    }

    #[test]
    fn slew_demand_does_not_depend_on_the_sampling_step() {
        // Numerical cross-check: sampling ten times finer must give nearly the same answer,
        // or the one-second step would be too coarse to trust.
        //
        // Azimuth: within 0.1 % for passes peaking below 80 degrees (measured 0.05 %) and
        // within 3 % above, where the peak is only a few seconds wide (measured 1.6 % at 86
        // degrees). Elevation: within 2 % (measured 1.1 %, on a grazing pass whose elevation
        // rate is itself only 0.02 deg/s).
        let (satellite, seattle, passes) = seattle_passes();
        for pass in &passes {
            let at = |step_ms| {
                slew_demand_with_step(
                    satellite.propagator(),
                    seattle.observer(),
                    pass.aos,
                    pass.los,
                    step_ms,
                )
                .unwrap()
            };
            let (coarse, fine) = (at(1_000), at(100));
            // A one-second average can never exceed the peak of finer averages.
            assert!(coarse.azimuth_deg_per_s <= fine.azimuth_deg_per_s + 1e-9);
            assert!(coarse.elevation_deg_per_s <= fine.elevation_deg_per_s + 1e-9);
            let azimuth_gap =
                (fine.azimuth_deg_per_s - coarse.azimuth_deg_per_s) / fine.azimuth_deg_per_s;
            let elevation_gap =
                (fine.elevation_deg_per_s - coarse.elevation_deg_per_s) / fine.elevation_deg_per_s;
            let azimuth_tolerance = if pass.max_elevation_deg < 80.0 {
                0.001
            } else {
                0.03
            };
            assert!(
                azimuth_gap < azimuth_tolerance,
                "azimuth gap {azimuth_gap} at peak {}",
                pass.max_elevation_deg
            );
            assert!(elevation_gap < 0.02, "elevation gap {elevation_gap}");
        }
    }

    #[test]
    fn a_long_pass_is_sampled_no_more_than_the_cap() {
        // Asking for a 1 ms step over a pass of several minutes would be hundreds of thousands
        // of samples; the cap must hold it to 2000 and still give a sensible answer.
        let (satellite, seattle, passes) = seattle_passes();
        let longest = passes
            .iter()
            .filter(|p| p.max_elevation_deg < 70.0)
            .max_by_key(|p| (p.los - p.aos).num_milliseconds())
            .unwrap();
        let capped = slew_demand_with_step(
            satellite.propagator(),
            seattle.observer(),
            longest.aos,
            longest.los,
            1,
        )
        .unwrap();
        let normal = slew_demand_of(&satellite, &seattle, longest);
        let gap =
            (capped.azimuth_deg_per_s - normal.azimuth_deg_per_s).abs() / normal.azimuth_deg_per_s;
        assert!(gap < 0.01, "capped {capped:?} vs normal {normal:?}");
    }

    // --- file_pass -----------------------------------------------------------------------------

    #[test]
    fn an_ordinary_pass_becomes_a_window() {
        let (satellite, seattle, passes) = seattle_passes();
        let mut survey = WindowSurvey::default();
        file_pass(&mut survey, &satellite, &seattle, &passes[0]).unwrap();
        assert_eq!(survey.windows.len(), 1);
        assert!(survey.rejected.is_empty() && survey.failures.is_empty());
        assert_eq!(survey.windows[0].satellite(), satellite.id());
        assert_eq!(survey.windows[0].station(), seattle.id());
    }

    #[test]
    fn a_sub_second_pass_is_rejected_as_too_short_not_lost() {
        // `Pass` has public fields, so a real pass can be shrunk to 0.4 s between two whole
        // seconds. Rounding inward leaves nothing, and the survey must say so with MAAT-103.
        let (satellite, seattle, passes) = seattle_passes();
        let mut sliver = passes[0];
        let whole_second = DateTime::from_timestamp(sliver.aos.timestamp(), 0).unwrap();
        sliver.aos = whole_second + Duration::milliseconds(300);
        sliver.los = whole_second + Duration::milliseconds(700);

        let mut survey = WindowSurvey::default();
        file_pass(&mut survey, &satellite, &seattle, &sliver).unwrap();
        assert!(survey.windows.is_empty() && survey.failures.is_empty());
        assert_eq!(survey.rejected.len(), 1);
        let rejected = &survey.rejected[0];
        assert_eq!(rejected.reason, ReasonCode::BelowMinimumDuration);
        assert_eq!(rejected.reason.id(), "MAAT-103");
        assert_eq!((rejected.aos, rejected.los), (sliver.aos, sliver.los));
        assert!(rejected.detail.contains("0.400 s"), "{}", rejected.detail);
    }

    #[test]
    fn a_pass_with_an_unusable_elevation_is_reported_as_a_failure() {
        // Should Ephemerust ever hand back a pass with a nonsensical peak, it must surface as a
        // failure for that pair, never as a silently missing window.
        let (satellite, seattle, passes) = seattle_passes();
        let mut broken = passes[0];
        broken.max_elevation_deg = f64::NAN;

        let mut survey = WindowSurvey::default();
        file_pass(&mut survey, &satellite, &seattle, &broken).unwrap();
        assert!(survey.windows.is_empty() && survey.rejected.is_empty());
        assert_eq!(survey.failures.len(), 1);
        assert!(!survey.is_complete());
        assert!(matches!(
            survey.failures[0].error,
            GroundError::InvalidElevation { .. }
        ));
    }

    #[test]
    fn a_pass_over_the_slew_limit_is_rejected_with_the_numbers() {
        let (satellite, seattle, passes) = seattle_passes();
        let slow = seattle
            .clone()
            .with_slew_limits(SlewLimits {
                azimuth_deg_per_s: 6.0,
                elevation_deg_per_s: 2.5,
            })
            .unwrap();
        let overhead = passes
            .iter()
            .find(|p| p.max_elevation_deg > 82.0)
            .expect("the three-day set includes a near-overhead pass");
        let needed = slew_demand_of(&satellite, &slow, overhead).azimuth_deg_per_s;

        let mut survey = WindowSurvey::default();
        file_pass(&mut survey, &satellite, &slow, overhead).unwrap();
        assert!(survey.windows.is_empty());
        assert_eq!(survey.rejected.len(), 1);
        let rejected = &survey.rejected[0];
        assert_eq!(rejected.reason, ReasonCode::SlewRateExceeded);
        assert_eq!(
            rejected.detail,
            format!("tracking needs {needed:.2} deg/s in azimuth (the station allows 6.00)")
        );
        assert_eq!(rejected.max_elevation_deg, overhead.max_elevation_deg);
    }

    // --- request validation --------------------------------------------------------------------

    #[test]
    fn a_search_window_must_end_after_it_starts() {
        let satellites = [iss()];
        let stations = [station(1, 47.6, -122.3)];
        let t = satellites[0].epoch();
        for end in [t, t - Duration::seconds(1)] {
            let error = contact_windows(&satellites, &stations, t, end).unwrap_err();
            assert!(
                matches!(error, GroundError::InvalidSearchWindow { start, end: e } if start == t && e == end),
                "{error:?}"
            );
        }
    }

    #[test]
    fn duplicate_station_identifiers_are_refused() {
        let satellites = [iss()];
        let stations = [station(1, 47.6, -122.3), station(1, 10.0, 20.0)];
        let t = satellites[0].epoch();
        let error = contact_windows(&satellites, &stations, t, t + Duration::hours(1)).unwrap_err();
        assert!(
            matches!(&error, GroundError::DuplicateStation { id, .. } if id.get() == 1),
            "{error:?}"
        );
    }

    #[test]
    fn duplicate_satellite_identifiers_are_refused() {
        let satellites = [iss(), iss()];
        let stations = [station(1, 47.6, -122.3)];
        let t = satellites[0].epoch();
        let error = contact_windows(&satellites, &stations, t, t + Duration::hours(1)).unwrap_err();
        assert!(
            matches!(&error, GroundError::DuplicateSatellite { id, .. } if id.get() == 25544),
            "{error:?}"
        );
    }

    #[test]
    fn no_satellites_or_no_stations_gives_an_empty_complete_survey() {
        let t = iss().epoch();
        let end = t + Duration::hours(6);
        for (satellites, stations) in [
            (vec![], vec![station(1, 47.6, -122.3)]),
            (vec![iss()], vec![]),
            (vec![], vec![]),
        ] {
            let survey = contact_windows(&satellites, &stations, t, end).unwrap();
            assert!(survey.windows.is_empty() && survey.rejected.is_empty());
            assert!(survey.is_complete());
        }
    }
}
