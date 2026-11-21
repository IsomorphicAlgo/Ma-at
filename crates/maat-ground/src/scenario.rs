//! Station and request files: TOML in, validated stations and requests out.
//!
//! This module is behind the `scenario` feature, which is off by default, so that the library
//! carries no file format and no `serde` or `toml` dependency unless a front end asks for it.
//! The `maat` command and the browser demo both turn it on, and so read exactly the same files.
//!
//! A file holds any number of `[[station]]` and `[[request]]` tables:
//!
//! ```toml
//! [[station]]
//! id = 1                          # your own number for the station, 0 to 65535
//! name = "Seattle"
//! latitude_deg = 47.61            # north positive
//! longitude_deg = -122.33         # east positive, -180 to 180
//! altitude_m = 50.0               # optional, default 0
//! minimum_elevation_deg = 10.0    # the elevation mask
//! turnaround_s = 30               # optional: fixed time between passes
//! slew_azimuth_deg_per_s = 6.0    # optional, both or neither: how fast the antenna turns
//! slew_elevation_deg_per_s = 2.5
//! satnogs_id = 2433               # optional: the station's number on the SatNOGS Network
//!
//! [[request]]
//! satellite = 25544               # catalog number, as in the catalog file
//! priority = 5                    # 0 to 255, higher wins
//! min_contact_s = 180             # optional: ignore passes shorter than this
//! need_s = 1200                   # optional: stop scheduling once this much is planned
//! satnogs_transmitter = "AbCdEfGhJkLmNpQrStUvWx"   # optional: for the SatNOGS export
//! ```
//!
//! The site keys (`latitude_deg`, `longitude_deg`, `altitude_m`, `minimum_elevation_deg`) are
//! the ones Chronus Gateway uses in its own `[station]` section.
//!
//! A key that is not listed here is an error, so a misspelt option is reported and not ignored.

use maat_flight::{Priority, SatelliteId, SatelliteRequest, Seconds, StationId};
use serde::Deserialize;

use crate::export::SatnogsIds;
use crate::station::{GroundStation, SlewLimits, StationSite};

/// Everything read from the station and request files.
#[derive(Debug, Default)]
pub struct Scenario {
    /// The ground stations, in file order.
    pub stations: Vec<GroundStation>,
    /// The contact requests, in file order.
    pub requests: Vec<SatelliteRequest>,
    /// SatNOGS numbers for the stations and satellites that gave one.
    pub satnogs: SatnogsIds,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileToml {
    #[serde(default)]
    station: Vec<StationToml>,
    #[serde(default)]
    request: Vec<RequestToml>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StationToml {
    id: u16,
    name: String,
    latitude_deg: f64,
    longitude_deg: f64,
    #[serde(default)]
    altitude_m: f64,
    minimum_elevation_deg: f64,
    turnaround_s: Option<u32>,
    slew_azimuth_deg_per_s: Option<f64>,
    slew_elevation_deg_per_s: Option<f64>,
    satnogs_id: Option<u32>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RequestToml {
    satellite: u32,
    priority: u8,
    min_contact_s: Option<u32>,
    need_s: Option<u32>,
    satnogs_transmitter: Option<String>,
}

impl Scenario {
    /// Adds the stations and requests in one file's text.
    ///
    /// `file` is the name to use in error messages.
    ///
    /// # Errors
    ///
    /// A message naming the file and, where it can, the line, the station or the request.
    pub fn add_toml(&mut self, file: &str, text: &str) -> Result<(), String> {
        // The parser's message already carries the line, the column and the offending text.
        let parsed: FileToml = toml::from_str(text)
            .map_err(|error| format!("{file}: {}", tidy(&error.to_string())))?;

        for station in parsed.station {
            let id = StationId::new(station.id);
            let label = format!("{file}: station {} \"{}\"", station.id, station.name);
            let mut built = GroundStation::new(
                id,
                station.name,
                StationSite {
                    latitude_deg: station.latitude_deg,
                    longitude_deg: station.longitude_deg,
                    altitude_m: station.altitude_m,
                },
                station.minimum_elevation_deg,
            )
            .map_err(|error| format!("{file}: {error}"))?;

            match (
                station.slew_azimuth_deg_per_s,
                station.slew_elevation_deg_per_s,
            ) {
                (None, None) => {}
                (Some(azimuth_deg_per_s), Some(elevation_deg_per_s)) => {
                    built = built
                        .with_slew_limits(SlewLimits {
                            azimuth_deg_per_s,
                            elevation_deg_per_s,
                        })
                        .map_err(|error| format!("{file}: {error}"))?;
                }
                (Some(_), None) => {
                    return Err(format!(
                        "{label} has slew_azimuth_deg_per_s but not slew_elevation_deg_per_s; \
                         give both rates or neither"
                    ));
                }
                (None, Some(_)) => {
                    return Err(format!(
                        "{label} has slew_elevation_deg_per_s but not slew_azimuth_deg_per_s; \
                         give both rates or neither"
                    ));
                }
            }
            if let Some(seconds) = station.turnaround_s {
                built = built.with_turnaround(Seconds::new(seconds));
            }
            if let Some(number) = station.satnogs_id {
                self.satnogs = std::mem::take(&mut self.satnogs).with_station(id, number);
            }
            self.stations.push(built);
        }

        for request in parsed.request {
            let satellite = SatelliteId::new(request.satellite);
            let mut built = SatelliteRequest::new(satellite, Priority::new(request.priority));
            if let Some(seconds) = request.min_contact_s {
                built = built.with_min_contact(Seconds::new(seconds));
            }
            if let Some(seconds) = request.need_s {
                built = built.with_need(Seconds::new(seconds));
            }
            if let Some(transmitter) = request.satnogs_transmitter {
                self.satnogs = std::mem::take(&mut self.satnogs)
                    .with_transmitter(satellite, &transmitter)
                    .map_err(|error| format!("{file}: {error}"))?;
            }
            self.requests.push(built);
        }
        Ok(())
    }
}

/// The TOML parser ends its messages with a newline and leads with "TOML parse error at"; keep
/// the content, drop the trailing blank.
fn tidy(message: &str) -> String {
    message.trim_end().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    const UUID: &str = "AbCdEfGhJkLmNpQrStUvWx";

    fn load(text: &str) -> Result<Scenario, String> {
        let mut scenario = Scenario::default();
        scenario.add_toml("stations.toml", text)?;
        Ok(scenario)
    }

    const FULL: &str = r#"
        [[station]]
        id = 1
        name = "Seattle"
        latitude_deg = 47.61
        longitude_deg = -122.33
        altitude_m = 50.0
        minimum_elevation_deg = 10.0
        turnaround_s = 30
        slew_azimuth_deg_per_s = 6.0
        slew_elevation_deg_per_s = 2.5
        satnogs_id = 2433

        [[station]]
        id = 2
        name = "Quito"
        latitude_deg = -0.18
        longitude_deg = -78.47
        minimum_elevation_deg = 5

        [[request]]
        satellite = 25544
        priority = 5
        min_contact_s = 180
        need_s = 1200
        satnogs_transmitter = "AbCdEfGhJkLmNpQrStUvWx"

        [[request]]
        satellite = 100123
        priority = 2
    "#;

    #[test]
    fn a_full_file_gives_stations_requests_and_satnogs_numbers() {
        let scenario = load(FULL).unwrap();
        assert_eq!(scenario.stations.len(), 2);

        let seattle = &scenario.stations[0];
        assert_eq!((seattle.id().get(), seattle.name()), (1, "Seattle"));
        assert_eq!(seattle.site().latitude_deg, 47.61);
        assert_eq!(seattle.site().longitude_deg, -122.33);
        assert_eq!(seattle.site().altitude_m, 50.0);
        assert_eq!(seattle.min_elevation_deg(), 10.0);
        assert_eq!(seattle.turnaround(), Seconds::new(30));
        let rates = seattle
            .rule()
            .slew_rates()
            .expect("Seattle has slew limits");
        assert_eq!(rates.azimuth_centidegrees_per_second(), 600);
        assert_eq!(rates.elevation_centidegrees_per_second(), 250);

        // Optional keys left out take their defaults; an integer is accepted for a float.
        let quito = &scenario.stations[1];
        assert_eq!(quito.site().altitude_m, 0.0);
        assert_eq!(quito.min_elevation_deg(), 5.0);
        assert_eq!(quito.turnaround(), Seconds::ZERO);
        assert!(quito.rule().slew_rates().is_none());

        assert_eq!(scenario.requests.len(), 2);
        let first = &scenario.requests[0];
        assert_eq!(first.satellite().get(), 25_544);
        assert_eq!(first.priority().get(), 5);
        assert_eq!(first.min_contact(), Seconds::new(180));
        assert_eq!(first.need(), Seconds::new(1_200));
        let second = &scenario.requests[1];
        assert_eq!(
            second.satellite().get(),
            100_123,
            "a number too big for a TLE is fine"
        );
        assert!(second.is_unlimited());
        assert_eq!(second.min_contact(), Seconds::ZERO);

        assert_eq!(scenario.satnogs.station(StationId::new(1)), Some(2_433));
        assert_eq!(scenario.satnogs.station(StationId::new(2)), None);
        assert_eq!(
            scenario.satnogs.transmitter(SatelliteId::new(25_544)),
            Some(UUID)
        );
        assert_eq!(
            scenario.satnogs.transmitter(SatelliteId::new(100_123)),
            None
        );
    }

    #[test]
    fn two_files_add_up() {
        let mut scenario = Scenario::default();
        scenario
            .add_toml(
                "stations.toml",
                "[[station]]\nid = 1\nname = \"A\"\nlatitude_deg = 1.0\nlongitude_deg = 2.0\n\
                 minimum_elevation_deg = 10.0\n",
            )
            .unwrap();
        scenario
            .add_toml(
                "requests.toml",
                "[[request]]\nsatellite = 7\npriority = 1\n",
            )
            .unwrap();
        assert_eq!((scenario.stations.len(), scenario.requests.len()), (1, 1));
    }

    #[test]
    fn an_empty_file_is_valid_and_adds_nothing() {
        let scenario = load("# nothing yet\n").unwrap();
        assert!(scenario.stations.is_empty() && scenario.requests.is_empty());
    }

    #[test]
    fn a_misspelt_key_is_reported_with_its_line() {
        let error = load(
            "[[station]]\nid = 1\nname = \"A\"\nlatitude_deg = 1.0\nlongitude_deg = 2.0\n\
             minimum_elevation_deg = 10.0\nturnaround = 30\n",
        )
        .unwrap_err();
        assert!(error.starts_with("stations.toml: "), "{error}");
        assert!(error.contains("line 7"), "{error}");
        assert!(error.contains("unknown field `turnaround`"), "{error}");
        // The parser lists the keys that are accepted, which is the fix.
        assert!(error.contains("turnaround_s"), "{error}");
    }

    #[test]
    fn a_missing_required_key_is_named() {
        let error = load("[[station]]\nid = 1\nname = \"A\"\nlatitude_deg = 1.0\n").unwrap_err();
        assert!(error.contains("missing field `longitude_deg`"), "{error}");
        let error = load("[[request]]\nsatellite = 7\n").unwrap_err();
        assert!(error.contains("missing field `priority`"), "{error}");
    }

    #[test]
    fn a_value_of_the_wrong_kind_or_size_is_reported() {
        let error = load("[[request]]\nsatellite = 7\npriority = 300\n").unwrap_err();
        assert!(error.contains("line 3") && error.contains("300"), "{error}");
        let error = load("[[request]]\nsatellite = \"ISS\"\npriority = 1\n").unwrap_err();
        assert!(error.contains("line 2"), "{error}");
        let error = load("[[request]]\nsatellite = -5\npriority = 1\n").unwrap_err();
        assert!(error.contains("line 2"), "{error}");
    }

    #[test]
    fn text_that_is_not_toml_is_reported_with_its_line() {
        let error = load("[[station]\nid = 1\n").unwrap_err();
        assert!(
            error.starts_with("stations.toml: ") && error.contains("line 1"),
            "{error}"
        );
    }

    #[test]
    fn a_station_the_library_refuses_keeps_the_librarys_explanation() {
        let error = load(
            "[[station]]\nid = 4\nname = \"Nowhere\"\nlatitude_deg = 123.0\n\
             longitude_deg = 2.0\nminimum_elevation_deg = 10.0\n",
        )
        .unwrap_err();
        assert_eq!(
            error,
            "stations.toml: invalid ground station 4 \"Nowhere\": latitude_deg is 123, expected \
             -90 to 90 degrees (positive north)"
        );
    }

    #[test]
    fn one_slew_rate_without_the_other_is_refused() {
        let station = "[[station]]\nid = 1\nname = \"A\"\nlatitude_deg = 1.0\n\
                       longitude_deg = 2.0\nminimum_elevation_deg = 10.0\n";
        let error = load(&format!("{station}slew_azimuth_deg_per_s = 6.0\n")).unwrap_err();
        assert_eq!(
            error,
            "stations.toml: station 1 \"A\" has slew_azimuth_deg_per_s but not \
             slew_elevation_deg_per_s; give both rates or neither"
        );
        let error = load(&format!("{station}slew_elevation_deg_per_s = 6.0\n")).unwrap_err();
        assert!(error.contains("but not slew_azimuth_deg_per_s"), "{error}");
        // A rate the library refuses is explained by the library.
        let error = load(&format!(
            "{station}slew_azimuth_deg_per_s = 0.0\nslew_elevation_deg_per_s = 2.0\n"
        ))
        .unwrap_err();
        assert!(
            error.starts_with("stations.toml: ") && error.contains("slew"),
            "{error}"
        );
    }

    #[test]
    fn a_malformed_satnogs_transmitter_is_refused() {
        let error =
            load("[[request]]\nsatellite = 7\npriority = 1\nsatnogs_transmitter = \"short\"\n")
                .unwrap_err();
        assert!(
            error.starts_with("stations.toml: ") && error.contains("22 letters and digits"),
            "{error}"
        );
    }
}
