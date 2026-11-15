#![forbid(unsafe_code)]
#![warn(missing_docs)]
//! # maat-ground
//!
//! The **human layer** of the Ma'at contact scheduler. It depends on [`maat_flight`] (re-exported
//! as [`flight`]) and adds everything that is convenient on the ground but has no place onboard:
//! calendar time, floating-point angles, heap-allocated names, rich errors, and pass prediction
//! through [Ephemerust](https://crates.io/crates/ephemerust).
//!
//! The dependency points one way only. This crate knows about the flight crate; the flight crate
//! cannot know about this one, and the compiler enforces it.
//!
//! ## What it does
//!
//! 1. **Loads satellites** from OMM text in any of its four formats ([`satellites_from_omm`]) or
//!    from a TLE ([`Satellite::from_tle`]).
//! 2. **Describes ground stations**: a validated site, an elevation mask, optional antenna
//!    slew limits and a fixed turnaround time ([`GroundStation`]).
//! 3. **Surveys contact windows** for every satellite over every station
//!    ([`contact_windows`]). The result is a sorted list of flight
//!    [`ContactWindow`](flight::ContactWindow)s, each recording where the antenna points at its
//!    start and end, plus every pass that was found but cannot be used, each with a `MAAT-xxx`
//!    reason.
//! 4. **Plans contacts** ([`plan_contacts`]) with the greedy scheduler from the flight crate or
//!    the [`optimal`] one, which is an integer program and runs on the ground only. Both leave
//!    a station its turnaround time between passes and, if it has slew limits, the time to turn
//!    its antenna from one pass to the next. [`plan_contacts_with`] sets the optimal
//!    scheduler's time limit, and can keep a floor under every satellite.
//! 5. **Exports** the scheduled passes ([`export`]) as SatNOGS Network observations or as one
//!    station's schedule.
//! 5. **Explains the plan.** A [`ContactPlan`] has one entry for every pass that was found, with
//!    its reason code and a sentence of detail, and renders as text
//!    ([`ContactPlan::to_text`]) or JSON ([`ContactPlan::to_json`]). [`guidance`] says what each
//!    code means and what would change the outcome.
//!
//! ## The boundary, and how values cross it
//!
//! Ephemerust reports a pass with sub-second times and a floating-point elevation. The flight
//! layer keeps whole seconds and hundredths of a degree. [`window_from_pass`] converts between
//! them and always rounds **inward**:
//!
//! - AOS rounds up to the next whole second, LOS rounds down, so the integer window lies inside
//!   the real pass and never promises contact time that does not exist.
//! - Maximum elevation rounds down, so the window never claims a better pass than predicted.
//!
//! ## Example
//!
//! ```
//! use chrono::Duration;
//! use ephemerust::Tle;
//! use maat_ground::flight::{ReasonCode, StationId};
//! use maat_ground::{GroundStation, Satellite, SlewLimits, StationSite, contact_windows};
//!
//! // A public reference element set for the ISS.
//! let iss = Satellite::from_tle(&Tle::parse(
//!     "ISS (ZARYA)\n\
//!      1 25544U 98067A   20194.88612269 -.00002218  00000-0 -31515-4 0  9992\n\
//!      2 25544  51.6461 221.2784 0001413  89.1723 280.4612 15.49507896236008",
//! )?)?;
//!
//! // A station with a 10 degree mask and a mount that turns slowly in azimuth.
//! let station = GroundStation::new(
//!     StationId::new(1),
//!     "Seattle",
//!     StationSite { latitude_deg: 47.6062, longitude_deg: -122.3321, altitude_m: 50.0 },
//!     10.0,
//! )?
//! .with_slew_limits(SlewLimits { azimuth_deg_per_s: 1.0, elevation_deg_per_s: 2.5 })?;
//!
//! let start = iss.epoch();
//! let survey = contact_windows(&[iss], &[station], start, start + Duration::days(1))?;
//!
//! // Low passes are usable; the high ones need a faster azimuth turn and say so.
//! assert!(!survey.windows.is_empty());
//! assert!(!survey.rejected.is_empty());
//! for pass in &survey.rejected {
//!     assert_eq!(pass.reason, ReasonCode::SlewRateExceeded);
//!     assert!(pass.detail.contains("in azimuth"));
//! }
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

pub mod error;
pub mod explain;
pub mod export;
pub mod optimal;
pub mod plan;
pub mod satellite;
pub mod station;
pub mod survey;
pub mod time;
pub mod window;

#[cfg(test)]
mod testkit;

/// The vehicle layer, re-exported so that ground code needs only one dependency.
pub use maat_flight as flight;

pub use error::GroundError;
pub use explain::{guidance, parse_reason_code};
pub use export::{
    STATION_SCHEDULE_SCHEMA, SatnogsIds, SatnogsObservation, SatnogsSchedule, SkippedPass,
    satnogs_schedule, station_schedule, station_schedule_json,
};
pub use plan::{
    ContactPlan, DEFAULT_OPTIMAL_TIME_LIMIT, DecidedBy, JSON_SCHEMA, Optimality, PlanEntry,
    PlanOptions, SatelliteSummary, Scheduler, StationSummary, plan_contacts, plan_contacts_with,
    time_until_first_pass,
};
pub use satellite::{Satellite, satellites_from_catalog, satellites_from_omm, satellites_from_tle};
pub use station::{GroundStation, SlewLimits, StationSite};
pub use survey::{RejectedPass, SurveyFailure, WindowSurvey, contact_windows};
pub use time::{timestamp_ceil, timestamp_floor, utc_from_timestamp};
pub use window::window_from_pass;
