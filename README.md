# Ma'at

*Working name. Ma'at is the Egyptian principle of balance, order and fair judgment.*

A Rust workspace that turns satellites, ground stations and priorities into a conflict-free,
explainable contact plan. It is built on [Ephemerust](https://crates.io/crates/ephemerust), and
its scheduling core is designed to run on the ground **and** onboard a spacecraft.

> **Status: Stage 3 of 5 — explained contact plans.** Ma'at loads satellites from OMM or TLE,
> finds every contact opportunity over a set of ground stations, schedules them with a greedy
> scheduler (flight-safe) or an optimal one (ground only), and gives a stated reason for every
> pass, scheduled or not. File-driven CLI commands and export formats are Stage 4.

## Crates

| Crate | Layer | What it holds |
|---|---|---|
| [`maat-flight`](crates/maat-flight) | Vehicle | `#![no_std]`, no heap, no dependencies, integer-only. Contact windows, the greedy scheduler, and the `MAAT-xxx` reason codes. |
| [`maat-ground`](crates/maat-ground) | Human | Depends on `maat-flight`. OMM/TLE satellites, ground stations, the window survey, the optimal scheduler, and the explained contact plan as text or JSON. |
| [`maat-cli`](crates/maat-cli) | Human | The `maat` command. A thin layer over `maat-ground`. |

The split is enforced by the compiler, not by convention. `maat-flight` is `#![no_std]` and has an
empty dependency list, so code from the human layer cannot leak into it. CI builds it for a
bare-metal Cortex-M target on every push to prove it.

## Try it

```text
cargo test --workspace
cargo run -p maat-ground --example contact_plan
cargo run -p maat-cli -- explain MAAT-107
```

The example plans six hours for five synthetic satellites over four stations and prints the
plan. Every pass that is not scheduled says why, and names what beat it:

```text
Contact plan (greedy scheduler)
  14 passes found: 4 scheduled, 10 not scheduled
  1218 s of scheduled contact, plan value 9096

SCHEDULED
  AOS (UTC)              LENGTH  SATELLITE  STATION         PEAK  PRI
  2026-08-01 12:26:26     132 s  EXAMPLE-D  Svalbard   11.05 deg    2
  2026-08-01 13:13:10     250 s  EXAMPLE-A  Cape Town  15.07 deg    9
  2026-08-01 14:48:58     404 s  EXAMPLE-A  Cape Town  62.77 deg    9
  2026-08-01 16:40:39     432 s  EXAMPLE-B  Quito      57.16 deg    4

NOT SCHEDULED
  ...
  2026-08-01 12:19:34     160 s  EXAMPLE-B  Svalbard   11.56 deg    4
      MAAT-103: dropped: the pass is shorter than the minimum contact length
      the pass lasts 160 s and EXAMPLE-B asks for passes of at least 180 s
  2026-08-01 16:41:27     430 s  EXAMPLE-C  Quito      55.25 deg    4
      MAAT-107: dropped: lost a tie with an equal-priority pass at the same station
      EXAMPLE-B has the same priority (4) and has Quito from 2026-08-01 16:40:39 to 2026-08-01 16:47:51; it wins the tie because its pass is longer (432 s against 430 s)
  2026-08-01 16:47:15     419 s  EXAMPLE-D  Quito      43.31 deg    2
      MAAT-101: dropped: overlaps a higher-priority pass at the same station
      EXAMPLE-B (priority 4) has Quito from 2026-08-01 16:40:39 to 2026-08-01 16:47:51; this pass has priority 2
  2026-08-01 17:05:24     529 s  EXAMPLE-E  Seattle    79.76 deg    -
      MAAT-105: dropped: tracking the pass needs a faster antenna slew than the station allows
      tracking needs 3.84 deg/s in azimuth (the station allows 2.00)
  ...

GREEDY AGAINST OPTIMAL, ONE WEEK
  greedy   166 of 615 passes scheduled,  58881 s of contact, value 285948
  optimal  173 of 615 passes scheduled,  61263 s of contact, value 293042
  The optimal plan is worth 2.5 % more.
```

Add `-- --json` to the example for the same plan as JSON.

In code:

```rust
use chrono::Duration;
use maat_ground::flight::{Priority, SatelliteRequest, Seconds, StationId};
use maat_ground::{
    GroundStation, Scheduler, StationSite, contact_windows, plan_contacts, satellites_from_omm,
};

// OMM text in JSON, KVN, XML or CSV; the format is detected.
let satellites = satellites_from_omm(&std::fs::read_to_string("catalog.json")?)?;

let seattle = GroundStation::new(
    StationId::new(1),
    "Seattle",
    StationSite { latitude_deg: 47.61, longitude_deg: -122.33, altitude_m: 50.0 },
    10.0, // elevation mask, degrees
)?
.with_turnaround(Seconds::new(120)); // time to re-point between passes
let stations = [seattle];

// What each satellite is asking for.
let requests: Vec<SatelliteRequest> = satellites
    .iter()
    .map(|s| SatelliteRequest::new(s.id(), Priority::new(5)).with_min_contact(Seconds::new(180)))
    .collect();

let start = satellites[0].epoch();
let survey = contact_windows(&satellites, &stations, start, start + Duration::days(1))?;
let plan = plan_contacts(&survey, &satellites, &stations, &requests, Scheduler::Greedy)?;

print!("{}", plan.to_text());                    // for a person
std::fs::write("plan.json", plan.to_json())?;    // for a program
```

Onboard, the same greedy scheduler runs without the ground crate, on fixed-size arrays:

```rust
use maat_flight::{Decision, schedule_greedy};

let mut decisions = [Decision::PENDING; 256];
let summary = schedule_greedy(&windows, &requests, &station_rules, &mut decisions)?;
```

## Two schedulers

| | Greedy | Optimal |
|---|---|---|
| Where | `maat-flight`: ground and onboard | `maat-ground`: ground only |
| Method | Best window first, never reconsidered | Integer linear program, solved exactly |
| Priority means | A strict rank: higher always wins | A weight: `priority + 1` per second |
| Memory | None of its own; the caller's slice | Allocates |
| Run time | Bounded: proportional to n² | Not bounded; fast in practice |
| Measured, 3000 windows | about 25 ms | 0.1 to 0.3 s |

Both obey the same rules, through the same function: a station serves one pass at a time and
needs its turnaround time between passes; a satellite uses one station at a time; a pass shorter
than a satellite's minimum is not used; contact beyond a satellite's need is not scheduled.

The optimal scheduler exists to measure what greedy gives up. On crowded random problems it was
worth 2 to 15 % more; on a week of realistic orbits over four stations, about 2 %.

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
| `MAAT-106` | rejected | dropped: no contact request covers this satellite |
| `MAAT-107` | rejected | dropped: lost a tie with an equal-priority pass at the same station |
| `MAAT-108` | rejected | dropped: too close to another pass for the station to re-point in time |
| `MAAT-109` | rejected | dropped: conflicts with a pass the optimizer kept for a better overall plan |
| `MAAT-201` | invalid input | invalid window: loss of signal is not later than acquisition of signal |
| `MAAT-202` | invalid input | invalid window: the duration does not fit in 32-bit seconds |
| `MAAT-203` | invalid input | invalid window: the maximum elevation is outside 0 to 90 degrees |
| `MAAT-204` | invalid input | invalid request: a satellite has more than one contact request |
| `MAAT-205` | invalid input | invalid request: a station has more than one rule |
| `MAAT-301` | resource limit | not evaluated: a fixed-size plan buffer is full |

An explanation has three levels:

1. the **code and summary** above: stable, one line, held in the flight crate;
2. the **detail** on each plan entry: the satellites, stations, times and numbers involved;
3. the **guidance** from `maat explain MAAT-107`: what the code means and what would change the
   outcome.

## Design in brief

- **Integer-only flight types.** Time is whole UTC seconds and elevation is hundredths of a
  degree. No floating point in the flight crate means bit-identical results on every processor.
- **Half-open windows.** A contact is `[AOS, LOS)`. A pass that ends the second another begins
  does not conflict with it.
- **Inward rounding at the boundary.** Converting a predicted pass to a flight window rounds AOS
  up, LOS down and peak elevation down. The window never promises more than the pass.
- **Nothing is dropped silently.** Every pass that is found appears once in the plan, with a
  reason.
- **One definition of conflict.** Both schedulers and the tests call the same function.
- **A fixed ranking.** Priority, then length, then AOS, then station and satellite number. The
  same input gives the same plan on any processor, in any input order.
- **One bad satellite does not sink the plan.** A satellite that cannot be propagated is
  reported, the others are still planned, and the plan is marked incomplete.
- **No panic paths in flight code.** `unwrap`, `expect`, `panic!`, slice indexing and unchecked
  arithmetic are denied by lint in `maat-flight`.
- **Errors explain themselves.** An error says what was given, what was expected and why.

The reasoning behind each decision is recorded in [`Methodology.md`](Methodology.md).

## Accuracy and limitations

Stated openly, in the spirit of Ephemerust's precision assessment:

- **Designed to be flight-suitable. Not flight-qualified.** Nothing here has been through formal
  verification, certification or flight heritage.
- **The flight crate schedules; it does not predict passes.** Windows are computed on the ground
  and handed to it as data. A heap-free onboard predictor, built directly on the `sgp4` crate
  and checked against Ephemerust, is planned for later.
- **Greedy is not optimal.** It can take one long pass that blocks two shorter ones worth more.
  That is the price of a bounded, memory-free scheduler, and the optimal scheduler measures it.
- **The two schedulers mean different things by priority.** Greedy ranks; optimal weighs. With
  mixed priorities their plans can differ in kind, not only in quality.
- **The optimal scheduler has no time limit.** The problem is NP-hard in general. Problems of up
  to 3000 windows solved in under a third of a second in testing, but nothing bounds the worst
  case. It is proven optimal by exhaustive search only on small problems (up to 16 windows).
- **A pass is used whole.** Neither scheduler trims a pass to fit, so a satellite can receive
  more contact than it asked for, by less than one pass.
- **Turnaround is one fixed time per station.** It does not depend on how far the antenna has
  to turn between the two passes.
- **One-second, 0.01-degree resolution.** Each window is up to two seconds shorter than the
  predicted pass, by design.
- **Pass accuracy is Ephemerust's.** Windows inherit the accuracy of SGP4 and of the element set's
  age. A search far from the element epoch returns confident-looking windows for an orbit that is
  no longer true, and Ma'at does not yet warn about it.
- **The elevation mask is one number per station.** A horizon that varies with azimuth is not
  modelled.
- **Slew limits reject a pass whole.** A pass that needs a faster turn than the mount allows is
  not split into the usable parts either side of the fast section.
- **Unix time, no leap seconds.** Timestamps count seconds the way Unix time does.

## Roadmap

| Stage | Scope | State |
|---|---|---|
| 0 | Core types and the `no_std` boundary | done |
| 1 | Windows: N satellites × M stations, OMM input, elevation masks, slew limits | done |
| 2 | Scheduler: greedy (flight-safe), then optimal ILP (ground-only) | **built, awaiting sign-off** |
| 3 | Explainable output: every pass scheduled or rejected says why | **built, awaiting sign-off** |
| 4 | Plug-ins: Chronus export, SatNOGS-compatible format, full CLI | next |
| 5 | WASM browser demo | later |
| — | Onboard pass prediction: heap-free, on the `sgp4` crate, validated against Ephemerust | later |

## Credits

- [Ephemerust](https://crates.io/crates/ephemerust) by the same author provides OMM and TLE
  parsing and pass prediction. It builds on the [`sgp4`](https://crates.io/crates/sgp4) crate.
- [`good_lp`](https://crates.io/crates/good_lp) models the optimal scheduler's integer program,
  and [`microlp`](https://crates.io/crates/microlp), a pure-Rust solver, solves it.
- [`chrono`](https://crates.io/crates/chrono) for calendar time,
  [`serde_json`](https://crates.io/crates/serde_json) for JSON output and
  [`thiserror`](https://crates.io/crates/thiserror) for error types. All ground layer only.
- The reference ISS element set used in tests and examples is public data. The other satellites
  in tests and examples are synthetic.

Minimum supported Rust version: **1.89**. License: [MIT](LICENSE).
