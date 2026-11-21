# Ma'at

*Ma'at is the Egyptian principle of balance, order and fair judgment.*

A Rust workspace that turns satellites, ground stations and priorities into a conflict-free,
explainable contact plan. It is built on [Ephemerust](https://crates.io/crates/ephemerust), and
its scheduling core is designed to run on the ground **and** onboard a spacecraft.

> **Status: Stage 5 of 5 — a browser demo (built, awaiting sign-off).** Ma'at loads
> satellites from OMM or TLE, finds every contact opportunity over a set of ground stations,
> schedules them with a greedy scheduler (flight-safe) or an optimal one (ground only), and gives
> a stated reason for every pass, scheduled or not. The `maat plan` command does all of that from
> a catalog file and a TOML station file, and writes text, JSON, SatNOGS observations or one
> station's schedule.

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
cargo run -p maat-cli -- plan --catalog examples/catalog.json --stations examples/stations.toml --start 2026-08-01T12:00:00Z --hours 6
cargo run -p maat-ground --example contact_plan
cargo run -p maat-cli -- explain MAAT-107
```

The `plan` command and the example both plan six hours for five synthetic satellites over four
stations and print the same six-hour plan; the example then goes on to compare a week. Every pass that is not scheduled says why, and names what
beat it:

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
  AOS (UTC)              LENGTH  SATELLITE  STATION         PEAK  PRI
  2026-08-01 12:04:53     240 s  EXAMPLE-E  Seattle    12.78 deg    -
      MAAT-106: dropped: no contact request covers this satellite
      no contact request names EXAMPLE-E (catalog number 100126)
  2026-08-01 12:19:34     160 s  EXAMPLE-B  Svalbard   11.56 deg    4
      MAAT-103: dropped: the pass is shorter than the minimum contact length
      the pass lasts 160 s and EXAMPLE-B asks for passes of at least 180 s
  2026-08-01 12:20:23     158 s  EXAMPLE-C  Svalbard   11.50 deg    4
      MAAT-103: dropped: the pass is shorter than the minimum contact length
      the pass lasts 158 s and EXAMPLE-C asks for passes of at least 180 s
  2026-08-01 12:45:48     268 s  EXAMPLE-E  Cape Town  13.50 deg    -
      MAAT-106: dropped: no contact request covers this satellite
      no contact request names EXAMPLE-E (catalog number 100126)
  2026-08-01 15:26:30     374 s  EXAMPLE-E  Seattle    18.63 deg    -
      MAAT-106: dropped: no contact request covers this satellite
      no contact request names EXAMPLE-E (catalog number 100126)
  2026-08-01 15:43:37     415 s  EXAMPLE-E  Quito      22.65 deg    -
      MAAT-106: dropped: no contact request covers this satellite
      no contact request names EXAMPLE-E (catalog number 100126)
  2026-08-01 16:41:27     430 s  EXAMPLE-C  Quito      55.25 deg    4
      MAAT-107: dropped: lost a tie with an equal-priority pass at the same station
      EXAMPLE-B has the same priority (4) and has Quito from 2026-08-01 16:40:39 to 2026-08-01 16:47:51; it wins the tie because its pass is longer (432 s against 430 s)
  2026-08-01 16:47:15     419 s  EXAMPLE-D  Quito      43.31 deg    2
      MAAT-101: dropped: overlaps a higher-priority pass at the same station
      EXAMPLE-B (priority 4) has Quito from 2026-08-01 16:40:39 to 2026-08-01 16:47:51; this pass has priority 2
  2026-08-01 17:05:24     529 s  EXAMPLE-E  Seattle    79.76 deg    -
      MAAT-105: dropped: tracking the pass needs a faster antenna slew than the station allows
      tracking needs 3.84 deg/s in azimuth (the station allows 2.00)
  2026-08-01 17:23:39     324 s  EXAMPLE-E  Quito      15.96 deg    -
      MAAT-106: dropped: no contact request covers this satellite
      no contact request names EXAMPLE-E (catalog number 100126)

SATELLITES
  NAME       PRI       NEED  SCHEDULED  PASSES  NEED MET
  EXAMPLE-A    9     1200 s      654 s   2 of 2   NO
  EXAMPLE-B    4        all      432 s   1 of 2   -
  EXAMPLE-C    4        all        0 s   0 of 2   -
  EXAMPLE-D    2        all      132 s   1 of 2   -
  EXAMPLE-E    -          -        0 s   0 of 6   -

STATIONS
  NAME       TURNAROUND       SLEW AZ/EL  SCHEDULED  PASSES
  Seattle         120 s  2.00/2.00 deg/s        0 s   0 of 3
  Quito             0 s                -      432 s   1 of 5
  Svalbard         60 s                -      132 s   1 of 3
  Cape Town         0 s                -      654 s   2 of 3

GREEDY AGAINST OPTIMAL, ONE WEEK
  greedy   165 of 615 passes scheduled,  58637 s of contact, value 285216
  optimal  171 of 615 passes scheduled,  60761 s of contact, value 291548
  The optimal plan is worth 2.2 % more.
  It is proven: no plan is worth more (time limit 10 s).

CONTACT PER SATELLITE, ONE WEEK (seconds)
               greedy  optimal optimal with floor
  EXAMPLE-A      1227     1227               1227
  EXAMPLE-B     35317    18216              35317
  EXAMPLE-C     15176    32257              15176
  EXAMPLE-D      6917     9061               6917
  value        285216   291548             285216
  With the floor the optimal plan is worth 0.0 % more than greedy.

A PASS LOST TO ANTENNA TURNING TIME
  EXAMPLE-D over Seattle, 2026-08-07 09:14:58
  MAAT-108: dropped: too close to another pass for the station to re-point in time (Seattle needs 158 s between these two passes (120 s fixed plus 38 s to turn its antenna from the end of one to the start of the other) and this pass begins 122 s after the pass of EXAMPLE-B (from 2026-08-07 09:08:10 to 2026-08-07 09:12:56))
```

Add `-- --json` to the example for the same plan as JSON.

In code:

```rust
use chrono::Duration;
use maat_ground::flight::{Priority, SatelliteRequest, Seconds, StationId};
use maat_ground::{
    GroundStation, Scheduler, SlewLimits, StationSite, contact_windows, plan_contacts,
    satellites_from_omm,
};

// OMM text in JSON, KVN, XML or CSV; the format is detected.
let satellites = satellites_from_omm(&std::fs::read_to_string("catalog.json")?)?;

let seattle = GroundStation::new(
    StationId::new(1),
    "Seattle",
    StationSite { latitude_deg: 47.61, longitude_deg: -122.33, altitude_m: 50.0 },
    10.0, // elevation mask, degrees
)?
// How fast the antenna turns. A pass that needs a faster turn is not usable, and the time to
// turn from the end of one pass to the start of the next is kept free between them.
.with_slew_limits(SlewLimits { azimuth_deg_per_s: 6.0, elevation_deg_per_s: 2.5 })?
.with_turnaround(Seconds::new(30)); // fixed time between passes, on top of the turn
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

The optimal scheduler takes a time limit. A plan is needed before its first pass begins, so that
is the longest limit worth giving:

```rust
use maat_ground::{PlanOptions, plan_contacts_with, time_until_first_pass};

let limit = time_until_first_pass(&survey, chrono::Utc::now()).unwrap_or_default();
let options = PlanOptions::new(Scheduler::Optimal).with_optimal_time_limit(limit);
let plan = plan_contacts_with(&survey, &satellites, &stations, &requests, &options)?;
// `plan.optimality` says whether the plan is proven best or the best found in the time.
```

## From files: `maat plan`

```text
maat plan --catalog <FILE> --stations <FILE> [--requests <FILE>]
          [--start 2026-08-01T12:00:00Z] [--hours 24]
          [--scheduler greedy|optimal] [--time-limit <SECONDS>] [--floor]
          [--format text|json|satnogs|station] [--station <ID>] [--output <FILE>]
```

- **The catalog** is OMM (JSON, KVN, XML or CSV) or a file of two-line element sets, named or
  not. The format is detected.
- **The station file** is TOML: one `[[station]]` table per ground station and one `[[request]]`
  table per satellite to schedule. Requests can go in a second file. See
  [`examples/stations.toml`](examples/stations.toml) for every key. A key that is not known is
  an error, so a misspelt option is reported with its line and not ignored.
- **The plan goes to standard output**, or to `--output`. Notes and warnings go to standard
  error: the search window used, an incomplete survey, a plan that is not proven optimal,
  element sets more than 14 days from the search, and requests for satellites that are not in
  the catalog.
- **Exit status:** 0 for a plan, 1 when the inputs could not be turned into one, 2 for a
  command line that makes no sense.

| `--format` | What is written |
|---|---|
| `text` | The plan for a person, as above |
| `json` | The whole plan, `maat.contact-plan/1`: every pass, scheduled or not, with its reason |
| `satnogs` | The scheduled passes as a list of SatNOGS Network observations |
| `station` | One station's scheduled passes, `maat.station-schedule/1` |

### What the two exports are, and are not

- **`satnogs`** writes a JSON array of objects with exactly the four fields the
  [SatNOGS Network](https://network.satnogs.org) takes for a new observation:
  `ground_station`, `transmitter_uuid`, `start` and `end`. SatNOGS has its own numbers for
  stations and transmitters; give them as `satnogs_id` on a station and `satnogs_transmitter` on
  a request. A scheduled pass without both is left out and named on standard error. **This
  output has not been sent to the network.** That needs an account, a station you may schedule
  on, and a transmitter the station supports, and the network applies checks of its own.
- **`station`** is for a station running
  [Chronus Gateway](https://github.com/IsomorphicAlgo/chronus-gateway): what to track and when,
  with the site written in the same keys as the gateway's `[station]` configuration.
  **Chronus Gateway (1.0.0-rc.1) has no schedule input**, so nothing reads this file yet. It is
  Ma'at's side of an interface the gateway does not have.

## In a browser

`demo/` is a page that runs the scheduler itself, compiled to WebAssembly. Choose satellites,
priorities and ground stations, build the schedule, and replay the greedy scheduler's decisions
one at a time. Every pass, scheduled or not, shows its reason; selecting a pass that lost
highlights the pass that beat it. Nothing is sent to a server.

Build it once, then serve the folder (a browser will not load WebAssembly from a file opened
directly):

```text
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version 0.2.130 --locked
./demo/build.ps1                                   # or: sh demo/build.sh
python -m http.server 8000 --directory demo        # then open http://localhost:8000
```

- **It is the same code.** `crates/maat-wasm` adds no scheduling logic: it passes text to
  `maat-ground` and passes the plan back. The page turns your choices into the same TOML station
  file the `maat` command reads, and shows it to you.
- **It makes the same plan.** `node demo/check.mjs <native-plan.json>` runs the WebAssembly build
  and compares it with the native command's output. On a week of the example fleet every pass,
  time, reason and total matched; four peak elevations out of 615 differed in the fourteenth
  decimal place, because the two builds use different sine and cosine routines.
- **The replay order is the scheduler's.** The bridge sorts the passes with the flight crate's own
  ranking function, and a test replays a week in that order to confirm that nothing scheduled
  ever conflicts with what came before it.

## Two schedulers

| | Greedy | Optimal |
|---|---|---|
| Where | `maat-flight`: ground and onboard | `maat-ground`: ground only |
| Method | Best window first, never reconsidered | Integer linear program, solved exactly |
| Priority means | A strict rank: higher always wins | A weight: `priority + 1` per second |
| Memory | None of its own; the caller's slice | Allocates |
| Run time | Bounded: proportional to n² | Up to a time limit you set (default 60 s) |
| Measured, 3000 windows | about 25 ms | 0.1 to 0.5 s, once 2.8 s |
| Result | The same on every machine | The same on every machine when proven optimal |

They are two separate schedulers; a plan comes from one or the other. Both obey the same rules,
through the same function: a station serves one pass at a time and needs time between passes; a
satellite uses one station at a time; a pass shorter than a satellite's minimum is not used;
contact beyond a satellite's need is not scheduled.

**Time between passes** at a station has two parts: a fixed turnaround, and, for a station with
slew limits, the time to turn its antenna from where one pass ends to where the next begins.
The second part differs for every pair of passes, and both schedulers work it out pair by pair.

**The optimal plan is never worth less than the greedy plan in total.** The search starts from
the greedy plan and keeps it unless it finds a better one. That is a statement about the whole
plan. It is not a promise to each satellite: the optimal plan can give one satellite less than
greedy did in order to give others more.

The optimal scheduler exists to measure what greedy gives up. On crowded random problems it was
worth 2 to 15 % more; on a week of realistic orbits over four stations, about 2 %.

### A floor for each satellite

The optimal plan's gain can come out of one satellite's share. In the example week it is worth
2.2 % more than greedy in total, and EXAMPLE-B loses half its contact to EXAMPLE-C, which has
the same priority and shares its orbit:

```text
CONTACT PER SATELLITE, ONE WEEK (seconds)
               greedy  optimal optimal with floor
  EXAMPLE-A      1227     1227               1227
  EXAMPLE-B     35317    18216              35317
  EXAMPLE-C     15176    32257              15176
  EXAMPLE-D      6917     9061               6917
  value        285216   291548             285216
```

With a floor (`--floor`, or `PlanOptions::with_satellite_floor(true)`), no satellite gets less
useful contact than the greedy plan gives it, and the optimal scheduler improves the plan only
where nobody pays for it. **What that costs depends on the fleet.** In this week it costs the
whole gain: three of the satellites share an orbit and compete for the same passes, so every
gain is someone's loss. On 51 crowded random problems where the optimal plan beat greedy, the
floor kept 84 % of the gain on average: all of it in 31 problems, none of it in 4. It is off by
default.

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
| `MAAT-206` | invalid input | invalid window: an azimuth is outside 0 to 360 degrees |
| `MAAT-301` | resource limit | not evaluated: a fixed-size plan buffer is full |

An explanation has three levels:

1. the **code and summary** above: stable, one line, held in the flight crate;
2. the **detail** on each plan entry: the satellites, stations, times and numbers involved;
3. the **guidance** from `maat explain MAAT-107`: what the code means and what would change the
   outcome.

## Design in brief

- **Integer-only flight types.** Time is whole UTC seconds; azimuth and elevation are
  hundredths of a degree. No floating point in the flight crate means bit-identical results on every processor.
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
- **The optimal scheduler can run out of time.** The problem is NP-hard in general. Problems of
  up to 3000 windows usually solved in under half a second in testing, but the worst case is
  bounded only by the time limit. When the limit is reached the plan is the best found so far,
  marked "not proven optimal", and can differ between machines of different speed. The solver's
  answers are checked against exhaustive search only on small problems (up to 16 windows).
- **The optimal plan is better in total, not for every satellite**, unless a floor is asked
  for. With a floor, the gain over greedy can shrink to nothing (see above), and one
  3000-window problem took 8 to 10 s to solve where it took half a second without.
- **The exports are untested against their targets.** The SatNOGS output matches the documented
  request but has never been sent to the network. The station schedule has no reader yet.
- **The demo page was checked in one browser.** Headless Chrome on Linux: it loads, builds greedy
  and optimal plans, replays, selects by mouse and keyboard, reports a bad input, and fits a
  phone-width screen without sideways scrolling. It has not been tried in Firefox or Safari, and
  no automated browser test is in the repository.
- **The optimal scheduler holds up the page while it searches.** The demo limits it to five
  seconds. It runs on the page's own thread, not in a worker.
- **Element age is a warning, not a model.** `maat plan` warns when element sets are more than
  14 days from the search window. That figure is a rule of thumb. Nothing in a plan widens a
  window or lowers confidence as elements age.
- **A pass is used whole.** Neither scheduler trims a pass to fit, so a satellite can receive
  more contact than it asked for, by less than one pass.
- **Turning time assumes a simple mount.** Both axes move at once at their constant top rates,
  and azimuth goes the short way round. Acceleration, cable-wrap limits that force the long way
  round, and keyhole behaviour near the zenith are not modelled. Allow for them in the fixed
  turnaround time.
- **Turning time needs pointing.** It is applied when a window records where the antenna points
  at its start and end, which every window from `contact_windows` does. A window built by hand
  without pointing gets the fixed turnaround only.
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
| 2 | Scheduler: greedy (flight-safe), then optimal ILP (ground-only) | done |
| 3 | Explainable output: every pass scheduled or rejected says why | done |
| 4 | Plug-ins: Chronus export, SatNOGS-compatible format, full CLI | done |
| 5 | WASM browser demo | **built, awaiting sign-off** |
| — | Onboard pass prediction: heap-free, on the `sgp4` crate, validated against Ephemerust | later |

## Credits

- [Ephemerust](https://crates.io/crates/ephemerust) by the same author provides OMM and TLE
  parsing and pass prediction. It builds on the [`sgp4`](https://crates.io/crates/sgp4) crate.
- [`good_lp`](https://crates.io/crates/good_lp) models the optimal scheduler's integer program,
  and [`microlp`](https://crates.io/crates/microlp), a pure-Rust solver, solves it.
- [`chrono`](https://crates.io/crates/chrono) for calendar time,
  [`serde_json`](https://crates.io/crates/serde_json) for JSON output and
  [`thiserror`](https://crates.io/crates/thiserror) for error types. All ground layer only.
- [`toml`](https://crates.io/crates/toml) and [`serde`](https://crates.io/crates/serde) read
  station files, behind `maat-ground`'s optional `scenario` feature.
- [`wasm-bindgen`](https://crates.io/crates/wasm-bindgen) connects the browser demo to the
  scheduler.
- The SatNOGS observation fields follow the SatNOGS Network's published API. SatNOGS is a
  project of the Libre Space Foundation; Ma'at is not affiliated with it.
- The reference ISS element set used in tests and examples is public data. The other satellites
  in tests and examples are synthetic.

Minimum supported Rust version: **1.89**. License: [MIT](LICENSE).
