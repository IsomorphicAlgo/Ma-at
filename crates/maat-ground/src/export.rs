//! A contact plan in the forms other systems take: a per-station schedule for a
//! [Chronus Gateway](https://github.com/IsomorphicAlgo/chronus-gateway) station, and a list of
//! observations in the shape the [SatNOGS Network](https://network.satnogs.org) accepts.
//!
//! Both exports contain **scheduled passes only**. The full plan, with every pass that was not
//! scheduled and why, is [`ContactPlan::to_json`].
//!
//! ## What "compatible" means here, stated plainly
//!
//! - **Chronus Gateway** (checked against 1.0.0-rc.1) tracks one satellite per configuration
//!   and has no schedule input. [`station_schedule`] is therefore Ma'at's own document,
//!   `maat.station-schedule/1`: what one station is to track and when. The site uses the same
//!   key names as the gateway's `[station]` configuration (`latitude_deg`, `longitude_deg`,
//!   `altitude_m`, `minimum_elevation_deg`), so the two can be compared directly. Nothing in the
//!   gateway reads this document yet.
//! - **SatNOGS Network** creates observations from a list of objects with four fields:
//!   `ground_station`, `transmitter_uuid`, `start` and `end`. [`satnogs_schedule`] produces
//!   exactly that list. It has **not** been sent to the network: doing so needs an account, a
//!   station you may schedule on, and a transmitter the station supports, and the network
//!   applies checks of its own. SatNOGS identifies stations and transmitters by its own
//!   numbers, which Ma'at does not know; you supply them in a [`SatnogsIds`].

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use maat_flight::{SatelliteId, StationId};
use serde_json::{Value, json};

use crate::error::GroundError;
use crate::plan::{ContactPlan, PlanEntry};
use crate::station::GroundStation;

/// The identifier of the station schedule's layout. The number changes only if a field is
/// removed or changes meaning; adding a field does not change it.
pub const STATION_SCHEDULE_SCHEMA: &str = "maat.station-schedule/1";

/// Times in both exports: ISO 8601 in UTC, to the second. Scheduled windows are whole seconds.
const EXPORT_TIME: &str = "%Y-%m-%dT%H:%M:%SZ";

/// The passes one station is scheduled to track, as a JSON value. See the [module docs](self).
///
/// Passes are in order of AOS. A station with nothing scheduled gets an empty `passes` list.
///
/// The top level has `schema` ([`STATION_SCHEDULE_SCHEMA`]), `generated_by`, `scheduler`,
/// `complete` (false if the survey behind the plan could not search every satellite), `station`
/// and `passes`. Each pass has `satellite` (`norad_cat_id`, `name`), `aos`, `los`, `duration_s`,
/// `max_elevation_deg`, `aos_azimuth_deg`, `los_azimuth_deg` (`null` if the window records no
/// pointing) and `priority`.
#[must_use]
pub fn station_schedule(plan: &ContactPlan, station: &GroundStation) -> Value {
    let site = station.site();
    let passes: Vec<Value> = plan
        .scheduled()
        .filter(|entry| entry.station == station.id())
        .map(|entry| {
            let azimuth = |end: Option<maat_flight::Pointing>| {
                end.map(|pointing| f64::from(pointing.azimuth.centidegrees()) / 100.0)
            };
            let window = entry.window;
            json!({
                "satellite": {
                    "norad_cat_id": entry.satellite.get(),
                    "name": entry.satellite_name,
                },
                "aos": entry.aos.format(EXPORT_TIME).to_string(),
                "los": entry.los.format(EXPORT_TIME).to_string(),
                "duration_s": (entry.los - entry.aos).num_seconds(),
                "max_elevation_deg": entry.max_elevation_deg,
                "aos_azimuth_deg": azimuth(window.and_then(|w| w.aos_pointing())),
                "los_azimuth_deg": azimuth(window.and_then(|w| w.los_pointing())),
                "priority": entry.priority.map(maat_flight::Priority::get),
            })
        })
        .collect();

    json!({
        "schema": STATION_SCHEDULE_SCHEMA,
        "generated_by": concat!("maat-ground ", env!("CARGO_PKG_VERSION")),
        "scheduler": plan.scheduler.name(),
        "complete": plan.is_complete(),
        "station": {
            "id": station.id().get(),
            "name": station.name(),
            // The same key names as Chronus Gateway's `[station]` configuration.
            "latitude_deg": site.latitude_deg,
            "longitude_deg": site.longitude_deg,
            "altitude_m": site.altitude_m,
            "minimum_elevation_deg": station.min_elevation_deg(),
        },
        "passes": passes,
    })
}

/// [`station_schedule`] as indented JSON text ending in a newline.
#[must_use]
pub fn station_schedule_json(plan: &ContactPlan, station: &GroundStation) -> String {
    pretty(&station_schedule(plan, station))
}

/// The length of a SatNOGS transmitter identifier.
const TRANSMITTER_UUID_LENGTH: usize = 22;

/// The numbers SatNOGS uses for the stations and transmitters in a plan.
///
/// SatNOGS Network gives each ground station a number of its own, and SatNOGS DB gives each
/// satellite transmitter a 22-character identifier. Neither can be worked out from an orbit or a
/// site, so they are supplied here.
///
/// ```
/// use maat_ground::SatnogsIds;
/// use maat_ground::flight::{SatelliteId, StationId};
///
/// let ids = SatnogsIds::new()
///     .with_station(StationId::new(1), 2433)
///     .with_transmitter(SatelliteId::new(25_544), "AbCdEfGhJkLmNpQrStUvWx")?;
/// assert_eq!(ids.station(StationId::new(1)), Some(2433));
/// # Ok::<(), maat_ground::GroundError>(())
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SatnogsIds {
    stations: BTreeMap<StationId, u32>,
    transmitters: BTreeMap<SatelliteId, String>,
}

impl SatnogsIds {
    /// No identifiers yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Records the SatNOGS Network number of one of the plan's stations.
    #[must_use]
    pub fn with_station(mut self, station: StationId, satnogs_station: u32) -> Self {
        self.stations.insert(station, satnogs_station);
        self
    }

    /// Records the transmitter to observe for one of the plan's satellites.
    ///
    /// # Errors
    ///
    /// [`GroundError::InvalidTransmitter`] unless the identifier is 22 letters and digits,
    /// which is the form SatNOGS DB gives them.
    pub fn with_transmitter(
        mut self,
        satellite: SatelliteId,
        transmitter_uuid: &str,
    ) -> Result<Self, GroundError> {
        let well_formed = transmitter_uuid.len() == TRANSMITTER_UUID_LENGTH
            && transmitter_uuid.bytes().all(|b| b.is_ascii_alphanumeric());
        if !well_formed {
            return Err(GroundError::InvalidTransmitter {
                satellite,
                given: transmitter_uuid.to_owned(),
            });
        }
        self.transmitters
            .insert(satellite, transmitter_uuid.to_owned());
        Ok(self)
    }

    /// The SatNOGS Network number recorded for a station.
    #[must_use]
    pub fn station(&self, station: StationId) -> Option<u32> {
        self.stations.get(&station).copied()
    }

    /// The transmitter recorded for a satellite.
    #[must_use]
    pub fn transmitter(&self, satellite: SatelliteId) -> Option<&str> {
        self.transmitters.get(&satellite).map(String::as_str)
    }
}

/// One observation to create on the SatNOGS Network.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct SatnogsObservation {
    /// The SatNOGS Network number of the ground station.
    pub ground_station: u32,
    /// The SatNOGS DB identifier of the transmitter to observe.
    pub transmitter_uuid: String,
    /// When the observation begins: the pass's AOS.
    pub start: DateTime<Utc>,
    /// When it ends: the pass's LOS.
    pub end: DateTime<Utc>,
}

/// A scheduled pass that could not be turned into a SatNOGS observation.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct SkippedPass {
    /// The position of the pass in [`ContactPlan::entries`].
    pub entry: usize,
    /// What is missing, naming the satellite and the station.
    pub reason: String,
}

/// A contact plan's scheduled passes as SatNOGS observations.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct SatnogsSchedule {
    /// One observation per scheduled pass that has both identifiers, in order of AOS.
    pub observations: Vec<SatnogsObservation>,
    /// The scheduled passes left out, each with the reason. Never dropped silently.
    pub skipped: Vec<SkippedPass>,
}

impl SatnogsSchedule {
    /// The observations as a JSON array, each object with exactly the four fields the SatNOGS
    /// Network's new-observation request takes: `ground_station`, `transmitter_uuid`, `start`
    /// and `end`. Indented, ending in a newline.
    #[must_use]
    pub fn to_json(&self) -> String {
        let observations: Vec<Value> = self
            .observations
            .iter()
            .map(|observation| {
                json!({
                    "ground_station": observation.ground_station,
                    "transmitter_uuid": observation.transmitter_uuid,
                    "start": observation.start.format(EXPORT_TIME).to_string(),
                    "end": observation.end.format(EXPORT_TIME).to_string(),
                })
            })
            .collect();
        pretty(&Value::Array(observations))
    }
}

/// Turns a plan's scheduled passes into SatNOGS observations. See the [module docs](self).
///
/// A scheduled pass whose station has no SatNOGS number, or whose satellite has no transmitter,
/// in `ids` is listed in [`SatnogsSchedule::skipped`] with the reason.
#[must_use]
pub fn satnogs_schedule(plan: &ContactPlan, ids: &SatnogsIds) -> SatnogsSchedule {
    let mut schedule = SatnogsSchedule {
        observations: Vec::new(),
        skipped: Vec::new(),
    };
    for (index, entry) in plan.entries.iter().enumerate() {
        if !entry.is_scheduled() {
            continue;
        }
        match (ids.station(entry.station), ids.transmitter(entry.satellite)) {
            (Some(ground_station), Some(transmitter)) => {
                schedule.observations.push(SatnogsObservation {
                    ground_station,
                    transmitter_uuid: transmitter.to_owned(),
                    start: entry.aos,
                    end: entry.los,
                });
            }
            (station, transmitter) => schedule.skipped.push(SkippedPass {
                entry: index,
                reason: missing(entry, station.is_none(), transmitter.is_none()),
            }),
        }
    }
    schedule
}

/// Says which identifier a skipped pass lacks.
fn missing(entry: &PlanEntry, no_station: bool, no_transmitter: bool) -> String {
    let pass = format!(
        "the pass of {} over {} at {}",
        entry.satellite_name,
        entry.station_name,
        entry.aos.format("%Y-%m-%d %H:%M:%S")
    );
    let station = format!(
        "station {} ({}) has no SatNOGS station number",
        entry.station, entry.station_name
    );
    let transmitter = format!(
        "satellite {} ({}) has no SatNOGS transmitter",
        entry.satellite, entry.satellite_name
    );
    match (no_station, no_transmitter) {
        (true, true) => format!("{pass}: {station} and {transmitter}"),
        (true, false) => format!("{pass}: {station}"),
        _ => format!("{pass}: {transmitter}"),
    }
}

/// Indented JSON ending in a newline.
fn pretty(value: &Value) -> String {
    // A `Value` always serializes: its keys are strings and it holds no non-finite numbers.
    let mut text = serde_json::to_string_pretty(value).unwrap_or_else(|_| "null".to_owned());
    text.push('\n');
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::{Scheduler, plan_contacts};
    use crate::station::StationSite;
    use crate::survey::WindowSurvey;
    use crate::testkit::{pointed, request, window};
    use maat_flight::Seconds;

    /// Times count from 2026-08-01 12:00:00 UTC.
    const T0: i64 = 1_785_585_600;
    const UUID_1: &str = "AbCdEfGhJkLmNpQrStUvWx";
    const UUID_2: &str = "22CharactersLong000002";

    fn station(id: u16, name: &str) -> GroundStation {
        GroundStation::new(
            StationId::new(id),
            name,
            StationSite {
                latitude_deg: 47.61,
                longitude_deg: -122.33,
                altitude_m: 50.0,
            },
            10.0,
        )
        .unwrap()
        .with_turnaround(Seconds::new(30))
    }

    /// Satellite 1 over Seattle twice (one with pointing), satellite 2 over Quito once, and a
    /// pass of satellite 2 over Seattle that loses to satellite 1.
    fn plan() -> (ContactPlan, Vec<GroundStation>) {
        let stations = vec![station(1, "Seattle"), station(2, "Quito")];
        let survey = WindowSurvey {
            windows: vec![
                window(1, 1, T0 + 2_000, T0 + 2_450),
                pointed(1, 1, T0, T0 + 600, 4_550, 31_025),
                window(2, 2, T0 + 100, T0 + 400),
                window(2, 1, T0 + 300, T0 + 500),
            ],
            rejected: Vec::new(),
            failures: Vec::new(),
        };
        let requests = [request(1, 9), request(2, 1)];
        let plan = plan_contacts(&survey, &[], &stations, &requests, Scheduler::Greedy).unwrap();
        assert_eq!(plan.scheduled().count(), 3);
        (plan, stations)
    }

    // --- Station schedule ----------------------------------------------------------------------

    #[test]
    fn a_station_schedule_lists_that_stations_scheduled_passes_in_order() {
        let (plan, stations) = plan();
        let schedule = station_schedule(&plan, &stations[0]);
        assert_eq!(schedule["schema"], "maat.station-schedule/1");
        assert_eq!(schedule["scheduler"], "greedy");
        assert_eq!(schedule["complete"], true);
        assert_eq!(
            schedule["generated_by"],
            format!("maat-ground {}", env!("CARGO_PKG_VERSION"))
        );
        assert_eq!(
            schedule["station"],
            json!({
                "id": 1, "name": "Seattle",
                "latitude_deg": 47.61, "longitude_deg": -122.33, "altitude_m": 50.0,
                "minimum_elevation_deg": 10.0,
            })
        );

        let passes = schedule["passes"].as_array().unwrap();
        assert_eq!(
            passes.len(),
            2,
            "the losing pass and Quito's pass are not here"
        );
        assert_eq!(
            passes[0],
            json!({
                "satellite": { "norad_cat_id": 1, "name": "satellite 1" },
                "aos": "2026-08-01T12:00:00Z",
                "los": "2026-08-01T12:10:00Z",
                "duration_s": 600,
                "max_elevation_deg": 90.0,
                "aos_azimuth_deg": 45.5,
                "los_azimuth_deg": 310.25,
                "priority": 9,
            })
        );
        // A window without pointing has no azimuths to give.
        assert_eq!(passes[1]["aos"], "2026-08-01T12:33:20Z");
        assert_eq!(passes[1]["aos_azimuth_deg"], Value::Null);
        assert_eq!(passes[1]["los_azimuth_deg"], Value::Null);
    }

    #[test]
    fn the_station_block_uses_the_keys_of_a_chronus_gateway_station_section() {
        // Chronus Gateway 1.0.0-rc.1 reads these four keys from `[station]`, and refuses keys
        // it does not know. If this list changes, the gateway's configuration has to be checked.
        let (plan, stations) = plan();
        let schedule = station_schedule(&plan, &stations[0]);
        let station = schedule["station"].as_object().unwrap();
        for key in [
            "latitude_deg",
            "longitude_deg",
            "altitude_m",
            "minimum_elevation_deg",
        ] {
            assert!(station[key].is_f64(), "{key}");
        }
    }

    #[test]
    fn a_station_with_nothing_scheduled_gets_an_empty_list() {
        let (plan, _) = plan();
        let idle = station(7, "Idle");
        let schedule = station_schedule(&plan, &idle);
        assert_eq!(schedule["passes"], json!([]));
        assert_eq!(schedule["station"]["id"], 7);
    }

    #[test]
    fn station_schedule_text_is_indented_json_that_parses_back() {
        let (plan, stations) = plan();
        let text = station_schedule_json(&plan, &stations[1]);
        assert!(text.ends_with("}\n") && text.contains("\n  \"passes\": ["));
        let parsed: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(parsed, station_schedule(&plan, &stations[1]));
        assert_eq!(
            text,
            station_schedule_json(&plan, &stations[1]),
            "deterministic"
        );
    }

    // --- SatNOGS -------------------------------------------------------------------------------

    fn all_ids() -> SatnogsIds {
        SatnogsIds::new()
            .with_station(StationId::new(1), 2_433)
            .with_station(StationId::new(2), 17)
            .with_transmitter(SatelliteId::new(1), UUID_1)
            .unwrap()
            .with_transmitter(SatelliteId::new(2), UUID_2)
            .unwrap()
    }

    #[test]
    fn every_scheduled_pass_becomes_one_observation_in_order_of_aos() {
        let (plan, _) = plan();
        let schedule = satnogs_schedule(&plan, &all_ids());
        assert!(schedule.skipped.is_empty());
        let summary: Vec<(u32, &str, i64, i64)> = schedule
            .observations
            .iter()
            .map(|o| {
                (
                    o.ground_station,
                    o.transmitter_uuid.as_str(),
                    o.start.timestamp() - T0,
                    o.end.timestamp() - T0,
                )
            })
            .collect();
        assert_eq!(
            summary,
            [
                (2_433, UUID_1, 0, 600),
                (17, UUID_2, 100, 400),
                (2_433, UUID_1, 2_000, 2_450),
            ]
        );
    }

    #[test]
    fn satnogs_json_has_exactly_the_four_fields_the_network_takes() {
        let (plan, _) = plan();
        let text = satnogs_schedule(&plan, &all_ids()).to_json();
        let parsed: Value = serde_json::from_str(&text).unwrap();
        let observations = parsed.as_array().unwrap();
        assert_eq!(observations.len(), 3);
        assert_eq!(
            observations[0],
            json!({
                "ground_station": 2433,
                "transmitter_uuid": UUID_1,
                "start": "2026-08-01T12:00:00Z",
                "end": "2026-08-01T12:10:00Z",
            })
        );
        for observation in observations {
            let mut keys: Vec<&str> = observation
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect();
            keys.sort_unstable();
            assert_eq!(keys, ["end", "ground_station", "start", "transmitter_uuid"]);
            assert_eq!(observation["transmitter_uuid"].as_str().unwrap().len(), 22);
        }
        assert!(text.ends_with("]\n"));
    }

    #[test]
    fn a_pass_without_its_identifiers_is_skipped_and_says_what_is_missing() {
        let (plan, _) = plan();
        // Station 2 has no number; satellite 1 has no transmitter.
        let ids = SatnogsIds::new()
            .with_station(StationId::new(1), 2_433)
            .with_transmitter(SatelliteId::new(2), UUID_2)
            .unwrap();
        let schedule = satnogs_schedule(&plan, &ids);
        assert!(schedule.observations.is_empty());
        assert_eq!(schedule.skipped.len(), 3);
        // Every skipped pass points at a scheduled entry.
        for skipped in &schedule.skipped {
            assert!(plan.entries[skipped.entry].is_scheduled());
        }
        assert_eq!(
            schedule.skipped[0].reason,
            "the pass of satellite 1 over Seattle at 2026-08-01 12:00:00: satellite 1 \
             (satellite 1) has no SatNOGS transmitter"
        );
        assert_eq!(
            schedule.skipped[1].reason,
            "the pass of satellite 2 over Quito at 2026-08-01 12:01:40: station 2 (Quito) has \
             no SatNOGS station number"
        );

        let none = satnogs_schedule(&plan, &SatnogsIds::new());
        assert!(none.skipped[0].reason.ends_with(
            "station 1 (Seattle) has no SatNOGS station number and satellite 1 (satellite 1) \
             has no SatNOGS transmitter"
        ));
        assert_eq!(none.to_json(), "[]\n");
    }

    #[test]
    fn observations_and_skipped_together_account_for_every_scheduled_pass() {
        let (plan, _) = plan();
        for ids in [
            all_ids(),
            SatnogsIds::new(),
            SatnogsIds::new().with_station(StationId::new(2), 17),
        ] {
            let schedule = satnogs_schedule(&plan, &ids);
            assert_eq!(
                schedule.observations.len() + schedule.skipped.len(),
                plan.scheduled().count()
            );
        }
    }

    #[test]
    fn a_transmitter_must_be_22_letters_and_digits() {
        let satellite = SatelliteId::new(25_544);
        assert!(
            SatnogsIds::new()
                .with_transmitter(satellite, UUID_1)
                .is_ok()
        );
        for bad in [
            "",
            "tooShort",
            "AbCdEfGhJkLmNpQrStUvW",       // 21
            "AbCdEfGhJkLmNpQrStUvWxY",     // 23
            "AbCdEfGhJkLmNpQrStUv-x",      // punctuation
            "AbCdEfGhJkLmNpQrStUvW\u{e9}", // 22 characters but not ASCII
        ] {
            let error = SatnogsIds::new()
                .with_transmitter(satellite, bad)
                .unwrap_err();
            let message = error.to_string();
            assert!(
                message.contains("25544") && message.contains("22"),
                "{message}"
            );
        }
    }

    #[test]
    fn ids_are_looked_up_by_station_and_satellite() {
        let ids = all_ids();
        assert_eq!(ids.station(StationId::new(2)), Some(17));
        assert_eq!(ids.station(StationId::new(3)), None);
        assert_eq!(ids.transmitter(SatelliteId::new(2)), Some(UUID_2));
        assert_eq!(ids.transmitter(SatelliteId::new(9)), None);
        // A later entry for the same station replaces the earlier one.
        let replaced = ids.with_station(StationId::new(2), 99);
        assert_eq!(replaced.station(StationId::new(2)), Some(99));
    }
}
