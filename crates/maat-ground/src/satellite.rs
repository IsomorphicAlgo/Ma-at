//! Satellites: an identifier, a name and an element set ready to propagate.
//!
//! Element sets come in as **OMM** (Orbit Mean-Elements Message) or as a classic **TLE**. OMM is
//! the main route: the public catalog ran out of five-digit numbers in July 2026, and an object
//! numbered 100000 or above cannot be written as a plain TLE. Ephemerust reads OMM in JSON, KVN,
//! XML and CSV and detects which one it was given, so nothing here needs to be told the format.

use std::fmt;

use chrono::{DateTime, Utc};
use ephemerust::{Omm, Propagator, Tle};
use maat_flight::SatelliteId;

use crate::error::GroundError;

/// One satellite, ready for pass prediction.
///
/// Building a `Satellite` initializes SGP4 from its element set once. That work is then shared by
/// every ground station the satellite is checked against. It also means an element set that
/// cannot be propagated is refused here, when it is loaded, and not later in the middle of a
/// search.
///
/// ```
/// use ephemerust::Tle;
/// use maat_ground::Satellite;
///
/// let tle = Tle::parse(
///     "ISS (ZARYA)\n\
///      1 25544U 98067A   20194.88612269 -.00002218  00000-0 -31515-4 0  9992\n\
///      2 25544  51.6461 221.2784 0001413  89.1723 280.4612 15.49507896236008",
/// )?;
/// let iss = Satellite::from_tle(&tle)?;
/// assert_eq!(iss.id().get(), 25544);
/// assert_eq!(iss.name(), "ISS (ZARYA)");
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub struct Satellite {
    id: SatelliteId,
    name: String,
    epoch: DateTime<Utc>,
    propagator: Propagator,
}

impl Satellite {
    /// Builds a satellite from an OMM, using its catalog number as the identifier.
    ///
    /// # Errors
    ///
    /// - [`GroundError::MissingCatalogNumber`] if the OMM has no `NORAD_CAT_ID`. An operator may
    ///   publish elements for an object that is not cataloged yet; give such an object an
    ///   identifier with [`Satellite::from_omm_with_id`].
    /// - [`GroundError::Elements`] if SGP4 cannot be started from the elements.
    /// - [`GroundError::ImpossibleOrbit`] if the elements describe an orbit that passes through
    ///   the Earth.
    pub fn from_omm(omm: &Omm) -> Result<Self, GroundError> {
        let Some(number) = omm.catalog_number else {
            return Err(GroundError::MissingCatalogNumber {
                name: display_name(omm.name.as_deref(), None),
            });
        };
        Self::from_omm_with_id(SatelliteId::new(number), omm)
    }

    /// Builds a satellite from an OMM under an identifier chosen by the caller.
    ///
    /// Use this for an object without a catalog number, or to schedule under a mission's own
    /// numbering.
    ///
    /// # Errors
    ///
    /// [`GroundError::Elements`] or [`GroundError::ImpossibleOrbit`], as for
    /// [`Satellite::from_omm`].
    pub fn from_omm_with_id(id: SatelliteId, omm: &Omm) -> Result<Self, GroundError> {
        let name = display_name(omm.name.as_deref(), Some(id));
        let propagator = Propagator::from_omm(omm).map_err(|source| GroundError::Elements {
            satellite: name.clone(),
            source,
        })?;
        check_orbit_clears_the_earth(&name, omm.mean_motion, omm.eccentricity)?;
        Ok(Self {
            id,
            name,
            epoch: omm.epoch,
            propagator,
        })
    }

    /// Builds a satellite from a two-line element set, using its catalog number as the
    /// identifier.
    ///
    /// # Errors
    ///
    /// [`GroundError::Elements`] or [`GroundError::ImpossibleOrbit`], as for
    /// [`Satellite::from_omm`].
    pub fn from_tle(tle: &Tle) -> Result<Self, GroundError> {
        let id = SatelliteId::new(tle.catalog_number);
        let name = display_name(tle.name.as_deref(), Some(id));
        let propagator = Propagator::new(tle).map_err(|source| GroundError::Elements {
            satellite: name.clone(),
            source,
        })?;
        check_orbit_clears_the_earth(&name, tle.mean_motion, tle.eccentricity)?;
        Ok(Self {
            id,
            name,
            epoch: tle.epoch,
            propagator,
        })
    }

    /// The satellite's identifier.
    #[must_use]
    pub fn id(&self) -> SatelliteId {
        self.id
    }

    /// The satellite's name. An element set without a name gets `"catalog <number>"`.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The epoch of the element set: the instant its orbit description is valid for.
    ///
    /// Prediction accuracy falls off with distance from the epoch. For a low orbit, an element
    /// set more than a week or two old is of little use.
    #[must_use]
    pub fn epoch(&self) -> DateTime<Utc> {
        self.epoch
    }

    /// The initialized SGP4 propagator.
    pub(crate) fn propagator(&self) -> &Propagator {
        &self.propagator
    }
}

// `Propagator` does not implement `Debug`, so this is written out by hand.
impl fmt::Debug for Satellite {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Satellite")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("epoch", &self.epoch)
            .finish_non_exhaustive()
    }
}

/// Reads every satellite from OMM text, in any of the four OMM formats.
///
/// The text may hold one message or many (a provider's answer to a group query, a saved
/// catalog). The satellites come back in the order they appear.
///
/// # Errors
///
/// - [`GroundError::Catalog`] if the text is not OMM or a message in it is invalid. The message
///   names the failing entry, and [`GroundError::hint`] says how to fix it.
/// - [`GroundError::MissingCatalogNumber`] if an entry has no catalog number.
/// - [`GroundError::Elements`] if an entry cannot be propagated.
/// - [`GroundError::DuplicateSatellite`] if two entries share a catalog number.
///
/// ```
/// use maat_ground::satellites_from_omm;
///
/// // CSV, as CelesTrak serves it. The second object's number does not fit in a TLE.
/// let csv = "\
/// OBJECT_NAME,OBJECT_ID,EPOCH,MEAN_MOTION,ECCENTRICITY,INCLINATION,RA_OF_ASC_NODE,ARG_OF_PERICENTER,MEAN_ANOMALY,EPHEMERIS_TYPE,CLASSIFICATION_TYPE,NORAD_CAT_ID,ELEMENT_SET_NO,REV_AT_EPOCH,BSTAR,MEAN_MOTION_DOT,MEAN_MOTION_DDOT
/// EXAMPLE-A,2026-150A,2026-08-01T12:00:00.000000,15.49,.0002,51.64,221.0,89.0,280.0,0,U,65001,999,42,.3E-4,.2E-4,0
/// EXAMPLE-B,2026-150B,2026-08-01T12:00:00.000000,15.2,.0012,97.5,120.0,45.0,315.0,0,U,100123,999,42,.1E-3,.1E-4,0
/// ";
/// let satellites = satellites_from_omm(csv)?;
/// assert_eq!(satellites.len(), 2);
/// assert_eq!(satellites[1].id().get(), 100_123);
/// # Ok::<(), maat_ground::GroundError>(())
/// ```
pub fn satellites_from_omm(text: &str) -> Result<Vec<Satellite>, GroundError> {
    let messages = Omm::parse_all(text).map_err(|source| GroundError::Catalog { source })?;
    let satellites = messages
        .iter()
        .map(Satellite::from_omm)
        .collect::<Result<Vec<_>, _>>()?;
    check_unique_satellites(&satellites)?;
    Ok(satellites)
}

/// Fails if two satellites share an identifier.
pub(crate) fn check_unique_satellites(satellites: &[Satellite]) -> Result<(), GroundError> {
    for (index, later) in satellites.iter().enumerate() {
        if let Some(earlier) = satellites[..index].iter().find(|s| s.id == later.id) {
            return Err(GroundError::DuplicateSatellite {
                id: later.id,
                first: earlier.name.clone(),
                second: later.name.clone(),
            });
        }
    }
    Ok(())
}

/// Earth's gravitational parameter in km^3/s^2 (WGS84).
const EARTH_MU_KM3_S2: f64 = 398_600.441_8;
/// Earth's equatorial radius in km (WGS84).
const EARTH_RADIUS_KM: f64 = 6_378.137;
const SECONDS_PER_DAY: f64 = 86_400.0;

/// Height of an orbit's lowest point above the Earth's surface, in kilometres.
///
/// Kepler's third law gives the semi-major axis from the mean motion, `a = (mu / n^2)^(1/3)`,
/// and the lowest point is at `a * (1 - e)` from the Earth's centre. An element set's mean
/// motion is an SGP4 *mean* element, so this is good to a few kilometres, which is all a sanity
/// check needs.
fn perigee_altitude_km(mean_motion_rev_per_day: f64, eccentricity: f64) -> f64 {
    let radians_per_second = mean_motion_rev_per_day * std::f64::consts::TAU / SECONDS_PER_DAY;
    let semi_major_axis_km = (EARTH_MU_KM3_S2 / radians_per_second.powi(2)).cbrt();
    semi_major_axis_km * (1.0 - eccentricity) - EARTH_RADIUS_KM
}

/// Refuses an element set whose orbit passes through the Earth.
///
/// SGP4 itself accepts such elements and returns positions below the surface, so a corrupted
/// element set would otherwise produce an empty or nonsensical list of passes without any error.
/// The test is deliberately loose: an orbit with a perigee of 100 km is about to decay but is
/// physically real, and is let through.
fn check_orbit_clears_the_earth(
    satellite: &str,
    mean_motion_rev_per_day: f64,
    eccentricity: f64,
) -> Result<(), GroundError> {
    let perigee_altitude_km = perigee_altitude_km(mean_motion_rev_per_day, eccentricity);
    // Written so that a NaN altitude is refused too.
    if perigee_altitude_km >= 0.0 {
        Ok(())
    } else {
        Err(GroundError::ImpossibleOrbit {
            satellite: satellite.to_owned(),
            perigee_altitude_km,
            mean_motion_rev_per_day,
            eccentricity,
        })
    }
}

/// The name to show for an object: its own name, else its number, else a placeholder.
fn display_name(name: Option<&str>, id: Option<SatelliteId>) -> String {
    match (name.map(str::trim).filter(|n| !n.is_empty()), id) {
        (Some(name), _) => name.to_owned(),
        (None, Some(id)) => format!("catalog {id}"),
        (None, None) => "unnamed object".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    const ISS_TLE: &str = "ISS (ZARYA)\n\
        1 25544U 98067A   20194.88612269 -.00002218  00000-0 -31515-4 0  9992\n\
        2 25544  51.6461 221.2784 0001413  89.1723 280.4612 15.49507896236008";

    /// One OMM message as JSON, with the pieces the tests vary left as parameters.
    fn omm_json(name: Option<&str>, catalog: Option<u32>, mean_motion: f64) -> String {
        let mut fields = vec![
            r#""EPOCH":"2026-08-01T12:00:00""#.to_owned(),
            format!(r#""MEAN_MOTION":{mean_motion}"#),
            r#""ECCENTRICITY":0.0012"#.to_owned(),
            r#""INCLINATION":97.5"#.to_owned(),
            r#""RA_OF_ASC_NODE":120.0"#.to_owned(),
            r#""ARG_OF_PERICENTER":45.0"#.to_owned(),
            r#""MEAN_ANOMALY":315.0"#.to_owned(),
            r#""BSTAR":0.0001"#.to_owned(),
        ];
        if let Some(name) = name {
            fields.push(format!(r#""OBJECT_NAME":"{name}""#));
        }
        if let Some(catalog) = catalog {
            fields.push(format!(r#""NORAD_CAT_ID":{catalog}"#));
        }
        format!("{{{}}}", fields.join(","))
    }

    fn omm(name: Option<&str>, catalog: Option<u32>) -> Omm {
        Omm::parse(&omm_json(name, catalog, 15.2)).unwrap()
    }

    #[test]
    fn from_tle_takes_identifier_name_and_epoch_from_the_element_set() {
        let tle = Tle::parse(ISS_TLE).unwrap();
        let satellite = Satellite::from_tle(&tle).unwrap();
        assert_eq!(satellite.id(), SatelliteId::new(25544));
        assert_eq!(satellite.name(), "ISS (ZARYA)");
        assert_eq!(satellite.epoch(), tle.epoch);
    }

    #[test]
    fn from_omm_takes_identifier_name_and_epoch_from_the_message() {
        let satellite = Satellite::from_omm(&omm(Some("EXAMPLE-SAT"), Some(100_123))).unwrap();
        assert_eq!(satellite.id(), SatelliteId::new(100_123));
        assert_eq!(satellite.name(), "EXAMPLE-SAT");
        assert_eq!(
            satellite.epoch(),
            Utc.with_ymd_and_hms(2026, 8, 1, 12, 0, 0).unwrap()
        );
    }

    #[test]
    fn a_nine_digit_catalog_number_survives_as_the_identifier() {
        // The reason OMM exists: numbers a TLE cannot hold.
        let satellite = Satellite::from_omm(&omm(Some("FAR FUTURE"), Some(799_500_123))).unwrap();
        assert_eq!(satellite.id().get(), 799_500_123);
    }

    #[test]
    fn an_unnamed_object_is_named_after_its_catalog_number() {
        let satellite = Satellite::from_omm(&omm(None, Some(100_123))).unwrap();
        assert_eq!(satellite.name(), "catalog 100123");
    }

    #[test]
    fn an_omm_without_a_catalog_number_is_refused_with_the_way_out() {
        let error = Satellite::from_omm(&omm(Some("PRE-LAUNCH"), None)).unwrap_err();
        assert!(matches!(
            &error,
            GroundError::MissingCatalogNumber { name } if name == "PRE-LAUNCH"
        ));
        assert!(error.to_string().contains("from_omm_with_id"), "{error}");
    }

    #[test]
    fn from_omm_with_id_schedules_an_uncataloged_object() {
        let satellite =
            Satellite::from_omm_with_id(SatelliteId::new(7), &omm(Some("PRE-LAUNCH"), None))
                .unwrap();
        assert_eq!(satellite.id(), SatelliteId::new(7));
        assert_eq!(satellite.name(), "PRE-LAUNCH");
    }

    #[test]
    fn from_omm_with_id_overrides_the_catalog_number() {
        let satellite =
            Satellite::from_omm_with_id(SatelliteId::new(7), &omm(Some("X"), Some(100_123)))
                .unwrap();
        assert_eq!(satellite.id(), SatelliteId::new(7));
    }

    #[test]
    fn elements_sgp4_cannot_start_from_are_refused_at_load_time() {
        // The OMM parser never lets an eccentricity of 1.5 through, but `Omm` has public fields,
        // so a caller can still hand one over. SGP4 refuses it, and the refusal must surface.
        let mut open_orbit = omm(Some("ESCAPEE"), Some(1));
        open_orbit.eccentricity = 1.5;
        let error = Satellite::from_omm(&open_orbit).unwrap_err();
        assert!(
            matches!(&error, GroundError::Elements { satellite, .. } if satellite == "ESCAPEE"),
            "{error:?}"
        );
        assert!(
            error
                .to_string()
                .starts_with("the element set for ESCAPEE cannot be propagated"),
            "{error}"
        );
    }

    // --- Orbit sanity check --------------------------------------------------------------------

    #[test]
    fn perigee_altitude_matches_known_orbits() {
        // Cross-check against orbits whose heights are common knowledge. Tolerance 15 km: the
        // mean motion is an SGP4 mean element, not an exact Keplerian one.
        // ISS: 15.495 rev/day, nearly circular, about 420 km.
        let iss = perigee_altitude_km(15.495_078_96, 0.000_141_3);
        assert!((iss - 420.0).abs() < 15.0, "ISS perigee {iss} km");
        // Geostationary: one revolution per sidereal day, 35 786 km.
        let geo = perigee_altitude_km(1.002_737_9, 0.0);
        assert!((geo - 35_786.0).abs() < 15.0, "GEO altitude {geo} km");
    }

    #[test]
    fn perigee_falls_as_eccentricity_rises() {
        let circular = perigee_altitude_km(15.2, 0.0);
        let stretched = perigee_altitude_km(15.2, 0.05);
        assert!(stretched < circular);
        // The drop is a * e: about 6880 km * 0.05 = 344 km.
        assert!(((circular - stretched) - 344.0).abs() < 5.0);
    }

    #[test]
    fn an_orbit_inside_the_earth_is_refused_with_its_depth() {
        // 20 revolutions a day is a 72-minute orbit, about 650 km underground. SGP4 accepts it
        // and reports positions below the surface, so Ma'at has to catch it.
        let underground = Omm::parse(&omm_json(Some("MOLE"), Some(1), 20.0)).unwrap();
        let error = Satellite::from_omm(&underground).unwrap_err();
        assert!(
            matches!(
                &error,
                GroundError::ImpossibleOrbit { satellite, perigee_altitude_km, .. }
                    if satellite == "MOLE" && (-700.0..-600.0).contains(perigee_altitude_km)
            ),
            "{error:?}"
        );
        let message = error.to_string();
        assert!(message.contains("MOLE") && message.contains("below the Earth's surface"));
        assert!(message.contains("20 rev/day"), "{message}");
    }

    #[test]
    fn a_stretched_orbit_that_dips_into_the_earth_is_refused() {
        // A sensible mean motion with an eccentricity that drives perigee underground.
        let mut dipping = omm(Some("DIPPER"), Some(1));
        dipping.eccentricity = 0.3;
        let error = Satellite::from_omm(&dipping).unwrap_err();
        assert!(
            matches!(error, GroundError::ImpossibleOrbit { .. }),
            "{error:?}"
        );
    }

    #[test]
    fn a_low_but_real_orbit_is_accepted() {
        // 16.3 revolutions a day is a perigee near 190 km: about to decay, but real.
        assert!(
            Satellite::from_omm(&Omm::parse(&omm_json(Some("LOW"), Some(1), 16.3)).unwrap())
                .is_ok()
        );
        // And the ordinary cases: low orbit, geostationary.
        assert!(Satellite::from_omm(&omm(Some("LEO"), Some(2))).is_ok());
        let geo = omm_json(Some("GEO"), Some(3), 1.0027).replace("97.5", "0.05");
        assert!(Satellite::from_omm(&Omm::parse(&geo).unwrap()).is_ok());
    }

    #[test]
    fn the_orbit_check_applies_to_tles_too() {
        let mut tle = Tle::parse(ISS_TLE).unwrap();
        tle.mean_motion = 20.0;
        // SGP4 accepts these elements too, so the check is what has to refuse them.
        let error = Satellite::from_tle(&tle).unwrap_err();
        assert!(
            matches!(error, GroundError::ImpossibleOrbit { .. }),
            "{error:?}"
        );
    }

    #[test]
    fn debug_output_shows_identity_without_the_propagator() {
        let text = format!(
            "{:?}",
            Satellite::from_omm(&omm(Some("X"), Some(5))).unwrap()
        );
        assert!(
            text.contains("Satellite") && text.contains("\"X\""),
            "{text}"
        );
    }

    // --- satellites_from_omm -------------------------------------------------------------------

    #[test]
    fn a_catalog_keeps_its_order() {
        let text = format!(
            "[{},{},{}]",
            omm_json(Some("C"), Some(300), 15.2),
            omm_json(Some("A"), Some(100), 15.2),
            omm_json(Some("B"), Some(200), 15.2)
        );
        let names: Vec<String> = satellites_from_omm(&text)
            .unwrap()
            .iter()
            .map(|s| s.name().to_owned())
            .collect();
        assert_eq!(names, ["C", "A", "B"]);
    }

    #[test]
    fn a_duplicate_catalog_number_is_refused_and_both_entries_are_named() {
        let text = format!(
            "[{},{}]",
            omm_json(Some("MONDAY"), Some(100), 15.2),
            omm_json(Some("TUESDAY"), Some(100), 15.2)
        );
        let error = satellites_from_omm(&text).unwrap_err();
        assert!(matches!(
            &error,
            GroundError::DuplicateSatellite { id, first, second }
                if id.get() == 100 && first == "MONDAY" && second == "TUESDAY"
        ));
    }

    #[test]
    fn text_that_is_not_omm_is_a_catalog_error_with_a_hint() {
        // A TLE handed to the OMM loader: a likely mistake, so the hint must point the way.
        let error = satellites_from_omm(ISS_TLE).unwrap_err();
        assert!(matches!(error, GroundError::Catalog { .. }), "{error:?}");
        assert!(error.hint().is_some_and(|hint| hint.contains("TLE")));
    }

    #[test]
    fn an_invalid_entry_in_a_catalog_is_located_by_position_and_name() {
        let bad = omm_json(Some("BROKEN"), Some(200), 15.2).replace("0.0012", "1.5");
        let text = format!("[{},{}]", omm_json(Some("FINE"), Some(100), 15.2), bad);
        let message = satellites_from_omm(&text).unwrap_err().to_string();
        assert!(message.contains("message 2"), "{message}");
        assert!(message.contains("BROKEN"), "{message}");
        assert!(message.contains("ECCENTRICITY"), "{message}");
    }

    #[test]
    fn an_empty_catalog_is_an_error_not_an_empty_list() {
        // "No satellites" from a provider usually means the query matched nothing; scheduling
        // against it silently would hide that.
        let error = satellites_from_omm("[]").unwrap_err();
        assert!(matches!(error, GroundError::Catalog { .. }), "{error:?}");
    }

    #[test]
    fn check_unique_accepts_distinct_identifiers() {
        let satellites = vec![
            Satellite::from_omm(&omm(Some("A"), Some(1))).unwrap(),
            Satellite::from_omm(&omm(Some("B"), Some(2))).unwrap(),
        ];
        assert!(check_unique_satellites(&satellites).is_ok());
        assert!(check_unique_satellites(&[]).is_ok());
    }
}
