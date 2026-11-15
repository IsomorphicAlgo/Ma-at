# Ma'at

*Working name. Ma'at is the Egyptian principle of balance, order and fair judgment.*

A Rust workspace that turns satellites, ground stations and priorities into a conflict-free,
explainable contact plan. It is built on [Ephemerust](https://crates.io/crates/ephemerust), and
its scheduling core is designed to run on the ground **and** onboard a spacecraft.

> **Status: Stage 1 of 5 — contact windows.** Ma'at can load satellites from OMM or TLE, describe
> ground stations, and list every contact opportunity for N satellites over M stations, with a
> stated reason for each pass it turns down. There is no scheduler yet: that is Stage 2.

## Crates

| Crate | Layer | What it holds |
|---|---|---|
| [`maat-flight`](crates/maat-flight) | Vehicle | `#![no_std]`, no heap, no dependencies, integer-only. Contact windows, identifiers, time, and the `MAAT-xxx` reason codes. |
| [`maat-ground`](crates/maat-ground) | Human | Depends on `maat-flight`. OMM/TLE satellites, ground stations, the window survey, rich errors. Pass prediction through Ephemerust. |
| [`maat-cli`](crates/maat-cli) | Human | The `maat` command. A thin layer over `maat-ground`. |

The split is enforced by the compiler, not by convention. `maat-flight` is `#![no_std]` and has an
empty dependency list, so code from the human layer cannot leak into it. CI builds it for a
bare-metal Cortex-M target on every push to prove it.

## Try it

```text
cargo test --workspace
cargo run -p maat-ground --example window_survey
cargo run -p maat-cli -- codes
```

The example surveys one day for two synthetic satellites over three stations. One station has a
slow antenna mount, so its high passes are turned down, and the survey says why:

```text
2 satellites x 3 stations, 24 h from 2026-08-01 12:00:00 UTC

SATELLITE  STATION    AOS       LOS       LENGTH       PEAK
100123     Svalbard   12:19:34  12:22:14   160 s  11.56 deg
65001      Seattle    12:28:28  12:33:07   279 s   8.73 deg
100123     Quito      16:40:39  16:47:51   432 s  57.16 deg
100123     Seattle    18:27:42  18:36:36   534 s  46.20 deg
...
23 usable windows
3 passes rejected
  65001 over Seattle at 10:02:59, peak 73.9 deg
    MAAT-105: dropped: tracking the pass needs a faster antenna slew than the station allows
    tracking needs 3.46 deg/s in azimuth (the station allows 1.00)
  ...
0 satellite/station pairs failed
```

In code:

```rust
use chrono::Duration;
use maat_ground::flight::StationId;
use maat_ground::{GroundStation, StationSite, contact_windows, satellites_from_omm};

// OMM text in JSON, KVN, XML or CSV; the format is detected.
let satellites = satellites_from_omm(&std::fs::read_to_string("catalog.json")?)?;

let seattle = GroundStation::new(
    StationId::new(1),
    "Seattle",
    StationSite { latitude_deg: 47.61, longitude_deg: -122.33, altitude_m: 50.0 },
    10.0, // elevation mask, degrees
)?;

let start = satellites[0].epoch();
let survey = contact_windows(&satellites, &[seattle], start, start + Duration::days(1))?;

for window in &survey.windows {   // sorted by AOS, ready for a scheduler
    println!("{} {} {}", window.satellite(), window.aos(), window.duration());
}
for pass in &survey.rejected {    // found, but not usable, with the reason
    println!("{}: {}", pass.reason, pass.detail);
}
```

## Reason codes

Every decision carries a stable code. Once published, a code's number and meaning never change.

| Code | Class | Meaning |
|---|---|---|
| `MAAT-001` | accepted | scheduled: the pass is in the contact plan |
| `MAAT-101` | rejected | dropped: overlaps a higher-priority pass at the same station |
| `MAAT-102` | rejected | dropped: the satellite is already scheduled with another station at that time |
| `MAAT-103` | rejected | dropped: the pass is shorter than the minimum contact length |
| `MAAT-104` | rejected | dropped: the satellite's contact need is already met by other passes |
| `MAAT-105` | rejected | dropped: tracking the pass needs a faster antenna slew than the station allows |
| `MAAT-201` | invalid input | invalid window: loss of signal is not later than acquisition of signal |
| `MAAT-202` | invalid input | invalid window: the duration does not fit in 32-bit seconds |
| `MAAT-203` | invalid input | invalid window: the maximum elevation is outside 0 to 90 degrees |
| `MAAT-301` | resource limit | not evaluated: a fixed-size plan buffer is full |

`maat codes` prints this table from the flight crate itself. The window survey emits `MAAT-103`
and `MAAT-105` today. `MAAT-101`, `102`, `104` and `301` are reserved for the Stage 2 scheduler.

## Design in brief

- **Integer-only flight types.** Time is whole UTC seconds and elevation is hundredths of a
  degree. No floating point in the flight crate means bit-identical results on every processor.
- **Half-open windows.** A contact is `[AOS, LOS)`. A pass that ends the second another begins
  does not conflict with it.
- **Inward rounding at the boundary.** Converting a predicted pass to a flight window rounds AOS
  up, LOS down and peak elevation down. The window never promises more than the pass.
- **Nothing is dropped silently.** Every pass the survey finds becomes a usable window, a
  rejected pass with a reason code, or a reported failure.
- **One bad satellite does not sink the survey.** A satellite that cannot be propagated is
  reported, and the others are still searched.
- **No panic paths in flight code.** `unwrap`, `expect`, `panic!`, slice indexing and unchecked
  arithmetic are denied by lint in `maat-flight`.
- **Errors explain themselves.** An error says what was given, what was expected and why.

The reasoning behind each decision is recorded in [`Methodology.md`](Methodology.md).

## Accuracy and limitations

Stated openly, in the spirit of Ephemerust's precision assessment:

- **Designed to be flight-suitable. Not flight-qualified.** Nothing here has been through formal
  verification, certification or flight heritage.
- **The flight crate does not predict passes.** Windows are computed on the ground and handed to
  it as data. A heap-free onboard predictor, built directly on the `sgp4` crate and checked
  against Ephemerust, is planned for later.
- **One-second, 0.01-degree resolution.** Each window is up to two seconds shorter than the
  predicted pass, by design.
- **Pass accuracy is Ephemerust's.** Windows inherit the accuracy of SGP4 and of the element set's
  age. A search far from the element epoch returns confident-looking windows for an orbit that is
  no longer true, and Ma'at does not yet warn about it. See Ephemerust's
  `docs/accuracy-and-limits.md`.
- **Pass completeness is checked, for the cases tested.** Ma'at's tests compare the survey with a
  one-second brute-force elevation scan for two low orbits over two stations and require every
  pass to appear. Ephemerust 1.2.1 carries a far larger comparison of its own.
- **The elevation mask is one number per station.** A horizon that varies with azimuth is not
  modelled.
- **Slew limits reject a pass whole.** A pass that needs a faster turn than the mount allows is
  not split into the usable parts either side of the fast section. The rate tested is the
  largest average over any one second.
- **Unix time, no leap seconds.** Timestamps count seconds the way Unix time does.

## Roadmap

| Stage | Scope | State |
|---|---|---|
| 0 | Core types and the `no_std` boundary | done |
| 1 | Windows: N satellites × M stations, OMM input, elevation masks, slew limits | **built, awaiting sign-off** |
| 2 | Scheduler: greedy (flight-safe), then optimal ILP (ground-only) | next |
| 3 | Explainable output: every pass scheduled or rejected says why | |
| 4 | Plug-ins: Chronus export, SatNOGS-compatible format, full CLI | |
| 5 | WASM browser demo | later |
| — | Onboard pass prediction: heap-free, on the `sgp4` crate, validated against Ephemerust | later |

## Credits

- [Ephemerust](https://crates.io/crates/ephemerust) by the same author provides OMM and TLE
  parsing and pass prediction. It builds on the [`sgp4`](https://crates.io/crates/sgp4) crate.
- [`chrono`](https://crates.io/crates/chrono) for calendar time and
  [`thiserror`](https://crates.io/crates/thiserror) for error types, in the ground layer only.
- The reference ISS element set used in tests and examples is public data. The other satellites
  in tests and examples are synthetic.

Minimum supported Rust version: **1.89**. License: [MIT](LICENSE).
