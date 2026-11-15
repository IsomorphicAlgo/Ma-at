//! Shared helpers for this crate's unit tests: hand-built windows and random scheduling problems.

use maat_flight::{
    ContactWindow, Elevation, Priority, SatelliteId, SatelliteRequest, Seconds, StationId,
    StationRule, Timestamp,
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
/// conflicts, ties, unmet minimums and met needs all come up. One satellite has no request.
pub(crate) fn random_instance(rng: &mut Lcg, count: u64) -> Instance {
    let satellites = 2 + rng.below(5);
    let stations = 1 + rng.below(3);
    // Keep the crowding roughly constant as the problem grows.
    let horizon = 150 * count / stations.max(1) + 600;
    let windows = (0..count)
        .map(|_| {
            let aos = i64::try_from(rng.below(horizon)).unwrap();
            let length = i64::try_from(30 + rng.below(600)).unwrap();
            window(
                u32::try_from(1 + rng.below(satellites + 1)).unwrap(),
                u16::try_from(1 + rng.below(stations)).unwrap(),
                aos,
                aos + length,
            )
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
            rules.push(rule(
                u16::try_from(station).unwrap(),
                u32::try_from(rng.below(120)).unwrap(),
            ));
        }
    }
    Instance {
        windows,
        requests,
        rules,
    }
}
