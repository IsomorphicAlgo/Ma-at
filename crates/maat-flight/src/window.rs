//! Contact windows: when one satellite can talk to one ground station.

use core::fmt;

use crate::ids::{SatelliteId, StationId};
use crate::reason::ReasonCode;
use crate::time::{Seconds, Timestamp};

/// An elevation above the horizon, stored as whole hundredths of a degree.
///
/// Storing an integer keeps the flight layer free of floating point. A hundredth of a degree is
/// already finer than pass prediction from public element sets can justify.
///
/// ```
/// use maat_flight::Elevation;
///
/// let peak = Elevation::from_centidegrees(4_725)?; // 47.25 degrees
/// assert_eq!(peak.centidegrees(), 4_725);
/// assert!(peak > Elevation::HORIZON && peak < Elevation::ZENITH);
/// assert!(Elevation::from_centidegrees(9_001).is_err());
/// # Ok::<(), maat_flight::WindowError>(())
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Elevation(u16);

impl Elevation {
    /// The largest valid raw value: 90.00 degrees.
    pub const MAX_CENTIDEGREES: u16 = 9_000;
    /// 0.00 degrees: on the horizon.
    pub const HORIZON: Self = Self(0);
    /// 90.00 degrees: directly overhead.
    pub const ZENITH: Self = Self(Self::MAX_CENTIDEGREES);

    /// Builds an elevation from hundredths of a degree.
    ///
    /// # Errors
    ///
    /// Returns [`WindowError::ElevationOutOfRange`] if the value is above 9000 (90.00 degrees).
    pub const fn from_centidegrees(centidegrees: u16) -> Result<Self, WindowError> {
        if centidegrees > Self::MAX_CENTIDEGREES {
            Err(WindowError::ElevationOutOfRange { centidegrees })
        } else {
            Ok(Self(centidegrees))
        }
    }

    /// Returns the elevation in hundredths of a degree, from `0` to `9000`.
    #[must_use]
    pub const fn centidegrees(self) -> u16 {
        self.0
    }
}

impl fmt::Display for Elevation {
    /// Writes the elevation in degrees with two decimals, for example `47.25 deg`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Dividing by a non-zero constant cannot overflow or panic.
        let whole = self.0 / 100;
        let hundredths = self.0 % 100;
        write!(f, "{whole}.{hundredths:02} deg")
    }
}

/// One opportunity for one satellite to talk to one ground station.
///
/// The window is the **half-open** interval `[aos, los)`: in contact at acquisition of signal
/// (AOS), no longer in contact at loss of signal (LOS). A window that ends at the same second
/// another begins does not overlap it.
///
/// A `ContactWindow` can only be built through [`ContactWindow::new`], which checks it. Code that
/// receives one can therefore rely on `los` being later than `aos` without checking again.
///
/// ```
/// use maat_flight::{ContactWindow, Elevation, SatelliteId, Seconds, StationId, Timestamp};
///
/// let t = Timestamp::from_unix_seconds;
/// let peak = Elevation::from_centidegrees(3_000)?;
/// let first = ContactWindow::new(SatelliteId::new(25544), StationId::new(1), t(0), t(600), peak)?;
/// let second = ContactWindow::new(SatelliteId::new(43013), StationId::new(1), t(600), t(900), peak)?;
///
/// assert_eq!(first.duration(), Seconds::new(600));
/// assert!(first.contains(t(599)));
/// assert!(!first.contains(t(600)));   // LOS itself is outside the window
/// assert!(!first.overlaps(&second));  // back-to-back passes do not conflict
/// # Ok::<(), maat_flight::WindowError>(())
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ContactWindow {
    satellite: SatelliteId,
    station: StationId,
    aos: Timestamp,
    los: Timestamp,
    // Computed once in `new`, where the subtraction is checked, so `duration()` cannot fail.
    duration: Seconds,
    max_elevation: Elevation,
}

impl ContactWindow {
    /// Builds a contact window after checking that it makes sense.
    ///
    /// # Errors
    ///
    /// - [`WindowError::LosNotAfterAos`] if `los` is not strictly later than `aos`.
    /// - [`WindowError::TooLong`] if the window is longer than [`Seconds::MAX`].
    pub fn new(
        satellite: SatelliteId,
        station: StationId,
        aos: Timestamp,
        los: Timestamp,
        max_elevation: Elevation,
    ) -> Result<Self, WindowError> {
        if los <= aos {
            return Err(WindowError::LosNotAfterAos { aos, los });
        }
        let duration = aos
            .seconds_until(los)
            .ok_or(WindowError::TooLong { aos, los })?;
        Ok(Self {
            satellite,
            station,
            aos,
            los,
            duration,
            max_elevation,
        })
    }

    /// The satellite this window is for.
    #[must_use]
    pub const fn satellite(&self) -> SatelliteId {
        self.satellite
    }

    /// The ground station this window is for.
    #[must_use]
    pub const fn station(&self) -> StationId {
        self.station
    }

    /// Acquisition of signal: the first second of contact.
    #[must_use]
    pub const fn aos(&self) -> Timestamp {
        self.aos
    }

    /// Loss of signal: the first second that is no longer in contact.
    #[must_use]
    pub const fn los(&self) -> Timestamp {
        self.los
    }

    /// How long the contact lasts. Always at least one second.
    #[must_use]
    pub const fn duration(&self) -> Seconds {
        self.duration
    }

    /// The highest elevation the satellite reaches during the pass.
    #[must_use]
    pub const fn max_elevation(&self) -> Elevation {
        self.max_elevation
    }

    /// Whether `time` falls inside the window. AOS is inside; LOS is not.
    #[must_use]
    pub fn contains(&self, time: Timestamp) -> bool {
        self.aos <= time && time < self.los
    }

    /// Whether this window and `other` share at least one second.
    ///
    /// This compares time only. Whether an overlap is a *conflict* depends on what the two
    /// windows have in common (the same station, or the same satellite), which is the
    /// scheduler's decision.
    #[must_use]
    pub fn overlaps(&self, other: &Self) -> bool {
        self.aos < other.los && other.aos < self.los
    }
}

/// Why a contact window, or one of its parts, was refused.
///
/// Each variant maps to a stable [`ReasonCode`] through [`WindowError::reason`], and the
/// `Display` text starts with that code's identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum WindowError {
    /// Loss of signal is not strictly later than acquisition of signal.
    LosNotAfterAos {
        /// The acquisition-of-signal time that was given.
        aos: Timestamp,
        /// The loss-of-signal time that was given.
        los: Timestamp,
    },
    /// The window is longer than [`Seconds::MAX`], about 136 years.
    TooLong {
        /// The acquisition-of-signal time that was given.
        aos: Timestamp,
        /// The loss-of-signal time that was given.
        los: Timestamp,
    },
    /// An elevation above 90.00 degrees.
    ElevationOutOfRange {
        /// The raw value that was given, in hundredths of a degree.
        centidegrees: u16,
    },
}

impl WindowError {
    /// The stable reason code for this error.
    #[must_use]
    pub const fn reason(&self) -> ReasonCode {
        match self {
            Self::LosNotAfterAos { .. } => ReasonCode::LosNotAfterAos,
            Self::TooLong { .. } => ReasonCode::WindowTooLong,
            Self::ElevationOutOfRange { .. } => ReasonCode::ElevationOutOfRange,
        }
    }
}

impl fmt::Display for WindowError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let id = self.reason().id();
        match self {
            Self::LosNotAfterAos { aos, los } => write!(
                f,
                "{id}: loss of signal ({los}) must be later than acquisition of signal ({aos}); \
                 a contact window needs a duration of at least one second"
            ),
            Self::TooLong { aos, los } => write!(
                f,
                "{id}: the window from {aos} to {los} is longer than {} seconds, the most a \
                 32-bit duration can hold; check both times use the same epoch and unit",
                u32::MAX
            ),
            Self::ElevationOutOfRange { centidegrees } => write!(
                f,
                "{id}: elevation {centidegrees} is above {}, the value for 90.00 degrees; \
                 elevation is given in hundredths of a degree from the horizon (0) to the zenith",
                Elevation::MAX_CENTIDEGREES
            ),
        }
    }
}

impl core::error::Error for WindowError {}

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;
    use std::format;
    use std::string::ToString;

    const ISS: SatelliteId = SatelliteId::new(25544);
    const STATION: StationId = StationId::new(1);

    fn t(seconds: i64) -> Timestamp {
        Timestamp::from_unix_seconds(seconds)
    }

    fn window(aos: i64, los: i64) -> ContactWindow {
        ContactWindow::new(ISS, STATION, t(aos), t(los), Elevation::ZENITH).unwrap()
    }

    // --- Elevation -----------------------------------------------------------------------------

    #[test]
    fn elevation_accepts_the_whole_valid_range() {
        assert_eq!(Elevation::from_centidegrees(0), Ok(Elevation::HORIZON));
        assert_eq!(Elevation::from_centidegrees(9_000), Ok(Elevation::ZENITH));
        assert_eq!(
            Elevation::from_centidegrees(4_725).unwrap().centidegrees(),
            4_725
        );
    }

    #[test]
    fn elevation_rejects_anything_above_the_zenith() {
        assert_eq!(
            Elevation::from_centidegrees(9_001),
            Err(WindowError::ElevationOutOfRange {
                centidegrees: 9_001
            })
        );
        assert!(Elevation::from_centidegrees(u16::MAX).is_err());
    }

    #[test]
    fn elevation_orders_from_horizon_to_zenith() {
        let low = Elevation::from_centidegrees(500).unwrap();
        let high = Elevation::from_centidegrees(8_000).unwrap();
        assert!(Elevation::HORIZON < low && low < high && high < Elevation::ZENITH);
    }

    #[test]
    fn elevation_displays_degrees_with_two_decimals() {
        let display = |cd: u16| format!("{}", Elevation::from_centidegrees(cd).unwrap());
        assert_eq!(display(0), "0.00 deg");
        assert_eq!(display(5), "0.05 deg");
        assert_eq!(display(4_725), "47.25 deg");
        assert_eq!(display(9_000), "90.00 deg");
    }

    // --- ContactWindow construction ------------------------------------------------------------

    #[test]
    fn new_keeps_every_field() {
        let peak = Elevation::from_centidegrees(1_234).unwrap();
        let w = ContactWindow::new(ISS, STATION, t(100), t(700), peak).unwrap();
        assert_eq!(w.satellite(), ISS);
        assert_eq!(w.station(), STATION);
        assert_eq!(w.aos(), t(100));
        assert_eq!(w.los(), t(700));
        assert_eq!(w.duration(), Seconds::new(600));
        assert_eq!(w.max_elevation(), peak);
    }

    #[test]
    fn new_accepts_the_shortest_possible_window() {
        assert_eq!(window(10, 11).duration(), Seconds::new(1));
    }

    #[test]
    fn new_accepts_times_before_the_unix_epoch() {
        assert_eq!(window(-600, -60).duration(), Seconds::new(540));
        assert_eq!(window(-30, 30).duration(), Seconds::new(60));
    }

    #[test]
    fn new_rejects_a_zero_length_window() {
        let err = ContactWindow::new(ISS, STATION, t(50), t(50), Elevation::ZENITH).unwrap_err();
        assert_eq!(
            err,
            WindowError::LosNotAfterAos {
                aos: t(50),
                los: t(50)
            }
        );
    }

    #[test]
    fn new_rejects_a_backwards_window() {
        let err = ContactWindow::new(ISS, STATION, t(700), t(100), Elevation::ZENITH).unwrap_err();
        assert_eq!(
            err,
            WindowError::LosNotAfterAos {
                aos: t(700),
                los: t(100)
            }
        );
    }

    #[test]
    fn new_accepts_the_longest_window_and_rejects_one_second_more() {
        let longest = i64::from(u32::MAX);
        assert_eq!(window(0, longest).duration(), Seconds::MAX);

        let err =
            ContactWindow::new(ISS, STATION, t(0), t(longest + 1), Elevation::ZENITH).unwrap_err();
        assert_eq!(
            err,
            WindowError::TooLong {
                aos: t(0),
                los: t(longest + 1)
            }
        );
    }

    #[test]
    fn new_reports_too_long_instead_of_overflowing_at_the_extremes() {
        let err = ContactWindow::new(ISS, STATION, t(i64::MIN), t(i64::MAX), Elevation::ZENITH)
            .unwrap_err();
        assert_eq!(err.reason(), ReasonCode::WindowTooLong);
    }

    // --- contains / overlaps -------------------------------------------------------------------

    #[test]
    fn contains_includes_aos_and_excludes_los() {
        let w = window(100, 200);
        assert!(!w.contains(t(99)));
        assert!(w.contains(t(100)));
        assert!(w.contains(t(199)));
        assert!(!w.contains(t(200)));
    }

    #[test]
    fn overlaps_is_false_for_back_to_back_windows() {
        let first = window(100, 200);
        let second = window(200, 300);
        assert!(!first.overlaps(&second));
        assert!(!second.overlaps(&first));
    }

    #[test]
    fn overlaps_is_true_for_a_single_shared_second() {
        let first = window(100, 201);
        let second = window(200, 300);
        assert!(first.overlaps(&second));
        assert!(second.overlaps(&first));
    }

    #[test]
    fn overlaps_is_true_when_one_window_is_inside_another() {
        let outer = window(100, 500);
        let inner = window(200, 300);
        assert!(outer.overlaps(&inner));
        assert!(inner.overlaps(&outer));
    }

    #[test]
    fn overlaps_is_false_for_separate_windows() {
        let early = window(100, 200);
        let late = window(900, 1_000);
        assert!(!early.overlaps(&late));
        assert!(!late.overlaps(&early));
    }

    #[test]
    fn a_window_overlaps_itself() {
        let w = window(100, 200);
        assert!(w.overlaps(&w));
    }

    #[test]
    fn overlaps_agrees_with_contains_second_by_second() {
        // Cross-check the interval formula against the definition: two windows overlap exactly
        // when some second lies inside both. Exhaustive over every window in a small range.
        for a_start in 0..6 {
            for a_end in (a_start + 1)..7 {
                for b_start in 0..6 {
                    for b_end in (b_start + 1)..7 {
                        let a = window(a_start, a_end);
                        let b = window(b_start, b_end);
                        let shared = (0..7).any(|s| a.contains(t(s)) && b.contains(t(s)));
                        assert_eq!(a.overlaps(&b), shared, "{a:?} vs {b:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn overlaps_ignores_satellite_and_station() {
        let peak = Elevation::HORIZON;
        let a =
            ContactWindow::new(SatelliteId::new(1), StationId::new(1), t(0), t(10), peak).unwrap();
        let b =
            ContactWindow::new(SatelliteId::new(2), StationId::new(2), t(5), t(15), peak).unwrap();
        assert!(a.overlaps(&b));
    }

    // --- WindowError ---------------------------------------------------------------------------

    #[test]
    fn every_error_maps_to_its_reason_code() {
        let inverted = WindowError::LosNotAfterAos {
            aos: t(2),
            los: t(1),
        };
        let too_long = WindowError::TooLong {
            aos: t(0),
            los: t(i64::MAX),
        };
        let elevation = WindowError::ElevationOutOfRange {
            centidegrees: 9_500,
        };
        assert_eq!(inverted.reason(), ReasonCode::LosNotAfterAos);
        assert_eq!(too_long.reason(), ReasonCode::WindowTooLong);
        assert_eq!(elevation.reason(), ReasonCode::ElevationOutOfRange);
    }

    #[test]
    fn error_messages_lead_with_the_code_and_name_the_bad_values() {
        let inverted = WindowError::LosNotAfterAos {
            aos: t(700),
            los: t(100),
        }
        .to_string();
        assert!(inverted.starts_with("MAAT-201: "), "{inverted}");
        assert!(inverted.contains("unix 700") && inverted.contains("unix 100"));

        let too_long = WindowError::TooLong {
            aos: t(0),
            los: t(5_000_000_000),
        }
        .to_string();
        assert!(too_long.starts_with("MAAT-202: "), "{too_long}");
        assert!(too_long.contains("unix 5000000000"));

        let elevation = WindowError::ElevationOutOfRange {
            centidegrees: 9_500,
        }
        .to_string();
        assert!(elevation.starts_with("MAAT-203: "), "{elevation}");
        assert!(elevation.contains("9500"));
    }

    #[test]
    fn window_error_is_a_standard_error() {
        fn assert_error<E: core::error::Error>(_: &E) {}
        assert_error(&WindowError::ElevationOutOfRange {
            centidegrees: 9_001,
        });
    }
}
