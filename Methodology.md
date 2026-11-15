# Methodology — Ma'at

A living record of **why** the project is built the way it is: major decisions, trade-offs and
the reasoning behind them. Append new entries as decisions are made. Do not silently rewrite
history; mark an entry superseded or amended instead.

> **Status (2026-11-14): Stage 0 approved. Stage 1 built, awaiting owner sign-off.** Satellites
> load from OMM or TLE, and the window survey lists every contact opportunity for N satellites
> over M stations. Stage 2 (the scheduler) has not started.

---

## Decision log

### D-001 — Three crates, with the flight/ground boundary enforced by the compiler
**Decision:** A Cargo workspace with `maat-flight` (vehicle layer), `maat-ground` (human layer,
depends on flight) and `maat-cli` (depends on ground). `maat-flight` is `#![no_std]` and its
`[dependencies]` table is empty.
**Why:** The project's claim is that one scheduling core serves the spacecraft and the operator.
A claim like that is only worth making if it is checked. With `#![no_std]` and no dependencies,
any use of `std`, the heap or a ground-layer type in the flight crate is a compile error. CI adds
two checks: a build for `thumbv7em-none-eabihf` (bare-metal Cortex-M4F, no `std`, no allocator)
and an assertion that the flight crate's dependency tree is one line long.
**Trade-off:** The flight crate cannot use convenient crates such as `chrono` or `thiserror`.
Errors implement `core::fmt::Display` and `core::error::Error` by hand.

### D-002 — The flight crate does not depend on Ephemerust; contact windows are data
**Decision:** Pass prediction happens in `maat-ground` through Ephemerust. The flight crate
receives finished contact windows.
**Why:** The plan asked what Ephemerust would need for a lean, `no_std`-friendly path. The answer
from reading Ephemerust is that it is `std`-only throughout: it depends on `chrono`,
`serde_json` and `thiserror`, its error type wraps `std::io::Error` and carries `String`s, and its
pass prediction returns a `Vec`. Making it `no_std` would be a large change to a published 1.x
crate. It is also unnecessary: a scheduler needs to know *when* contacts are possible, not how
that was worked out. Treating windows as data keeps the flight crate small and lets windows come
from any source, including an uplinked table.
**Trade-off:** The flight crate cannot recompute windows onboard if its orbit knowledge changes.
**Owner direction (2026-11-14):** Confirmed. Ephemerust stays out of the flight crate. Onboard
prediction is wanted later, as a small heap-free pass predictor inside Ma'at written directly on
the `sgp4` crate (which supports `no_std`), tested against Ephemerust on the ground. It is on the
roadmap as its own item and does not change this decision.

### D-003 — Integer-only flight types: whole seconds and hundredths of a degree
**Decision:** `Timestamp` is an `i64` of whole seconds since the Unix epoch. `Seconds` is a `u32`.
`Elevation` is a `u16` of hundredths of a degree. The flight crate contains no floating point, and
the `clippy::float_arithmetic` lint keeps it that way.
**Why:** Integer arithmetic gives the same bits on every processor, with or without a
floating-point unit, so a plan computed on the ground and one computed onboard cannot disagree by
a rounding mode. Every type can derive `Eq` and `Hash`. The resolution is honest: pass times from
public element sets are good to about a second, so finer storage would be false precision.
**Trade-off:** Sub-second timing is not representable. A window is up to two seconds shorter than
the predicted pass (see D-005).

### D-004 — Contact windows are half-open intervals `[AOS, LOS)`
**Decision:** A window includes its AOS second and excludes its LOS second. Two windows overlap
only if they share at least one second.
**Why:** With half-open intervals, a pass ending at second *t* and another starting at *t* do not
conflict, durations are simply `los - aos`, and adjacent windows tile time without gaps or double
counting. It is the same convention Rust uses for ranges.
**Verification:** `overlaps` is cross-checked against the definition (some second lies in both
windows) exhaustively over every pair of windows in a small range.

### D-005 — Values round inward when they cross from ground to flight
**Decision:** `window_from_pass` rounds AOS up to the next whole second, LOS down, and maximum
elevation down to 0.01 degrees. Time conversion offers `timestamp_floor` and `timestamp_ceil` and
no unqualified "convert".
**Why:** A contact plan is a set of promises. Rounding outward could promise a second of contact
before the satellite clears the mask. Rounding inward means the integer window always lies inside
the real pass. Forcing the caller to choose floor or ceil makes the direction visible in the code.
**Tolerance:** contact time lost per window is in `[0 s, 2 s)`; elevation lost is in
`[0, 0.01 degrees]`. Both are asserted against real predicted passes in
`crates/maat-ground/tests/ephemerust_bridge.rs`.

### D-006 — Stable reason codes, declared from a single table
**Decision:** Every outcome has a `ReasonCode` with a fixed number, an identifier (`MAAT-101`)
and a one-line summary. The hundreds digit is the class: 0 accepted, 1 rejected, 2 invalid input,
3 resource limit. One `macro_rules!` table generates the enum, the lookups and `ReasonCode::ALL`.
Published numbers are never changed or reused; the enum is `#[non_exhaustive]`.
**Why:** Explainability is the project's distinguishing feature, and an explanation is only useful
if it is stable enough to search for, document and match on. A single table makes it impossible to
add a code without an identifier and a summary. A hand-written `FROZEN` list in the tests fails if
any published code is renumbered or removed.
**Scope note (amended in Stage 1):** The catalogue holds every reason a *pass* can be in or out of
a plan, whichever layer decides it. `MAAT-105` (slew rate) is decided on the ground during the
window survey and is in the catalogue because it explains a missing pass. Ground-only
*configuration and input* errors (a bad station, an unreadable catalog, a date out of range) are
ordinary `GroundError`s without a code.
**Trade-off:** `MAAT-101`, `102`, `104` and `301` are defined before the scheduler that will emit
them. They come directly from the Stage 2 requirements in the plan. If Stage 2 shows one is
wrong, it should be corrected before the first release, while nothing is published.

### D-007 — No panicking constructs in flight library code
**Decision:** `maat-flight` denies `clippy::unwrap_used`, `expect_used`, `panic`,
`indexing_slicing` and `arithmetic_side_effects` outside tests, and forbids `unsafe`.
**Why:** Onboard, a panic is a fault. Denying the constructs that can panic or silently wrap turns
"we were careful" into something the build checks. Fallible operations return `Option` or `Result`.
**Verification:** Adding an `unwrap`, an unchecked `+` or a `std` import to the flight crate was
confirmed to fail the build.
**Limit:** These lints cover the common panic sources, not all of them. They are a discipline, not
a proof of panic freedom.

### D-008 — Validated constructors and private fields
**Decision:** `ContactWindow`, `Elevation`, `GroundStation` and `Satellite` have private fields
and can only be built through constructors that check their input.
**Why:** If an invalid value cannot be constructed, code that receives one does not need to
re-check it. The scheduler can rely on `los > aos` everywhere. `GroundStation` takes its position
as a `StationSite` struct with named fields so latitude and longitude cannot be swapped silently.

### D-009 — Conventions carried over from Ephemerust and Chronus Gateway
**Decision:** Edition 2024, MSRV 1.89, MIT license, resolver 3, workspace-level dependency
versions, `cargo deny` policy, layered tests (unit, integration, doctests), deterministic and
offline tests, and a stage-gate protocol: a stage is complete only when its deliverables exist,
its tests pass and the owner confirms the gate. Stages are not chained.

### D-010 — Ephemerust 1.2.1 is the floor
**Decision:** `maat-ground` requires `ephemerust >= 1.2.1`.
**Why:** 1.2.0 added what Stage 1 needs: OMM input in four formats, and
`Propagator::predict_passes`, which lets one initialized propagator be searched against many
stations. 1.2.1 fixed the pass search.
**Review of 1.2.0 (2026-11-14):** The published crate matched the owner's local source file for
file. Its own suite passed (212 unit, 33 integration, 45 doc tests) and clippy was clean. A
mutation probe fed 147,332 truncated and corrupted OMM inputs to the four parsers with no panic.
One finding mattered here and predated 1.2.0: the pass search sampled at a fixed 1/48 of the
orbital period and could step over a shorter pass. Against a one-second brute-force scan of three
orbits, five latitudes and three masks over seven days it missed 10 of 978 passes, all grazing
passes of 21 to 95 s.
**Review of 1.2.1 (2026-11-14):** The owner replaced the fixed step with safe steps that provably
cannot jump over a pass. The same probe now finds all 978, with none spurious and every edge
inside the scan's one-second bracket. Its suite passes (220 unit, 33 integration, 45 doc tests)
and the parser probe still shows no panic.
**Consequence:** Ma'at carries no workaround for short passes. It keeps its own smaller
brute-force comparison as an independent check (`tests/window_survey.rs`).

### D-011 — The window survey: three lists, one propagator per satellite, fixed order
**Decision:** `contact_windows(satellites, stations, start, end)` returns a `WindowSurvey` with
three lists: usable `windows`, `rejected` passes with a reason code and a detail string, and
`failures` for satellite/station pairs that could not be searched. Windows are sorted by AOS, then
LOS, station and satellite.
**Why three lists:** The plan's promise is that every pass says why it is in or out. That has to
start where passes are found. A pass that is dropped without a trace in Stage 1 can never be
explained in Stage 3.
**Why failures do not abort:** A real catalog contains objects that are about to decay or whose
elements are stale. SGP4 starts from such an element set and then diverges partway through the
search. Failing the whole survey for one of them would make the tool unusable on real data;
swallowing the failure would be silent. Recording it per pair is neither. Problems with the
*request* (a search window that ends before it starts, duplicate identifiers) are still returned
as an error, because nothing useful can be computed from them.
**Why a fixed order:** The output does not depend on the order of the inputs, so two runs on the
same data are comparable line by line, and the greedy scheduler in Stage 2 receives its input
already ordered by time.
**One propagator per satellite:** `Satellite` initializes SGP4 once when it is built and reuses it
for every station. An element set SGP4 cannot start from is therefore refused at load time.
**Verification:** The survey is compared with calling Ephemerust pair by pair (identical), with a
one-second brute-force elevation scan (every pass present, every edge exact on the set tested),
across all four OMM formats (identical windows), and for a TLE against the OMM of the same
element set (identical windows).

### D-012 — Slew limits: test the whole pass against the largest one-second average rate
**Decision:** A station may carry `SlewLimits` (degrees per second in azimuth and in elevation).
For such a station the survey samples each pass's look angles once a second from AOS to LOS and
takes the largest change per second about each axis. If either exceeds its limit the pass is
rejected with `MAAT-105`, and the detail gives the rate needed and the rate allowed. A pass
longer than 2000 s is sampled at 2000 evenly spaced points.
**Why sample:** It needs no model of the mount or the orbit, handles the wrap of azimuth at north
with one function, and works for any orbit.
**Why reject the whole pass:** A pass that goes nearly overhead is usable before and after the
fast section, so a smarter survey would offer two shorter windows. That is deferred: it changes
one pass into two and needs a model of how long the mount takes to catch up. Rejecting is the
conservative choice and is stated as a limitation.
**Verification and tolerances (three days of ISS passes over one station, 16 passes peaking from
10 to 86 degrees):**
- Against the closed form `v_rel / (range · cos(elevation))` at culmination with
  `v_rel = 7.4 km/s`: within 0.8 % for peaks below 80 degrees, 2.2 % low at 86 degrees.
  Test tolerance 3 %.
- Against the same measurement at a 0.1 s step: azimuth within 0.05 % below 80 degrees and 1.6 %
  at 86 degrees; elevation within 1.1 %. Test tolerances 0.1 %, 3 % and 2 %.
- Elevation rate never exceeds the orbital angular rate seen from below, about 1.05 deg/s for
  the ISS. Measured maximum 0.91 deg/s. Test bound 1.2 deg/s.
**Limit:** The figure is an average over one second, slightly below the instantaneous peak when
the peak is sharp. Slew *between* passes (turning from the end of one to the start of the next)
is a scheduling constraint and is not part of Stage 1; see O-6.

### D-013 — An orbit that passes through the Earth is refused when it is loaded
**Decision:** `Satellite` computes the perigee height from the mean motion and eccentricity
(`a = (mu / n²)^(1/3)`, perigee at `a(1 − e)`) and returns `GroundError::ImpossibleOrbit` if it is
below the surface.
**Why:** While testing, an element set with 20 revolutions per day (about 650 km underground) was
found to be accepted by SGP4, which then reported positions below the surface and an empty list of
passes with no error. A set with a normal mean motion and an eccentricity of 0.3 produced passes.
A corrupted catalog entry would therefore yield a quiet wrong answer.
**Scope:** The threshold is the surface, not a "sensible" altitude. A 190 km perigee is about to
decay but is real, and is accepted; if it diverges during the search, D-011 reports it.
**Tolerance:** The mean motion is an SGP4 mean element, so the height is good to a few kilometres.
Checked against the ISS (about 420 km) and geostationary height (35 786 km) to within 15 km.
**Note for Ephemerust:** `Propagator::from_omm`'s documentation says initialization fails when
perigee is inside the Earth. On 1.2.1 it does not for these cases. Either the check or the
sentence could move there.

### D-014 — Ground errors keep their source
**Decision:** `GroundError` variants that wrap an Ephemerust failure hold the original
`AstroError` as their `source`, and `GroundError::hint()` passes on Ephemerust's corrective hint.
`GroundError` no longer derives `Clone` or `PartialEq`.
**Why:** Ephemerust's errors are written to teach (which keyword is missing, which value is out
of range, what to do about it). Flattening them to a string would throw that away.
**Trade-off:** `AstroError` contains an I/O error and so cannot be cloned or compared. Tests match
on variants with `matches!` instead of `assert_eq!`.

---

## Open decisions

| # | Question | Needed by |
|---|---|---|
| O-1 | **Final name.** The repository is `IsomorphicAlgo/Ma-at`. `maat` is taken on crates.io; `maat-flight`, `maat-ground` and `maat-cli` were free on 2026-11-14. | First publish |
| O-2 | ~~OMM input.~~ **Resolved:** Ephemerust 1.2 (D-010). | — |
| O-3 | **Fixed-size storage for the scheduler.** Caller-provided slices (`&[ContactWindow]` in, `&mut [Decision]` out), a small in-crate fixed-capacity vector, or the `heapless` crate. The first keeps the flight crate dependency-free. | Stage 2 |
| O-4 | **Serialization of flight types.** An optional `serde` feature on `maat-flight`, or mirror types in `maat-ground`. | Stage 3 |
| O-5 | **Priority ties.** What decides between two overlapping passes of equal priority, and whether that outcome needs its own reason code. | Stage 2 |
| O-6 | **Turnaround between passes.** A steerable antenna needs time to turn from the end of one pass to the start of the next. Modelling it needs the azimuth and elevation at AOS and LOS, which `ContactWindow` does not carry. Either add them to the flight type (as integers) or give each station a fixed turnaround time. | Stage 2 |
| O-7 | **Minimum pass length and data need.** Where the per-satellite inputs to the scheduler live (priority, minimum useful contact, contact time wanted). | Stage 2 |
| O-8 | **Element-set age.** Whether the survey should warn, or refuse, when the search is far from an element set's epoch. | Stage 3 |
| O-9 | **Station file format** for the CLI (TOML, JSON or CSV), and loading many TLEs from one file (Ephemerust has `Omm::parse_all` but no TLE counterpart). | Stage 4 |

---

## Stage gates

### Stage 0 — Core types and the `no_std` boundary: **approved by the owner, 2026-11-14**

| Criterion | Evidence | State |
|---|---|---|
| Shared types exist: satellite, station, window, reason code | `maat-flight`: `SatelliteId`, `StationId`, `Priority`, `Timestamp`, `Seconds`, `Elevation`, `ContactWindow`, `ReasonCode` | met |
| The `no_std` boundary is enforced | `#![no_std]`, empty dependency list, host build passes; a `std` import was confirmed to fail | met on host |
| Ephemerust's `no_std` path assessed | D-002, confirmed by the owner | met |
| A pass crosses the boundary correctly | `tests/ephemerust_bridge.rs`, 10 tests on real ISS passes | met |
| Tests green | 104 passing on Rust 1.97 (Linux); `cargo test --workspace` run by the owner on Windows | met |
| Lints and docs clean | `cargo fmt --check`, `cargo clippy -D warnings`, `cargo doc -D warnings` | met |
| Bare-metal build of `maat-flight` | CI job `no-std` (`thumbv7em-none-eabihf`); workflow added by the owner | **awaiting the first CI run** |
| MSRV 1.89 build | CI job `test (1.89)` | **awaiting the first CI run** |
| Owner sign-off | "I approve stage 0" | given |

### Stage 1 — Windows

| Criterion | Evidence | State |
|---|---|---|
| OMM input | `satellites_from_omm`; JSON, KVN, XML and CSV fixtures give identical surveys; a six-digit catalog number reaches the windows and a nine-digit one is accepted as an identifier | met |
| Pass windows for N satellites × M stations | `contact_windows`; 2 × 4 fixture equals Ephemerust pair by pair | met |
| Elevation masks | Per-station mask; a higher mask gives fewer and shorter windows | met |
| Slew limits | `SlewLimits`, `MAAT-105`; D-012 tolerances | met, with the limits stated in D-012 |
| Output is a plain list of contact opportunities | `WindowSurvey::windows`: sorted flight `ContactWindow`s | met |
| Nothing dropped silently | Every pass is a window, a rejection or a failure; a decaying satellite is reported and the rest still surveyed | met |
| Independent cross-check | One-second brute-force scan: every pass present, every edge exact, on 2 orbits × 2 stations (masks of 10 and 40 degrees) over one day | met |
| Tests green | 176 passing: 125 unit, 33 integration, 18 doctests, on Rust 1.97 (Linux) | met here; **not yet run on Windows or 1.89** |
| Tests catch mistakes | 14 deliberate faults (wrap, sort, masks, checks, sampling) each failed at least one test | met |
| Lints and docs clean | `cargo fmt --check`, `cargo clippy -D warnings`, `cargo doc -D warnings` | met |
| Owner sign-off | | **pending** |

---

## Attribution

- **Ephemerust** (the owner's astrodynamics crate) provides OMM and TLE parsing and pass
  prediction; it builds on the **`sgp4`** crate. MIT.
- **`chrono`** (MIT/Apache-2.0) and **`thiserror`** (MIT/Apache-2.0), ground layer only.
- The reference ISS two-line element set in tests and examples is public data, also used in
  Ephemerust's documentation. All other element sets in tests and examples are synthetic.
- The azimuth-rate formula in D-012 is standard tracking geometry (line-of-sight rate divided by
  the cosine of elevation); it is used only as a test cross-check.
- Stages 0 and 1 were drafted with AI assistance (Claude) and are subject to owner review.

## Scope and export control

Like its sibling projects, Ma'at is limited to open, public standards and public data: published
element sets, generic ground-station geometry and textbook scheduling. Examples use public
reference objects such as the ISS, synthetic element sets, and antenna figures typical of amateur
equipment. It contains no controlled technical data and no parameters of any real mission's
operations.
