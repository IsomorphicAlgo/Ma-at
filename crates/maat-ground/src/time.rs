//! Calendar time on the ground, whole seconds in flight.
//!
//! Ephemerust speaks [`chrono::DateTime<Utc>`] with sub-second precision. The flight layer keeps
//! a [`Timestamp`] of whole seconds. Going from the first to the second loses the fraction, so
//! the caller must say which way to round. There is deliberately no plain "convert" function.

use chrono::{DateTime, Utc};
use maat_flight::Timestamp;

use crate::error::GroundError;

/// Converts a UTC time to a flight timestamp, rounding **down** to the whole second.
///
/// Use this for the *end* of an interval (LOS), so the integer interval stays inside the real one.
///
/// ```
/// use chrono::{TimeZone, Utc};
/// use maat_ground::timestamp_floor;
///
/// let t = Utc.timestamp_opt(1_800_000_000, 900_000_000).unwrap(); // ....000.9 s
/// assert_eq!(timestamp_floor(t).unix_seconds(), 1_800_000_000);
/// ```
#[must_use]
pub fn timestamp_floor(time: DateTime<Utc>) -> Timestamp {
    // `DateTime::timestamp` is already a floor: chrono stores whole seconds plus a non-negative
    // fraction, including for times before 1970.
    Timestamp::from_unix_seconds(time.timestamp())
}

/// Converts a UTC time to a flight timestamp, rounding **up** to the whole second.
///
/// Use this for the *start* of an interval (AOS), so the integer interval stays inside the real
/// one. A time that is already on a whole second is unchanged.
///
/// ```
/// use chrono::{TimeZone, Utc};
/// use maat_ground::timestamp_ceil;
///
/// let fractional = Utc.timestamp_opt(1_800_000_000, 1).unwrap(); // one nanosecond past
/// assert_eq!(timestamp_ceil(fractional).unix_seconds(), 1_800_000_001);
///
/// let whole = Utc.timestamp_opt(1_800_000_000, 0).unwrap();
/// assert_eq!(timestamp_ceil(whole).unix_seconds(), 1_800_000_000);
/// ```
#[must_use]
pub fn timestamp_ceil(time: DateTime<Utc>) -> Timestamp {
    let whole = time.timestamp();
    let seconds = if time.timestamp_subsec_nanos() > 0 {
        whole.saturating_add(1)
    } else {
        whole
    };
    Timestamp::from_unix_seconds(seconds)
}

/// Converts a flight timestamp back to a UTC calendar time.
///
/// # Errors
///
/// Returns [`GroundError::TimeOutOfRange`] if the timestamp is beyond the dates `chrono` can
/// represent. An `i64` of seconds reaches about 292 billion years either side of 1970; a calendar
/// does not.
///
/// ```
/// use chrono::{TimeZone, Utc};
/// use maat_ground::flight::Timestamp;
/// use maat_ground::utc_from_timestamp;
///
/// let utc = utc_from_timestamp(Timestamp::from_unix_seconds(1_800_000_000))?;
/// assert_eq!(utc, Utc.with_ymd_and_hms(2027, 1, 15, 8, 0, 0).unwrap());
/// # Ok::<(), maat_ground::GroundError>(())
/// ```
pub fn utc_from_timestamp(timestamp: Timestamp) -> Result<DateTime<Utc>, GroundError> {
    let unix_seconds = timestamp.unix_seconds();
    DateTime::from_timestamp(unix_seconds, 0).ok_or(GroundError::TimeOutOfRange { unix_seconds })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn utc(seconds: i64, nanos: u32) -> DateTime<Utc> {
        Utc.timestamp_opt(seconds, nanos).unwrap()
    }

    #[test]
    fn whole_seconds_are_unchanged_by_either_rounding() {
        for seconds in [-86_400, -1, 0, 1, 1_800_000_000] {
            let t = utc(seconds, 0);
            assert_eq!(timestamp_floor(t).unix_seconds(), seconds);
            assert_eq!(timestamp_ceil(t).unix_seconds(), seconds);
        }
    }

    #[test]
    fn fractions_floor_down_and_ceil_up() {
        for nanos in [1, 500_000_000, 999_999_999] {
            let t = utc(100, nanos);
            assert_eq!(timestamp_floor(t).unix_seconds(), 100);
            assert_eq!(timestamp_ceil(t).unix_seconds(), 101);
        }
    }

    #[test]
    fn rounding_is_toward_minus_and_plus_infinity_before_1970() {
        // -1.5 s is stored by chrono as -2 s + 0.5 s. Floor must give -2 and ceil -1; rounding
        // toward zero would get both wrong.
        let t = utc(-2, 500_000_000);
        assert_eq!(timestamp_floor(t).unix_seconds(), -2);
        assert_eq!(timestamp_ceil(t).unix_seconds(), -1);
    }

    #[test]
    fn floor_never_exceeds_ceil_and_they_differ_by_at_most_one() {
        for seconds in [-3, 0, 7] {
            for nanos in [0, 1, 250_000_000, 999_999_999] {
                let t = utc(seconds, nanos);
                let gap = timestamp_ceil(t).unix_seconds() - timestamp_floor(t).unix_seconds();
                assert!(
                    (0..=1).contains(&gap),
                    "gap {gap} at {seconds}s + {nanos}ns"
                );
            }
        }
    }

    #[test]
    fn timestamp_round_trips_through_utc() {
        for seconds in [-1_000_000, 0, 1_594_588_800, 1_800_000_000] {
            let timestamp = Timestamp::from_unix_seconds(seconds);
            let back = timestamp_floor(utc_from_timestamp(timestamp).unwrap());
            assert_eq!(back, timestamp);
        }
    }

    #[test]
    fn out_of_range_timestamps_are_reported_not_panicked() {
        for seconds in [i64::MAX, i64::MIN] {
            assert!(matches!(
                utc_from_timestamp(Timestamp::from_unix_seconds(seconds)),
                Err(GroundError::TimeOutOfRange { unix_seconds }) if unix_seconds == seconds
            ));
        }
    }
}
