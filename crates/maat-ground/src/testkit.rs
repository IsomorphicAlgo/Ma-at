//! Shared helpers for this crate's unit tests: hand-built windows and random scheduling problems.

use maat_flight::{
    Azimuth, ContactWindow, Elevation, Pointing, Priority, SatelliteId, SatelliteRequest, Seconds,
    SlewRates, StationId, StationRule, Timestamp,
};

/// A contact window from plain numbers. Times are seconds from an arbitrary zero.
pub(crate) fn window(satellite: u32, station: u16, aos: i64, los: i64) -> ContactWindow {
    ContactWindow::new(
        SatelliteId::new(satellite),
        StationId::new(station),
        Timestamp::from_unix_seconds(aos),
        Timestamp::from_unix_seconds(los),
        Elevation::ZENITH,
    )
    .expect("test windows are valid")
}

/// A pointing from hundredths of a degree.
pub(crate) fn at(azimuth: u16, elevation: u16) -> Pointing {
    Pointing {
        azimuth: Azimuth::from_centidegrees(azimuth).expect("test azimuths are valid"),
        elevation: Elevation::from_centidegrees(elevation).expect("test elevations are valid"),
    }
}

/// A window that rises at `rise_azimuth` and sets at `set_azimuth`, both at 10 degrees up.
pub(crate) fn pointed(
    satellite: u32,
    station: u16,
    aos: i64,
    los: i64,
    rise_azimuth: u16,
    set_azimuth: u16,
) -> ContactWindow {
    window(satellite, station, aos, los)
        .with_pointing(at(rise_azimuth, 1_000), at(set_azimuth, 1_000))
}

pub(crate) fn request(satellite: u32, priority: u8) -> SatelliteRequest {
    SatelliteRequest::new(SatelliteId::new(satellite), Priority::new(priority))
}

pub(crate) fn rule(station: u16, turnaround: u32) -> StationRule {
    StationRule::new(StationId::new(station), Seconds::new(turnaround))
}

/// A small deterministic pseudo-random generator: no dependency, and the same problems on every
/// run and every machine.
pub(crate) struct Lcg(pub(crate) u64);

impl Lcg {
    pub(crate) fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 33
    }

    pub(crate) fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

/// A scheduling problem.
pub(crate) struct Instance {
    pub(crate) windows: Vec<ContactWindow>,
    pub(crate) requests: Vec<SatelliteRequest>,
    pub(crate) rules: Vec<StationRule>,
}

/// A crowded random problem with `count` windows: few stations and a short horizon, so that
/// conflicts, ties, unmet minimums and met needs all come up. One satellite has no request. Most
/// windows record their pointing and some stations have slew rates, so the time an antenna
/// needs to turn between passes is exercised too.
pub(crate) fn random_instance(rng: &mut Lcg, count: u64) -> Instance {
    let satellites = 2 + rng.below(5);
    let stations = 1 + rng.below(3);
    // Keep the crowding roughly constant as the problem grows.
    let horizon = 150 * count / stations.max(1) + 600;
    let windows = (0..count)
        .map(|_| {
            let aos = i64::try_from(rng.below(horizon)).unwrap();
            let length = i64::try_from(30 + rng.below(600)).unwrap();
            let plain = window(
                u32::try_from(1 + rng.below(satellites + 1)).unwrap(),
                u16::try_from(1 + rng.below(stations)).unwrap(),
                aos,
                aos + length,
            );
            // Most windows record where the antenna points; a few do not.
            if rng.below(5) == 0 {
                plain
            } else {
                let mut direction = || u16::try_from(rng.below(36_000)).unwrap();
                let (rise, set) = (direction(), direction());
                plain.with_pointing(at(rise, 1_000), at(set, 1_000))
            }
        })
        .collect();
    let mut requests = Vec::new();
    for satellite in 1..=satellites {
        let mut r = request(
            u32::try_from(satellite).unwrap(),
            u8::try_from(rng.below(4)).unwrap(),
        );
        if rng.below(2) == 0 {
            r = r.with_min_contact(Seconds::new(u32::try_from(rng.below(200)).unwrap()));
        }
        if rng.below(2) == 0 {
            r = r.with_need(Seconds::new(u32::try_from(rng.below(2_000)).unwrap()));
        }
        requests.push(r);
    }
    let mut rules = Vec::new();
    for station in 1..=stations {
        if rng.below(2) == 0 {
            let mut station_rule = rule(
                u16::try_from(station).unwrap(),
                u32::try_from(rng.below(120)).unwrap(),
            );
            // Half the rules also have a slow mount: 1 to 4 degrees per second.
            if rng.below(2) == 0 {
                let rate = u16::try_from(100 + rng.below(300)).unwrap();
                station_rule = station_rule.with_slew_rates(SlewRates::new(rate, rate).unwrap());
            }
            rules.push(station_rule);
        }
    }
    Instance {
        windows,
        requests,
        rules,
    }
}
