//! Ground-layer errors.
//!
//! Following the Ephemerust convention, an error says what was given, what was expected and,
//! where it helps, why.

use chrono::{DateTime, Utc};
use ephemerust::AstroError;
use maat_flight::{ReasonCode, SatelliteId, ScheduleError, StationId, WindowError};
use thiserror::Error;

/// Everything that can go wrong in the ground layer.
///
/// Errors that come from Ephemerust keep the original error as their
/// [`source`](std::error::Error::source), and [`GroundError::hint`] passes on its corrective hint.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum GroundError {
    /// The flight layer refused a contact window. The message starts with its `MAAT-2xx` code.
    #[error(transparent)]
    Window(#[from] WindowError),

    /// The scheduler refused the request. The message starts with its `MAAT` code.
    #[error(transparent)]
    Schedule(#[from] ScheduleError),

    /// The optimal scheduler's solver did not return a usable plan.
    #[error(
        "the optimal scheduler could not produce a plan: {message}; the greedy scheduler does \
         not depend on the solver and can be used instead"
    )]
    Solver {
        /// What went wrong, as reported by the solver or by the checks on its answer.
        message: String,
    },

    /// A ground station was given a number that is not finite or is outside its valid range.
    #[error("invalid ground station {station} \"{name}\": {field} is {value}, expected {expected}")]
    InvalidStation {
        /// The station's identifier.
        station: StationId,
        /// The station's name as given.
        name: String,
        /// Which field was wrong, such as `"latitude_deg"`.
        field: &'static str,
        /// The value that was given.
        value: f64,
        /// A description of the accepted range, including the unit.
        expected: &'static str,
    },

    /// A ground station was given an empty name.
    #[error(
        "invalid ground station {station}: the name is empty; give each station a name so that \
         schedules and error messages can refer to it"
    )]
    EmptyStationName {
        /// The station's identifier.
        station: StationId,
    },

    /// A predicted pass reported a maximum elevation that cannot be stored.
    #[error(
        "{id}: the pass has a maximum elevation of {degrees} degrees, expected a finite value \
         from 0 to 90; a pass that never rises above the horizon is not a contact",
        id = ReasonCode::ElevationOutOfRange.id()
    )]
    InvalidElevation {
        /// The elevation that was reported, in degrees.
        degrees: f64,
    },

    /// A flight timestamp is outside the range of calendar dates `chrono` can represent.
    #[error(
        "timestamp {unix_seconds} s since the Unix epoch is outside the range of calendar dates \
         that can be represented (roughly the years -262000 to +262000)"
    )]
    TimeOutOfRange {
        /// The timestamp, in whole seconds since the Unix epoch.
        unix_seconds: i64,
    },

    /// The text of a satellite catalog could not be read as OMM.
    #[error("could not read the satellite catalog: {source}")]
    Catalog {
        /// The Ephemerust error that explains what is wrong with the text.
        #[source]
        source: AstroError,
    },

    /// An element set was read but SGP4 cannot be started from it.
    #[error("the element set for {satellite} cannot be propagated: {source}")]
    Elements {
        /// The satellite's name, or its catalog number when it has no name.
        satellite: String,
        /// The Ephemerust error.
        #[source]
        source: AstroError,
    },

    /// A SatNOGS transmitter identifier is not in the form SatNOGS DB gives them.
    #[error(
        "the SatNOGS transmitter given for satellite {satellite} is {given:?}, expected the \
         22 letters and digits that SatNOGS DB shows as the transmitter's UUID"
    )]
    InvalidTransmitter {
        /// The satellite the transmitter was given for.
        satellite: SatelliteId,
        /// What was given.
        given: String,
    },

    /// An element set describes an orbit that passes through the Earth.
    #[error(
        "the element set for {satellite} describes an orbit whose lowest point is {depth:.0} km \
         below the Earth's surface (mean motion {mean_motion_rev_per_day} rev/day, eccentricity \
         {eccentricity}); nothing can be in that orbit, so the element set is wrong or corrupted",
        depth = -perigee_altitude_km
    )]
    ImpossibleOrbit {
        /// The satellite's name, or its catalog number when it has no name.
        satellite: String,
        /// Height of the orbit's lowest point above the surface, in kilometres. Negative here.
        perigee_altitude_km: f64,
        /// The mean motion from the element set, in revolutions per day.
        mean_motion_rev_per_day: f64,
        /// The eccentricity from the element set.
        eccentricity: f64,
    },

    /// An OMM without a catalog number was loaded without being given an identifier.
    #[error(
        "\"{name}\" has no catalog number (NORAD_CAT_ID), so there is no identifier to schedule \
         it under; an object that is not cataloged yet needs one assigned with \
         `Satellite::from_omm_with_id`"
    )]
    MissingCatalogNumber {
        /// The object's name, or a placeholder when it has none.
        name: String,
    },

    /// Two satellites share an identifier.
    #[error(
        "satellite {id} appears twice (\"{first}\" and \"{second}\"); each satellite needs a \
         unique identifier, so a catalog that lists several epochs for one object must be reduced \
         to one element set per object"
    )]
    DuplicateSatellite {
        /// The identifier that was used twice.
        id: SatelliteId,
        /// The name of the first satellite with this identifier.
        first: String,
        /// The name of the second.
        second: String,
    },

    /// Two ground stations share an identifier.
    #[error(
        "station {id} appears twice (\"{first}\" and \"{second}\"); each station needs a unique \
         identifier, because a contact window refers to its station by number"
    )]
    DuplicateStation {
        /// The identifier that was used twice.
        id: StationId,
        /// The name of the first station with this identifier.
        first: String,
        /// The name of the second.
        second: String,
    },

    /// The search window has no length.
    #[error(
        "the search window ends at {end}, which is not after its start at {start}; the end must \
         be later than the start"
    )]
    InvalidSearchWindow {
        /// The start that was given.
        start: DateTime<Utc>,
        /// The end that was given.
        end: DateTime<Utc>,
    },

    /// Pass prediction failed for one satellite over one station.
    #[error(
        "pass prediction failed for satellite {satellite} over station {station}: {source}; an \
         element set far from its epoch, or one for an object that has since decayed, is the \
         usual cause"
    )]
    PassPrediction {
        /// The satellite being searched.
        satellite: SatelliteId,
        /// The station it was being searched against.
        station: StationId,
        /// The Ephemerust error.
        #[source]
        source: AstroError,
    },
}

impl GroundError {
    /// The stable `MAAT-xxx` reason code for this error, when it has one.
    ///
    /// Errors that describe a contact window carry a code, because they explain why a pass is
    /// missing from a plan. Errors about ground-only configuration or input (a bad station, an
    /// unreadable catalog, a date out of range) do not.
    #[must_use]
    pub fn reason(&self) -> Option<ReasonCode> {
        match self {
            Self::Window(error) => Some(error.reason()),
            Self::Schedule(error) => Some(error.reason()),
            Self::InvalidElevation { .. } => Some(ReasonCode::ElevationOutOfRange),
            Self::InvalidStation { .. }
            | Self::EmptyStationName { .. }
            | Self::Solver { .. }
            | Self::TimeOutOfRange { .. }
            | Self::Catalog { .. }
            | Self::Elements { .. }
            | Self::ImpossibleOrbit { .. }
            | Self::InvalidTransmitter { .. }
            | Self::MissingCatalogNumber { .. }
            | Self::DuplicateSatellite { .. }
            | Self::DuplicateStation { .. }
            | Self::InvalidSearchWindow { .. }
            | Self::PassPrediction { .. } => None,
        }
    }

    /// A short, actionable hint for display on its own `Hint:` line, when Ephemerust offers one.
    ///
    /// Ephemerust attaches hints to element-set errors (a missing OMM keyword, a value out of
    /// range). This passes them through so a front end can show them without unwrapping the
    /// error chain itself.
    #[must_use]
    pub fn hint(&self) -> Option<&'static str> {
        match self {
            Self::Catalog { source }
            | Self::Elements { source, .. }
            | Self::PassPrediction { source, .. } => source.hint(),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use maat_flight::Timestamp;
    use std::error::Error as _;

    #[test]
    fn window_errors_pass_through_with_their_code_and_message() {
        let flight = WindowError::LosNotAfterAos {
            aos: Timestamp::from_unix_seconds(10),
            los: Timestamp::from_unix_seconds(5),
        };
        let ground = GroundError::from(flight);
        assert_eq!(ground.reason(), Some(ReasonCode::LosNotAfterAos));
        // `#[error(transparent)]` forwards the flight message unchanged.
        assert_eq!(ground.to_string(), flight.to_string());
    }

    #[test]
    fn invalid_elevation_carries_the_elevation_code() {
        let error = GroundError::InvalidElevation { degrees: -3.5 };
        assert_eq!(error.reason(), Some(ReasonCode::ElevationOutOfRange));
        let message = error.to_string();
        assert!(message.starts_with("MAAT-203: "), "{message}");
        assert!(message.contains("-3.5"), "{message}");
    }

    #[test]
    fn configuration_errors_have_no_reason_code() {
        let station = GroundError::InvalidStation {
            station: StationId::new(4),
            name: "Nowhere".to_owned(),
            field: "latitude_deg",
            value: 123.0,
            expected: "-90 to 90 degrees",
        };
        assert_eq!(station.reason(), None);
        assert_eq!(
            station.to_string(),
            "invalid ground station 4 \"Nowhere\": latitude_deg is 123, expected -90 to 90 degrees"
        );

        let unnamed = GroundError::EmptyStationName {
            station: StationId::new(4),
        };
        assert_eq!(unnamed.reason(), None);
        assert!(unnamed.to_string().contains("station 4"));

        let time = GroundError::TimeOutOfRange {
            unix_seconds: i64::MAX,
        };
        assert_eq!(time.reason(), None);
        assert!(time.to_string().contains(&i64::MAX.to_string()));
    }

    #[test]
    fn duplicate_errors_name_the_identifier_and_both_owners() {
        let satellites = GroundError::DuplicateSatellite {
            id: SatelliteId::new(25544),
            first: "ISS (ZARYA)".to_owned(),
            second: "ISS".to_owned(),
        }
        .to_string();
        assert!(satellites.contains("satellite 25544"), "{satellites}");
        assert!(satellites.contains("\"ISS (ZARYA)\"") && satellites.contains("\"ISS\""));

        let stations = GroundError::DuplicateStation {
            id: StationId::new(3),
            first: "North".to_owned(),
            second: "South".to_owned(),
        }
        .to_string();
        assert!(stations.contains("station 3"), "{stations}");
        assert!(stations.contains("\"North\"") && stations.contains("\"South\""));
    }

    #[test]
    fn ephemerust_errors_keep_their_source_and_hint() {
        // A real Ephemerust error: an OMM with no EPOCH.
        let source = ephemerust::Omm::parse(r#"{"MEAN_MOTION": 15.5}"#).unwrap_err();
        let expected_hint = source.hint();
        assert!(
            expected_hint.is_some(),
            "Ephemerust hints at the missing keyword"
        );

        let error = GroundError::Catalog { source };
        assert_eq!(error.hint(), expected_hint);
        assert_eq!(error.reason(), None);
        assert!(
            error.source().is_some(),
            "the Ephemerust error is the source"
        );
        let message = error.to_string();
        assert!(
            message.starts_with("could not read the satellite catalog: "),
            "{message}"
        );
        assert!(message.contains("EPOCH"), "{message}");
    }

    #[test]
    fn errors_without_an_ephemerust_source_have_no_hint() {
        assert_eq!(GroundError::InvalidElevation { degrees: 99.0 }.hint(), None);
        assert_eq!(
            GroundError::MissingCatalogNumber {
                name: "X".to_owned()
            }
            .hint(),
            None
        );
    }
}
