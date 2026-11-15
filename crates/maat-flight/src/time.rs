//! Time, as whole seconds.
//!
//! The flight layer keeps time as integers so that arithmetic is exact and identical on every
//! processor. One-second resolution is deliberate: pass prediction from public element sets is
//! only good to about a second, so anything finer would be false precision.
//!
//! All arithmetic here is *checked*: an operation that would overflow returns `None` instead of
//! wrapping around or panicking.

use core::fmt;

/// A point in time: whole seconds since the Unix epoch, 1970-01-01T00:00:00 UTC.
///
/// Like Unix time generally, this does not count leap seconds. Converting to and from calendar
/// time is the ground layer's job.
///
/// ```
/// use maat_flight::{Seconds, Timestamp};
///
/// let aos = Timestamp::from_unix_seconds(1_800_000_000);
/// let los = aos.checked_add(Seconds::new(600)).unwrap();
/// assert_eq!(aos.seconds_until(los), Some(Seconds::new(600)));
/// assert!(los > aos);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Timestamp(i64);

impl Timestamp {
    /// The Unix epoch itself, 1970-01-01T00:00:00 UTC.
    pub const UNIX_EPOCH: Self = Self(0);

    /// Builds a timestamp from whole seconds since the Unix epoch. Negative values are before it.
    #[must_use]
    pub const fn from_unix_seconds(seconds: i64) -> Self {
        Self(seconds)
    }

    /// Returns whole seconds since the Unix epoch.
    #[must_use]
    pub const fn unix_seconds(self) -> i64 {
        self.0
    }

    /// Returns the time `duration` after this one, or `None` if that is beyond what an `i64` holds.
    #[must_use]
    pub const fn checked_add(self, duration: Seconds) -> Option<Self> {
        // Widening a u32 into an i64 can never lose information.
        match self.0.checked_add(duration.get() as i64) {
            Some(seconds) => Some(Self(seconds)),
            None => None,
        }
    }

    /// Returns the time `duration` before this one, or `None` on overflow.
    #[must_use]
    pub const fn checked_sub(self, duration: Seconds) -> Option<Self> {
        match self.0.checked_sub(duration.get() as i64) {
            Some(seconds) => Some(Self(seconds)),
            None => None,
        }
    }

    /// Returns how long it is from this time until `later`.
    ///
    /// `None` means there is no such duration: either `later` is actually earlier than `self`, or
    /// the gap is longer than a [`Seconds`] value can hold (about 136 years).
    #[must_use]
    pub fn seconds_until(self, later: Self) -> Option<Seconds> {
        let gap = later.0.checked_sub(self.0)?;
        u32::try_from(gap).ok().map(Seconds::new)
    }
}

impl fmt::Display for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unix {}", self.0)
    }
}

/// A length of time in whole seconds.
///
/// A `u32` covers about 136 years, far longer than any contact, in four bytes.
///
/// ```
/// use maat_flight::Seconds;
///
/// let setup = Seconds::new(30);
/// let pass = Seconds::new(540);
/// assert_eq!(setup.checked_add(pass), Some(Seconds::new(570)));
/// assert_eq!(Seconds::MAX.checked_add(Seconds::new(1)), None);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Seconds(u32);

impl Seconds {
    /// No time at all.
    pub const ZERO: Self = Self(0);
    /// The longest representable duration.
    pub const MAX: Self = Self(u32::MAX);

    /// Builds a duration from a number of seconds.
    #[must_use]
    pub const fn new(seconds: u32) -> Self {
        Self(seconds)
    }

    /// Returns the number of seconds.
    #[must_use]
    pub const fn get(self) -> u32 {
        self.0
    }

    /// Adds two durations, or returns `None` if the sum does not fit.
    #[must_use]
    pub const fn checked_add(self, other: Self) -> Option<Self> {
        match self.0.checked_add(other.0) {
            Some(seconds) => Some(Self(seconds)),
            None => None,
        }
    }

    /// Adds two durations, stopping at [`Seconds::MAX`] instead of overflowing.
    ///
    /// Useful for running totals such as "contact time granted so far", where reaching the
    /// ceiling is harmless.
    #[must_use]
    pub const fn saturating_add(self, other: Self) -> Self {
        Self(self.0.saturating_add(other.0))
    }
}

impl fmt::Display for Seconds {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} s", self.0)
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;
    use std::format;

    #[test]
    fn timestamp_round_trips_unix_seconds() {
        assert_eq!(Timestamp::from_unix_seconds(-5).unix_seconds(), -5);
        assert_eq!(Timestamp::UNIX_EPOCH.unix_seconds(), 0);
    }

    #[test]
    fn timestamps_order_chronologically() {
        let before_epoch = Timestamp::from_unix_seconds(-1);
        let after_epoch = Timestamp::from_unix_seconds(1);
        assert!(before_epoch < Timestamp::UNIX_EPOCH);
        assert!(Timestamp::UNIX_EPOCH < after_epoch);
    }

    #[test]
    fn checked_add_and_sub_are_inverse() {
        let t = Timestamp::from_unix_seconds(1_800_000_000);
        let d = Seconds::new(540);
        let later = t.checked_add(d).unwrap();
        assert_eq!(later.unix_seconds(), 1_800_000_540);
        assert_eq!(later.checked_sub(d), Some(t));
    }

    #[test]
    fn checked_add_reports_overflow_instead_of_wrapping() {
        let end_of_time = Timestamp::from_unix_seconds(i64::MAX);
        assert_eq!(end_of_time.checked_add(Seconds::new(1)), None);
        assert_eq!(end_of_time.checked_add(Seconds::ZERO), Some(end_of_time));
    }

    #[test]
    fn checked_sub_reports_overflow_instead_of_wrapping() {
        let start_of_time = Timestamp::from_unix_seconds(i64::MIN);
        assert_eq!(start_of_time.checked_sub(Seconds::new(1)), None);
    }

    #[test]
    fn seconds_until_measures_a_forward_gap() {
        let a = Timestamp::from_unix_seconds(100);
        let b = Timestamp::from_unix_seconds(160);
        assert_eq!(a.seconds_until(b), Some(Seconds::new(60)));
        assert_eq!(a.seconds_until(a), Some(Seconds::ZERO));
    }

    #[test]
    fn seconds_until_is_none_when_the_other_time_is_earlier() {
        let a = Timestamp::from_unix_seconds(100);
        let b = Timestamp::from_unix_seconds(99);
        assert_eq!(a.seconds_until(b), None);
    }

    #[test]
    fn seconds_until_is_none_when_the_gap_does_not_fit() {
        let a = Timestamp::UNIX_EPOCH;
        let largest = Timestamp::from_unix_seconds(i64::from(u32::MAX));
        let too_far = Timestamp::from_unix_seconds(i64::from(u32::MAX) + 1);
        assert_eq!(a.seconds_until(largest), Some(Seconds::MAX));
        assert_eq!(a.seconds_until(too_far), None);
    }

    #[test]
    fn seconds_until_survives_the_extreme_ends_of_the_range() {
        // later - self would overflow an i64 here; the answer must be None, not a panic.
        let min = Timestamp::from_unix_seconds(i64::MIN);
        let max = Timestamp::from_unix_seconds(i64::MAX);
        assert_eq!(min.seconds_until(max), None);
        assert_eq!(max.seconds_until(min), None);
    }

    #[test]
    fn seconds_checked_add_reports_overflow() {
        assert_eq!(
            Seconds::new(1).checked_add(Seconds::new(2)),
            Some(Seconds::new(3))
        );
        assert_eq!(Seconds::MAX.checked_add(Seconds::new(1)), None);
    }

    #[test]
    fn seconds_saturating_add_stops_at_the_ceiling() {
        assert_eq!(Seconds::MAX.saturating_add(Seconds::new(10)), Seconds::MAX);
        assert_eq!(
            Seconds::new(5).saturating_add(Seconds::new(6)),
            Seconds::new(11)
        );
    }

    #[test]
    fn display_names_the_unit() {
        assert_eq!(format!("{}", Seconds::new(540)), "540 s");
        assert_eq!(format!("{}", Timestamp::from_unix_seconds(7)), "unix 7");
    }
}
