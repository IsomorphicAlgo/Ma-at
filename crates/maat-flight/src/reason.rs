//! Stable reason codes: why a pass was scheduled, dropped or refused.
//!
//! Every decision Ma'at makes carries a [`ReasonCode`]. Each code has a number that never
//! changes, an identifier built from it (`MAAT-101`) and a one-line summary. Operators can search
//! a log for an identifier; software can match on the number.
//!
//! ## Numbering
//!
//! The hundreds digit is the class:
//!
//! | Range | Class | Meaning |
//! |---|---|---|
//! | `MAAT-0xx` | [`ReasonClass::Accepted`] | The pass is in the plan. |
//! | `MAAT-1xx` | [`ReasonClass::Rejected`] | A valid pass lost to a scheduling rule or a station limit. |
//! | `MAAT-2xx` | [`ReasonClass::InvalidInput`] | The input was refused before scheduling. |
//! | `MAAT-3xx` | [`ReasonClass::ResourceLimit`] | A fixed-size buffer was full. |
//!
//! ## Stability rule
//!
//! Once a code is published its number, identifier and meaning are frozen. New codes are added
//! with new numbers; a retired code's number is never reused. The enum is `#[non_exhaustive]`, so
//! adding a code is not a breaking change.

use core::fmt;

/// The broad category a [`ReasonCode`] belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ReasonClass {
    /// `MAAT-0xx`: the pass is in the plan.
    Accepted,
    /// `MAAT-1xx`: a valid pass lost to a scheduling rule or a station limit.
    Rejected,
    /// `MAAT-2xx`: the input was refused before scheduling.
    InvalidInput,
    /// `MAAT-3xx`: a fixed-size buffer was full, so the pass could not be evaluated.
    ResourceLimit,
}

impl ReasonClass {
    /// A short lowercase label, such as `"rejected"`.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Accepted => "accepted",
            Self::Rejected => "rejected",
            Self::InvalidInput => "invalid input",
            Self::ResourceLimit => "resource limit",
        }
    }
}

impl fmt::Display for ReasonClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// Declares the whole reason-code catalogue from a single table.
///
/// One row per code keeps the enum, the lookup functions and [`ReasonCode::ALL`] in step by
/// construction: it is impossible to add a code and forget to give it an identifier or a summary.
macro_rules! reason_codes {
    (
        $(
            $(#[$doc:meta])*
            $variant:ident = $number:literal, $id:literal, $class:ident, $summary:literal;
        )+
    ) => {
        /// Why a pass was scheduled, dropped or refused. See the [module docs](self).
        ///
        /// ```
        /// use maat_flight::{ReasonClass, ReasonCode};
        ///
        /// let reason = ReasonCode::StationBusy;
        /// assert_eq!(reason.code(), 101);
        /// assert_eq!(reason.id(), "MAAT-101");
        /// assert_eq!(reason.class(), ReasonClass::Rejected);
        /// assert_eq!(ReasonCode::from_code(101), Some(reason));
        /// ```
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        #[non_exhaustive]
        #[repr(u16)]
        pub enum ReasonCode {
            $(
                $(#[$doc])*
                $variant = $number,
            )+
        }

        impl ReasonCode {
            /// Every reason code, in ascending numeric order.
            pub const ALL: &'static [Self] = &[$(Self::$variant,)+];

            /// The stable number, such as `101`.
            #[must_use]
            pub const fn code(self) -> u16 {
                self as u16
            }

            /// Looks a code up by number. Returns `None` for a number that is not assigned.
            #[must_use]
            pub const fn from_code(code: u16) -> Option<Self> {
                match code {
                    $($number => Some(Self::$variant),)+
                    _ => None,
                }
            }

            /// The stable identifier, such as `"MAAT-101"`.
            #[must_use]
            pub const fn id(self) -> &'static str {
                match self {
                    $(Self::$variant => $id,)+
                }
            }

            /// The category this code belongs to.
            #[must_use]
            pub const fn class(self) -> ReasonClass {
                match self {
                    $(Self::$variant => ReasonClass::$class,)+
                }
            }

            /// A one-line, human-readable explanation.
            #[must_use]
            pub const fn summary(self) -> &'static str {
                match self {
                    $(Self::$variant => $summary,)+
                }
            }
        }
    };
}

reason_codes! {
    /// The pass was accepted into the contact plan.
    Scheduled = 1, "MAAT-001", Accepted,
        "scheduled: the pass is in the contact plan";

    /// The station is already committed to a higher-priority pass that overlaps this one.
    StationBusy = 101, "MAAT-101", Rejected,
        "dropped: overlaps a higher-priority pass at the same station";

    /// The satellite is already scheduled with a different station during this pass.
    SatelliteBusy = 102, "MAAT-102", Rejected,
        "dropped: the satellite is already scheduled with another station at that time";

    /// The pass is shorter than the minimum useful contact length.
    BelowMinimumDuration = 103, "MAAT-103", Rejected,
        "dropped: the pass is shorter than the minimum contact length";

    /// Earlier passes already give the satellite all the contact time it asked for.
    NeedAlreadyMet = 104, "MAAT-104", Rejected,
        "dropped: the satellite's contact need is already met by other passes";

    /// Tracking the pass would need the antenna to turn faster than the station's slew limit.
    SlewRateExceeded = 105, "MAAT-105", Rejected,
        "dropped: tracking the pass needs a faster antenna slew than the station allows";

    /// No contact request names this satellite, so the scheduler has nothing to schedule it for.
    NoRequest = 106, "MAAT-106", Rejected,
        "dropped: no contact request covers this satellite";

    /// The station is committed to an overlapping pass of equal priority that won the tie-break.
    StationTieLost = 107, "MAAT-107", Rejected,
        "dropped: lost a tie with an equal-priority pass at the same station";

    /// The pass does not overlap another at the station but leaves too little time to re-point.
    StationTurnaround = 108, "MAAT-108", Rejected,
        "dropped: too close to another pass for the station to re-point in time";

    /// The optimal scheduler kept a conflicting pass because the plan as a whole is worth more.
    OutweighedByPlan = 109, "MAAT-109", Rejected,
        "dropped: conflicts with a pass the optimizer kept for a better overall plan";

    /// Loss of signal is not later than acquisition of signal, so the window has no duration.
    LosNotAfterAos = 201, "MAAT-201", InvalidInput,
        "invalid window: loss of signal is not later than acquisition of signal";

    /// The window is longer than a 32-bit count of seconds can hold.
    WindowTooLong = 202, "MAAT-202", InvalidInput,
        "invalid window: the duration does not fit in 32-bit seconds";

    /// The maximum elevation is not between the horizon and the zenith.
    ElevationOutOfRange = 203, "MAAT-203", InvalidInput,
        "invalid window: the maximum elevation is outside 0 to 90 degrees";

    /// Two contact requests name the same satellite.
    DuplicateRequest = 204, "MAAT-204", InvalidInput,
        "invalid request: a satellite has more than one contact request";

    /// Two station rules name the same station.
    DuplicateStationRule = 205, "MAAT-205", InvalidInput,
        "invalid request: a station has more than one rule";

    /// An azimuth is not a compass direction from 0 up to, but not including, 360 degrees.
    AzimuthOutOfRange = 206, "MAAT-206", InvalidInput,
        "invalid window: an azimuth is outside 0 to 360 degrees";

    /// A fixed-size buffer was full, so the pass could not be evaluated.
    CapacityExceeded = 301, "MAAT-301", ResourceLimit,
        "not evaluated: a fixed-size plan buffer is full";
}

impl ReasonCode {
    /// `true` only for [`ReasonCode::Scheduled`]-class codes: the pass is in the plan.
    #[must_use]
    pub const fn is_accepted(self) -> bool {
        matches!(self.class(), ReasonClass::Accepted)
    }
}

impl fmt::Display for ReasonCode {
    /// Writes the identifier and summary, for example
    /// `MAAT-101: dropped: overlaps a higher-priority pass at the same station`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.id(), self.summary())
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;
    use std::format;

    /// The published catalogue, written out by hand.
    ///
    /// This is the stability guard. If a change to the table above renumbers or removes a code,
    /// this test fails. Adding a code means adding a line here on purpose; an existing line must
    /// never be edited.
    const FROZEN: &[(ReasonCode, u16, &str)] = &[
        (ReasonCode::Scheduled, 1, "MAAT-001"),
        (ReasonCode::StationBusy, 101, "MAAT-101"),
        (ReasonCode::SatelliteBusy, 102, "MAAT-102"),
        (ReasonCode::BelowMinimumDuration, 103, "MAAT-103"),
        (ReasonCode::NeedAlreadyMet, 104, "MAAT-104"),
        (ReasonCode::SlewRateExceeded, 105, "MAAT-105"),
        (ReasonCode::NoRequest, 106, "MAAT-106"),
        (ReasonCode::StationTieLost, 107, "MAAT-107"),
        (ReasonCode::StationTurnaround, 108, "MAAT-108"),
        (ReasonCode::OutweighedByPlan, 109, "MAAT-109"),
        (ReasonCode::LosNotAfterAos, 201, "MAAT-201"),
        (ReasonCode::WindowTooLong, 202, "MAAT-202"),
        (ReasonCode::ElevationOutOfRange, 203, "MAAT-203"),
        (ReasonCode::DuplicateRequest, 204, "MAAT-204"),
        (ReasonCode::DuplicateStationRule, 205, "MAAT-205"),
        (ReasonCode::AzimuthOutOfRange, 206, "MAAT-206"),
        (ReasonCode::CapacityExceeded, 301, "MAAT-301"),
    ];

    #[test]
    fn published_codes_keep_their_numbers_and_identifiers() {
        for &(reason, number, id) in FROZEN {
            assert_eq!(reason.code(), number, "{reason:?} was renumbered");
            assert_eq!(reason.id(), id, "{reason:?} changed identifier");
        }
    }

    #[test]
    fn frozen_list_covers_the_whole_catalogue() {
        assert_eq!(
            FROZEN.len(),
            ReasonCode::ALL.len(),
            "a reason code was added or removed without updating the FROZEN stability list"
        );
        for (frozen, actual) in FROZEN.iter().zip(ReasonCode::ALL) {
            assert_eq!(frozen.0, *actual);
        }
    }

    #[test]
    fn identifier_is_the_number_zero_padded_to_three_digits() {
        for reason in ReasonCode::ALL {
            assert_eq!(reason.id(), format!("MAAT-{:03}", reason.code()));
        }
    }

    #[test]
    fn from_code_round_trips_every_code() {
        for reason in ReasonCode::ALL {
            assert_eq!(ReasonCode::from_code(reason.code()), Some(*reason));
        }
    }

    #[test]
    fn from_code_rejects_every_unassigned_number() {
        let assigned = (0..=u16::MAX)
            .filter(|code| ReasonCode::from_code(*code).is_some())
            .count();
        assert_eq!(assigned, ReasonCode::ALL.len());
        assert_eq!(ReasonCode::from_code(0), None);
        assert_eq!(ReasonCode::from_code(100), None);
        assert_eq!(ReasonCode::from_code(u16::MAX), None);
    }

    #[test]
    fn catalogue_is_in_strictly_ascending_order() {
        // Strictly ascending also proves that no two codes share a number.
        for pair in ReasonCode::ALL.windows(2) {
            assert!(
                pair[0].code() < pair[1].code(),
                "{:?} and {:?} are out of order",
                pair[0],
                pair[1]
            );
        }
    }

    #[test]
    fn class_matches_the_hundreds_digit() {
        for reason in ReasonCode::ALL {
            let expected = match reason.code() / 100 {
                0 => ReasonClass::Accepted,
                1 => ReasonClass::Rejected,
                2 => ReasonClass::InvalidInput,
                3 => ReasonClass::ResourceLimit,
                other => panic!("{reason:?} uses an unassigned class digit {other}"),
            };
            assert_eq!(reason.class(), expected, "{reason:?}");
        }
    }

    #[test]
    fn only_accepted_codes_report_is_accepted() {
        assert!(ReasonCode::Scheduled.is_accepted());
        for reason in ReasonCode::ALL {
            assert_eq!(
                reason.is_accepted(),
                reason.class() == ReasonClass::Accepted
            );
        }
        assert!(!ReasonCode::StationBusy.is_accepted());
    }

    #[test]
    fn summaries_are_single_line_printable_ascii() {
        // Plain ASCII keeps the text safe for any downlink, log or terminal.
        for reason in ReasonCode::ALL {
            let summary = reason.summary();
            assert!(!summary.is_empty(), "{reason:?} has no summary");
            assert!(
                summary.bytes().all(|b| b == b' ' || b.is_ascii_graphic()),
                "{reason:?} summary is not single-line printable ASCII: {summary:?}"
            );
            assert!(summary.len() <= 80, "{reason:?} summary is over 80 columns");
        }
    }

    #[test]
    fn display_joins_identifier_and_summary() {
        assert_eq!(
            format!("{}", ReasonCode::StationBusy),
            "MAAT-101: dropped: overlaps a higher-priority pass at the same station"
        );
    }

    #[test]
    fn class_labels_are_distinct() {
        assert_eq!(format!("{}", ReasonClass::Accepted), "accepted");
        assert_eq!(format!("{}", ReasonClass::Rejected), "rejected");
        assert_eq!(format!("{}", ReasonClass::InvalidInput), "invalid input");
        assert_eq!(format!("{}", ReasonClass::ResourceLimit), "resource limit");
    }
}
