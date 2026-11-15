//! Ground stations.

use ephemerust::ObserverLocation;
use maat_flight::StationId;

use crate::error::GroundError;

/// Where a station is on the Earth.
///
/// The fields are named on purpose. Four bare `f64` arguments in a row would make it easy to
/// swap latitude and longitude without the compiler noticing; a struct literal makes each value
/// say what it is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StationSite {
    /// Geodetic latitude in degrees, positive north. Valid range: -90 to 90.
    pub latitude_deg: f64,
    /// Geodetic longitude in degrees, positive east. Valid range: -180 to 180.
    pub longitude_deg: f64,
    /// Height above the WGS84 ellipsoid in metres. Valid range: -500 to 10 000.
    pub altitude_m: f64,
}

/// How fast a station's antenna mount can turn about each axis.
///
/// A steerable antenna has to follow the satellite across the sky. A pass that goes almost
/// straight overhead needs a very fast turn in azimuth near the top (the satellite swings from
/// one side of the sky to the other in seconds), and a mount that cannot keep up loses the link.
/// Stations that set a limit have such passes rejected with `MAAT-105`.
///
/// For scale: a common amateur rotator turns about 6 degrees per second in azimuth and 2 to 3 in
/// elevation, and the ISS needs more than 6 degrees per second in azimuth only on passes that
/// peak above roughly 80 degrees.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SlewLimits {
    /// Fastest turn about the vertical axis, in degrees per second. Valid range: 0.01 to 180.
    pub azimuth_deg_per_s: f64,
    /// Fastest turn about the horizontal axis, in degrees per second. Valid range: 0.01 to 180.
    pub elevation_deg_per_s: f64,
}

/// A validated ground station: an identifier, a name, a site, an elevation mask and, optionally,
/// slew limits.
///
/// A `GroundStation` can only be built through [`GroundStation::new`], so every value inside one
/// has been checked.
///
/// ```
/// use maat_ground::flight::StationId;
/// use maat_ground::{GroundStation, StationSite};
///
/// let station = GroundStation::new(
///     StationId::new(1),
///     "Seattle",
///     StationSite { latitude_deg: 47.6062, longitude_deg: -122.3321, altitude_m: 50.0 },
///     10.0,
/// )?;
/// assert_eq!(station.name(), "Seattle");
///
/// // Latitude and longitude swapped by mistake: caught, with an explanation.
/// let swapped = GroundStation::new(
///     StationId::new(2),
///     "Oops",
///     StationSite { latitude_deg: -122.3321, longitude_deg: 47.6062, altitude_m: 50.0 },
///     10.0,
/// );
/// assert!(swapped.is_err());
/// # Ok::<(), maat_ground::GroundError>(())
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct GroundStation {
    id: StationId,
    name: String,
    site: StationSite,
    min_elevation_deg: f64,
    slew_limits: Option<SlewLimits>,
}

/// The inclusive range a station field must fall in, with the text shown when it does not.
struct Range {
    field: &'static str,
    min: f64,
    max: f64,
    expected: &'static str,
}

const LATITUDE: Range = Range {
    field: "latitude_deg",
    min: -90.0,
    max: 90.0,
    expected: "-90 to 90 degrees (positive north)",
};
const LONGITUDE: Range = Range {
    field: "longitude_deg",
    min: -180.0,
    max: 180.0,
    expected: "-180 to 180 degrees (positive east; use negative values west of Greenwich)",
};
// Bounds chosen to catch unit mistakes (kilometres or feet given as metres) while admitting every
// real site: the Dead Sea shore is near -430 m and the highest observatories are below 6000 m.
const ALTITUDE: Range = Range {
    field: "altitude_m",
    min: -500.0,
    max: 10_000.0,
    expected: "-500 to 10000 metres above the WGS84 ellipsoid",
};
// 90 degrees is excluded: a mask at the zenith would mean no pass is ever visible.
const MIN_ELEVATION: Range = Range {
    field: "min_elevation_deg",
    min: 0.0,
    max: 89.0,
    expected: "0 to 89 degrees above the horizon",
};

// The lower bound is not zero: a mount that cannot turn at all cannot track anything, and a
// zero here is far more likely to be a missing value than a real one.
const SLEW_AZIMUTH: Range = Range {
    field: "slew azimuth_deg_per_s",
    min: 0.01,
    max: 180.0,
    expected: "0.01 to 180 degrees per second",
};
const SLEW_ELEVATION: Range = Range {
    field: "slew elevation_deg_per_s",
    min: 0.01,
    max: 180.0,
    expected: "0.01 to 180 degrees per second",
};

impl Range {
    /// Checks `value` against the range, naming the station in the error.
    fn check(&self, id: StationId, name: &str, value: f64) -> Result<(), GroundError> {
        // `contains` is false for NaN, and infinities fall outside every finite range, so this
        // one test also rejects every non-finite value.
        if (self.min..=self.max).contains(&value) {
            Ok(())
        } else {
            Err(GroundError::InvalidStation {
                station: id,
                name: name.to_owned(),
                field: self.field,
                value,
                expected: self.expected,
            })
        }
    }
}

impl GroundStation {
    /// Builds a ground station after checking every value.
    ///
    /// `min_elevation_deg` is the elevation mask: the satellite must be at least this far above
    /// the horizon for the station to use it. Five to ten degrees is typical; below that,
    /// terrain, buildings and atmosphere get in the way.
    ///
    /// # Errors
    ///
    /// - [`GroundError::EmptyStationName`] if `name` is empty or only whitespace.
    /// - [`GroundError::InvalidStation`] if a number is NaN, infinite or outside its range. The
    ///   error names the field, the value given and the range expected.
    pub fn new(
        id: StationId,
        name: impl Into<String>,
        site: StationSite,
        min_elevation_deg: f64,
    ) -> Result<Self, GroundError> {
        let name = name.into();
        if name.trim().is_empty() {
            return Err(GroundError::EmptyStationName { station: id });
        }

        LATITUDE.check(id, &name, site.latitude_deg)?;
        LONGITUDE.check(id, &name, site.longitude_deg)?;
        ALTITUDE.check(id, &name, site.altitude_m)?;
        MIN_ELEVATION.check(id, &name, min_elevation_deg)?;

        Ok(Self {
            id,
            name,
            site,
            min_elevation_deg,
            slew_limits: None,
        })
    }

    /// Returns the station with slew limits set, after checking them.
    ///
    /// A station without slew limits accepts every pass above its mask, which suits a fixed or
    /// omnidirectional antenna.
    ///
    /// # Errors
    ///
    /// [`GroundError::InvalidStation`] if either rate is NaN, infinite or outside 0.01 to 180
    /// degrees per second.
    ///
    /// ```
    /// use maat_ground::flight::StationId;
    /// use maat_ground::{GroundStation, SlewLimits, StationSite};
    ///
    /// let site = StationSite { latitude_deg: 47.6062, longitude_deg: -122.3321, altitude_m: 50.0 };
    /// let station = GroundStation::new(StationId::new(1), "Seattle", site, 10.0)?
    ///     .with_slew_limits(SlewLimits { azimuth_deg_per_s: 6.0, elevation_deg_per_s: 2.5 })?;
    /// assert_eq!(station.slew_limits().map(|l| l.azimuth_deg_per_s), Some(6.0));
    /// # Ok::<(), maat_ground::GroundError>(())
    /// ```
    pub fn with_slew_limits(mut self, limits: SlewLimits) -> Result<Self, GroundError> {
        SLEW_AZIMUTH.check(self.id, &self.name, limits.azimuth_deg_per_s)?;
        SLEW_ELEVATION.check(self.id, &self.name, limits.elevation_deg_per_s)?;
        self.slew_limits = Some(limits);
        Ok(self)
    }

    /// The station's identifier.
    #[must_use]
    pub fn id(&self) -> StationId {
        self.id
    }

    /// The station's name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Where the station is.
    #[must_use]
    pub fn site(&self) -> StationSite {
        self.site
    }

    /// The elevation mask in degrees.
    #[must_use]
    pub fn min_elevation_deg(&self) -> f64 {
        self.min_elevation_deg
    }

    /// The antenna's slew limits, or `None` if the station has none.
    #[must_use]
    pub fn slew_limits(&self) -> Option<SlewLimits> {
        self.slew_limits
    }

    /// The station's position in the form Ephemerust's pass prediction takes.
    #[must_use]
    pub fn observer(&self) -> ObserverLocation {
        ObserverLocation {
            latitude: self.site.latitude_deg,
            longitude: self.site.longitude_deg,
            elevation: self.site.altitude_m,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: StationId = StationId::new(7);
    const SEATTLE: StationSite = StationSite {
        latitude_deg: 47.6062,
        longitude_deg: -122.3321,
        altitude_m: 50.0,
    };

    /// Builds a station from `SEATTLE` with one field replaced, and returns the outcome.
    fn with(edit: impl FnOnce(&mut StationSite, &mut f64)) -> Result<GroundStation, GroundError> {
        let mut site = SEATTLE;
        let mut mask = 10.0;
        edit(&mut site, &mut mask);
        GroundStation::new(ID, "Test", site, mask)
    }

    /// Asserts that the result is an `InvalidStation` error blaming `field`.
    fn assert_invalid(result: Result<GroundStation, GroundError>, field: &str) {
        match result {
            Err(GroundError::InvalidStation {
                station,
                field: got,
                ..
            }) => {
                assert_eq!(station, ID);
                assert_eq!(got, field);
            }
            other => panic!("expected InvalidStation for {field}, got {other:?}"),
        }
    }

    #[test]
    fn a_valid_station_keeps_every_value() {
        let station = GroundStation::new(ID, "Seattle", SEATTLE, 10.0).unwrap();
        assert_eq!(station.id(), ID);
        assert_eq!(station.name(), "Seattle");
        assert_eq!(station.site(), SEATTLE);
        assert_eq!(station.min_elevation_deg(), 10.0);
    }

    #[test]
    fn observer_maps_fields_to_ephemerust_without_swapping_them() {
        let observer = GroundStation::new(ID, "Seattle", SEATTLE, 10.0)
            .unwrap()
            .observer();
        assert_eq!(observer.latitude, 47.6062);
        assert_eq!(observer.longitude, -122.3321);
        assert_eq!(observer.elevation, 50.0);
    }

    #[test]
    fn the_edges_of_every_range_are_accepted() {
        assert!(with(|s, _| s.latitude_deg = -90.0).is_ok());
        assert!(with(|s, _| s.latitude_deg = 90.0).is_ok());
        assert!(with(|s, _| s.longitude_deg = -180.0).is_ok());
        assert!(with(|s, _| s.longitude_deg = 180.0).is_ok());
        assert!(with(|s, _| s.altitude_m = -500.0).is_ok());
        assert!(with(|s, _| s.altitude_m = 10_000.0).is_ok());
        assert!(with(|_, mask| *mask = 0.0).is_ok());
        assert!(with(|_, mask| *mask = 89.0).is_ok());
    }

    #[test]
    fn values_just_outside_a_range_are_rejected() {
        assert_invalid(with(|s, _| s.latitude_deg = 90.001), "latitude_deg");
        assert_invalid(with(|s, _| s.latitude_deg = -90.001), "latitude_deg");
        assert_invalid(with(|s, _| s.longitude_deg = 180.001), "longitude_deg");
        assert_invalid(with(|s, _| s.longitude_deg = -180.001), "longitude_deg");
        assert_invalid(with(|s, _| s.altitude_m = -500.1), "altitude_m");
        assert_invalid(with(|s, _| s.altitude_m = 10_000.1), "altitude_m");
        assert_invalid(with(|_, mask| *mask = -0.1), "min_elevation_deg");
        assert_invalid(with(|_, mask| *mask = 89.1), "min_elevation_deg");
    }

    #[test]
    fn nan_and_infinity_are_rejected_in_every_field() {
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert_invalid(with(|s, _| s.latitude_deg = bad), "latitude_deg");
            assert_invalid(with(|s, _| s.longitude_deg = bad), "longitude_deg");
            assert_invalid(with(|s, _| s.altitude_m = bad), "altitude_m");
            assert_invalid(with(|_, mask| *mask = bad), "min_elevation_deg");
        }
    }

    #[test]
    fn a_longitude_given_as_0_to_360_is_rejected_with_a_hint() {
        // 237.67 E is the same meridian as 122.33 W. Rejecting it is deliberate: silently
        // accepting both conventions would hide a real mistake in a station file.
        let error = with(|s, _| s.longitude_deg = 237.6679).unwrap_err();
        assert!(
            error.to_string().contains("negative values west"),
            "{error}"
        );
    }

    #[test]
    fn swapped_latitude_and_longitude_are_caught_when_longitude_exceeds_90() {
        let swapped = StationSite {
            latitude_deg: SEATTLE.longitude_deg,
            longitude_deg: SEATTLE.latitude_deg,
            altitude_m: SEATTLE.altitude_m,
        };
        assert_invalid(
            GroundStation::new(ID, "Swapped", swapped, 10.0),
            "latitude_deg",
        );
    }

    #[test]
    fn the_altitude_bound_catches_some_unit_mistakes_but_not_all() {
        // The altitude bound cannot catch every unit mistake; this records what it does catch.
        assert!(with(|s, _| s.altitude_m = 5.0).is_ok());
        assert_invalid(with(|s, _| s.altitude_m = 16_500.0), "altitude_m");
    }

    #[test]
    fn empty_and_blank_names_are_rejected() {
        for name in ["", "   ", "\t\n"] {
            assert!(matches!(
                GroundStation::new(ID, name, SEATTLE, 10.0),
                Err(GroundError::EmptyStationName { station }) if station == ID
            ));
        }
    }

    // --- Slew limits ---------------------------------------------------------------------------

    fn with_slew(azimuth: f64, elevation: f64) -> Result<GroundStation, GroundError> {
        GroundStation::new(ID, "Test", SEATTLE, 10.0)?.with_slew_limits(SlewLimits {
            azimuth_deg_per_s: azimuth,
            elevation_deg_per_s: elevation,
        })
    }

    #[test]
    fn a_new_station_has_no_slew_limits() {
        let station = GroundStation::new(ID, "Test", SEATTLE, 10.0).unwrap();
        assert_eq!(station.slew_limits(), None);
    }

    #[test]
    fn slew_limits_are_kept_and_leave_the_rest_of_the_station_alone() {
        let station = with_slew(6.0, 2.5).unwrap();
        assert_eq!(
            station.slew_limits(),
            Some(SlewLimits {
                azimuth_deg_per_s: 6.0,
                elevation_deg_per_s: 2.5
            })
        );
        assert_eq!(station.name(), "Test");
        assert_eq!(station.site(), SEATTLE);
        assert_eq!(station.min_elevation_deg(), 10.0);
    }

    #[test]
    fn slew_limits_accept_the_edges_of_their_range() {
        assert!(with_slew(0.01, 0.01).is_ok());
        assert!(with_slew(180.0, 180.0).is_ok());
    }

    #[test]
    fn slew_limits_outside_their_range_name_the_axis() {
        assert_invalid(with_slew(0.0, 2.5), "slew azimuth_deg_per_s");
        assert_invalid(with_slew(-6.0, 2.5), "slew azimuth_deg_per_s");
        assert_invalid(with_slew(180.1, 2.5), "slew azimuth_deg_per_s");
        assert_invalid(with_slew(6.0, 0.0), "slew elevation_deg_per_s");
        assert_invalid(with_slew(6.0, 180.1), "slew elevation_deg_per_s");
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert_invalid(with_slew(bad, 2.5), "slew azimuth_deg_per_s");
            assert_invalid(with_slew(6.0, bad), "slew elevation_deg_per_s");
        }
    }

    #[test]
    fn the_first_bad_field_is_the_one_reported() {
        let everything_wrong = StationSite {
            latitude_deg: 200.0,
            longitude_deg: 500.0,
            altitude_m: 1e9,
        };
        assert_invalid(
            GroundStation::new(ID, "Bad", everything_wrong, 100.0),
            "latitude_deg",
        );
    }
}
