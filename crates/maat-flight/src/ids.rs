//! Identifiers and priority.
//!
//! These are *newtypes*: a struct wrapping one integer. They cost nothing at run time, but the
//! compiler will refuse to let a station number be passed where a satellite number is expected.

use core::fmt;

/// Identifies one satellite.
///
/// By convention this is the NORAD catalog number (25544 for the ISS). A `u32` holds the
/// nine-digit catalog numbers used by CCSDS OMM element sets, which the five-digit field of the
/// classic two-line element format cannot.
///
/// ```
/// use maat_flight::SatelliteId;
///
/// let iss = SatelliteId::new(25544);
/// assert_eq!(iss.get(), 25544);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SatelliteId(u32);

impl SatelliteId {
    /// Wraps a raw identifier.
    #[must_use]
    pub const fn new(id: u32) -> Self {
        Self(id)
    }

    /// Returns the raw identifier.
    #[must_use]
    pub const fn get(self) -> u32 {
        self.0
    }
}

impl fmt::Display for SatelliteId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Identifies one ground station within a contact plan.
///
/// The number is assigned by whoever builds the plan; it only has to be unique inside that plan.
///
/// ```
/// use maat_flight::StationId;
///
/// assert_eq!(StationId::new(7).get(), 7);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct StationId(u16);

impl StationId {
    /// Wraps a raw identifier.
    #[must_use]
    pub const fn new(id: u16) -> Self {
        Self(id)
    }

    /// Returns the raw identifier.
    #[must_use]
    pub const fn get(self) -> u16 {
        self.0
    }
}

impl fmt::Display for StationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// How important a satellite's contact is. **A higher value wins.**
///
/// Because the type derives `Ord`, priorities compare with the ordinary operators.
///
/// ```
/// use maat_flight::Priority;
///
/// let routine = Priority::new(10);
/// let urgent = Priority::new(200);
/// assert!(urgent > routine);
/// assert!(Priority::HIGHEST > urgent);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Priority(u8);

impl Priority {
    /// The lowest priority, `0`.
    pub const LOWEST: Self = Self(u8::MIN);
    /// The highest priority, `255`.
    pub const HIGHEST: Self = Self(u8::MAX);

    /// Wraps a raw priority value.
    #[must_use]
    pub const fn new(value: u8) -> Self {
        Self(value)
    }

    /// Returns the raw priority value.
    #[must_use]
    pub const fn get(self) -> u8 {
        self.0
    }
}

impl fmt::Display for Priority {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;
    use std::format;

    #[test]
    fn identifiers_round_trip_their_raw_value() {
        assert_eq!(SatelliteId::new(u32::MAX).get(), u32::MAX);
        assert_eq!(StationId::new(u16::MAX).get(), u16::MAX);
        assert_eq!(Priority::new(42).get(), 42);
    }

    #[test]
    fn satellite_id_holds_a_nine_digit_catalog_number() {
        // OMM allows nine digits; the TLE format stops at five.
        assert_eq!(SatelliteId::new(999_999_999).get(), 999_999_999);
    }

    #[test]
    fn higher_priority_value_compares_greater() {
        assert!(Priority::new(2) > Priority::new(1));
        assert!(Priority::LOWEST < Priority::HIGHEST);
        assert_eq!(Priority::LOWEST.get(), 0);
        assert_eq!(Priority::HIGHEST.get(), 255);
    }

    #[test]
    fn display_prints_the_bare_number() {
        assert_eq!(format!("{}", SatelliteId::new(25544)), "25544");
        assert_eq!(format!("{}", StationId::new(3)), "3");
        assert_eq!(format!("{}", Priority::new(9)), "9");
    }
}
