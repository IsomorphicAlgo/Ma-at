//! Longer explanations of the reason codes: what each one means and what would change it.
//!
//! A [`ReasonCode`]'s `summary` is one line, short enough for a log or a downlink, and lives in
//! the flight crate. The text here is for a person who has seen a code and wants to know what to
//! do about it. It lives on the ground because it is prose, and prose has no place onboard.

use maat_flight::ReasonCode;

/// What to tell someone who asks about a code this crate has no text for yet.
const NO_GUIDANCE: &str = "No further guidance has been written for this code. Its one-line \
     summary is the full explanation available.";

/// A paragraph on what a reason code means and what would change the outcome.
///
/// ```
/// use maat_ground::flight::ReasonCode;
/// use maat_ground::guidance;
///
/// let text = guidance(ReasonCode::StationTurnaround);
/// assert!(text.contains("turnaround"));
/// ```
#[must_use]
pub fn guidance(reason: ReasonCode) -> &'static str {
    match reason {
        ReasonCode::Scheduled => {
            "The pass is in the contact plan. Nothing already in the plan rules it out: its \
             station is free for the whole pass and for the turnaround time either side, its \
             satellite is not using another station, and the satellite still wants the contact."
        }
        ReasonCode::StationBusy => {
            "Another satellite with a higher priority has the same station at an overlapping \
             time. The greedy scheduler treats priority as a strict rank, so the higher priority \
             always wins, however short its pass. To schedule this pass, raise its satellite's \
             priority above the other's, or add a station that can see it at that time. The \
             detail names the pass that has the station."
        }
        ReasonCode::SatelliteBusy => {
            "The satellite is visible from two stations at once and the plan uses the other one. \
             Ma'at assumes a satellite talks to one station at a time. Between two passes of the \
             same satellite the longer one is taken, then the earlier one. Nothing needs fixing \
             unless the other station is the one you wanted; the detail names it."
        }
        ReasonCode::BelowMinimumDuration => {
            "The pass is shorter than the minimum useful contact. From the scheduler, that is \
             the minimum contact time in the satellite's request: lower it to accept shorter \
             passes. From the window survey, the pass lasts less than about a second and rounds \
             away to nothing, which no setting will change."
        }
        ReasonCode::NeedAlreadyMet => {
            "The satellite asked for a set amount of contact time and passes already in the plan \
             provide it, so this pass is surplus. Because a pass is used whole, the total may be \
             above the amount asked for. To schedule more, raise the need in the satellite's \
             request, or leave the need unset to ask for every pass."
        }
        ReasonCode::SlewRateExceeded => {
            "Following the satellite through this pass would need the antenna to turn faster \
             than the station's slew limit, in azimuth, elevation or both. It happens on passes \
             that go almost directly overhead, where the antenna must swing from one side of the \
             sky to the other in a few seconds. The whole pass is turned down; Ma'at does not \
             offer the usable parts before and after the fast section. Check the limit against \
             the mount's real figures. The detail gives the rate needed and the rate allowed."
        }
        ReasonCode::NoRequest => {
            "Windows were found for this satellite but no contact request names it, so the \
             scheduler has no priority or need to schedule it by. Add a request for the \
             satellite, or leave it out of the survey if it is not wanted."
        }
        ReasonCode::StationTieLost => {
            "Another satellite of the same priority has the same station at an overlapping time, \
             and it won the tie-break: the longer pass first, then the pass that begins earlier, \
             then the lower catalog number. To change the outcome, give this satellite a higher \
             priority. The detail says which step decided the tie."
        }
        ReasonCode::StationTurnaround => {
            "The pass does not overlap another pass at its station, but it begins or ends closer \
             to one than the station needs between passes. That time has two parts: the \
             station's fixed turnaround time, and, for a station with slew rates, the time to \
             turn its antenna from where the earlier pass ends to where the later one begins. \
             The other pass ranks higher, so it is kept. Shorten the turnaround time, or raise \
             the slew rates, if they are more cautious than the station really is. The detail \
             gives the gap and the time needed."
        }
        ReasonCode::OutweighedByPlan => {
            "The optimal scheduler left this pass out because it conflicts with a pass that is \
             in the plan, and swapping them would not raise the plan's total value. The value is \
             each satellite's useful contact time multiplied by its priority plus one, added up, \
             so unlike the greedy scheduler a long low-priority pass can outweigh a short \
             high-priority one. In a plan built with a floor under every satellite, a swap is also \
             refused if it would leave any satellite with less contact than the greedy plan \
             gives it. To change the outcome, raise this satellite's priority or lower the \
             other's. The detail names the pass that was kept."
        }
        ReasonCode::LosNotAfterAos => {
            "A contact window was given a loss-of-signal time that is not later than its \
             acquisition-of-signal time, so it has no duration. Check that the two times are in \
             the right order and use the same epoch and unit (whole seconds since 1970-01-01 \
             UTC)."
        }
        ReasonCode::WindowTooLong => {
            "A contact window is longer than a 32-bit count of seconds can hold, which is about \
             136 years. This is almost always a unit or epoch mistake: milliseconds given as \
             seconds, or one time measured from a different epoch than the other."
        }
        ReasonCode::ElevationOutOfRange => {
            "A maximum elevation is outside 0 to 90 degrees. In the flight layer elevation is \
             stored in hundredths of a degree, so the valid range is 0 to 9000. A negative \
             elevation means the satellite never rose above the horizon, which is not a pass."
        }
        ReasonCode::AzimuthOutOfRange => {
            "An azimuth is outside 0 to 360 degrees. In the flight layer azimuth is stored in \
             hundredths of a degree clockwise from north, so the valid range is 0 to 35999; \
             360 degrees is written as 0. Reduce the angle into that range before building the \
             pointing."
        }
        ReasonCode::DuplicateRequest => {
            "Two contact requests name the same satellite. The scheduler cannot tell which \
             priority, minimum contact and need to use, so it refuses the whole request. Combine \
             them into one request for that satellite."
        }
        ReasonCode::DuplicateStationRule => {
            "Two station rules name the same station, so the time it needs between passes is \
             ambiguous and \
             the whole request is refused. Give each station at most one rule. On the ground \
             this means two stations were given the same identifier."
        }
        ReasonCode::CapacityExceeded => {
            "The flight scheduler writes one decision per window into a slice supplied by the \
             caller, and the slice was too short. Nothing was scheduled. Give it at least as \
             many slots as there are windows. Onboard, where the slice has a fixed size, send \
             fewer windows or build the software with a larger buffer."
        }
        _ => NO_GUIDANCE,
    }
}

/// Finds a reason code from what a person might type: `MAAT-101`, `maat-101`, `MAAT101` or `101`.
///
/// ```
/// use maat_ground::flight::ReasonCode;
/// use maat_ground::parse_reason_code;
///
/// assert_eq!(parse_reason_code("MAAT-101"), Some(ReasonCode::StationBusy));
/// assert_eq!(parse_reason_code("maat-1"), Some(ReasonCode::Scheduled));
/// assert_eq!(parse_reason_code("999"), None);
/// ```
#[must_use]
pub fn parse_reason_code(text: &str) -> Option<ReasonCode> {
    let text = text.trim();
    let digits = text
        .get(..4)
        .filter(|prefix| prefix.eq_ignore_ascii_case("MAAT"))
        .map_or(text, |_| text[4..].trim_start_matches('-'));
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok().and_then(ReasonCode::from_code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_reason_code_has_its_own_guidance() {
        for reason in ReasonCode::ALL {
            let text = guidance(*reason);
            assert_ne!(text, NO_GUIDANCE, "{} has no guidance", reason.id());
            assert!(text.len() > 80, "{} guidance is too thin", reason.id());
            assert!(
                text.ends_with('.'),
                "{} guidance is not a sentence",
                reason.id()
            );
        }
    }

    #[test]
    fn no_two_codes_share_guidance() {
        for (i, a) in ReasonCode::ALL.iter().enumerate() {
            for b in &ReasonCode::ALL[i + 1..] {
                assert_ne!(guidance(*a), guidance(*b), "{} and {}", a.id(), b.id());
            }
        }
    }

    #[test]
    fn guidance_is_plain_single_paragraph_ascii() {
        // The same rule as the one-line summaries: safe for any terminal or log.
        for reason in ReasonCode::ALL {
            let text = guidance(*reason);
            assert!(
                text.bytes().all(|b| b == b' ' || b.is_ascii_graphic()),
                "{} guidance has a line break or non-ASCII text",
                reason.id()
            );
            assert!(
                !text.contains("  "),
                "{} guidance has a double space",
                reason.id()
            );
        }
    }

    #[test]
    fn rejections_say_what_would_change_the_outcome() {
        // Guidance for a dropped pass is only useful if it points at something to change.
        for reason in ReasonCode::ALL {
            if reason.class() == maat_flight::ReasonClass::Rejected {
                let text = guidance(*reason);
                assert!(
                    [
                        "To ",
                        "Add ",
                        "Check ",
                        "Shorten ",
                        "lower it",
                        "Nothing needs"
                    ]
                    .iter()
                    .any(|cue| text.contains(cue)),
                    "{} guidance offers no next step: {text}",
                    reason.id()
                );
            }
        }
    }

    #[test]
    fn codes_are_recognized_however_they_are_typed() {
        for text in [
            "MAAT-101",
            "maat-101",
            "Maat-101",
            "MAAT101",
            "101",
            " MAAT-101 ",
        ] {
            assert_eq!(
                parse_reason_code(text),
                Some(ReasonCode::StationBusy),
                "{text:?}"
            );
        }
        assert_eq!(parse_reason_code("MAAT-001"), Some(ReasonCode::Scheduled));
        assert_eq!(parse_reason_code("1"), Some(ReasonCode::Scheduled));
    }

    #[test]
    fn every_code_round_trips_through_its_identifier() {
        for reason in ReasonCode::ALL {
            assert_eq!(parse_reason_code(reason.id()), Some(*reason));
        }
    }

    #[test]
    fn things_that_are_not_codes_are_refused() {
        for text in [
            "",
            "MAAT",
            "MAAT-",
            "MAAT-abc",
            "MAAT-999",
            "0",
            "-101",
            "+101",
            "MAAT--",
            "101x",
            "é101",
            "MAA",
            "99999999999999999999",
        ] {
            assert_eq!(parse_reason_code(text), None, "{text:?}");
        }
    }
}
