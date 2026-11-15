# Methodology — Ma'at

A living record of **why** the project is built the way it is: major decisions, trade-offs and
the reasoning behind them. Append new entries as decisions are made. Do not silently rewrite
history; mark an entry superseded or amended instead.

> **Status (2026-11-15): version 0.4.0. Stages 0 and 1 approved. Stages 2 and 3 built together at
> the owner's direction, awaiting sign-off.** Ma'at schedules contact windows with a greedy
> scheduler in the flight crate or an optimal one on the ground, and every pass in the resulting
> plan carries a reason code and a detail sentence. Stage 4 (exports and the full CLI) has not
> started.

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
**Trade-off (resolved in Stage 2):** `MAAT-101`, `102`, `104` and `301` were defined before the
scheduler that emits them. Stage 2 kept all four with their meanings unchanged and added six more
(D-019). Every code in the catalogue is now produced by code and covered by a test.

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

### D-015 — Versioning: one minor version per stage until 1.0
**Decision:** The three crates share one version. Stage 0 is 0.1.0, Stage 1 is 0.2.0, and each
later stage takes the next minor version: 0.3.0 for the scheduler, 0.4.0 for explainable output,
0.5.0 for the plug-ins and full CLI. Fixes within a stage are patch releases. 1.0.0 is for when
the scheduler and its output have been used enough that the public API can be frozen.
**Why 0.2.0 for Stage 1 and not 0.1.0:** Stage 1 changed the public API that Stage 0 set
(`GroundError` lost `Clone` and `PartialEq`, see D-014). Before 1.0, Cargo treats a change of
minor version as breaking and a change of patch version as compatible, so an incompatible change
has to move the minor number. It also makes the version say where the project is: 0.2 has
windows, 0.3 will have a scheduler.
**Why not 1.0 yet:** The scheduler does not exist, and four of the ten reason codes have never
been emitted. Calling that 1.0 would promise a stability the project has not earned.
**Publishing:** Nothing is on crates.io yet. A version here records a state of the repository; a
git tag (`v0.2.0`) marks it. Publishing is a separate decision, and `maat` itself is taken there
(O-1).
**0.3.0 was skipped:** Stages 2 and 3 were built together at the owner's direction, so the
repository went from 0.2.0 to 0.4.0. The changelog keeps the two stages apart inside 0.4.0.

### D-016 — The greedy scheduler: best first, on the caller's memory
**Decision:** `maat_flight::schedule_greedy(windows, requests, stations, decisions)` considers
windows best first and schedules each one unless something already scheduled rules it out. It
writes one `Decision` per window into a slice the caller provides and keeps no state of its own.
This settles four open questions:
- **O-3, storage:** caller-provided slices. The flight crate stays free of dependencies and of
  any fixed capacity of its own; an onboard caller sizes the array, a ground caller uses a `Vec`.
  A slice that is too short is refused whole with `MAAT-301`, and nothing is written.
- **O-5, ties:** the ranking is priority, then the longer pass, then the earlier AOS, then the
  lower station number, the lower satellite number, and last the position in the input. It is a
  total order, so the plan does not depend on input order or processor. A pass that loses to an
  equal priority is told so with its own code (`MAAT-107`), because "a higher-priority pass" would
  be untrue and the remedy is different.
- **O-6, turnaround:** one fixed time per station (`StationRule`), applied before and after
  every scheduled pass. A pass inside the gap gets `MAAT-108`. Pointing-dependent turnaround
  would need azimuth and elevation in `ContactWindow` and is left for later.
- **O-7, per-satellite inputs:** `SatelliteRequest` in the flight crate holds priority, minimum
  contact and need. A satellite with no request is not scheduled (`MAAT-106`).
**Order of checks:** no request, too short, need met, station overlap, station turnaround,
satellite overlap. A met need is reported ahead of a conflict because freeing the station would
not get the pass scheduled.
**Why selection and not a sort:** each round scans the windows for the best one still pending,
then scans them again to judge it. That is `n²` steps with no scratch memory, no recursion and a
run time that depends only on the counts `n`, `r` and `s`, which is what "deterministic run time"
asks for. Measured (release build, one desktop core): 1000 windows in 1 to 3 ms, 3000 in about
25 ms.
**Assumptions, stated in the docs:** one pass at a time per station, one station at a time per
satellite, and a pass is used whole.
**Verification:** 52 unit tests, which pin down each rule and the boundary of every comparison. One
of them builds 400 crowded problems and checks each plan with a verifier written
independently of the scheduler: no two scheduled passes conflict, and every stated reason is
true (the named blocker is scheduled, really conflicts, and really outranks). The same test
requires every outcome code to come up at least 20 times. Another shuffles the input and requires
the same plan.
**Found by the tests:** a request with no limit stores its need as `Seconds::MAX`, and the running
total of granted contact saturates at the same value, so an unlimited need could read as "met".
`SatelliteRequest::is_met_by` now treats an unlimited need as never met.

### D-017 — The optimal scheduler: an integer program with clique constraints
**Decision:** `Scheduler::Optimal` in `maat-ground` models the same rules as an integer linear
program through `good_lp`, solved by `microlp`.
**Objective:** maximize, over satellites, `(priority + 1) × useful contact seconds`, where useful
contact is contact up to the satellite's need. **This makes priority a weight, where the greedy
scheduler makes it a strict rank.** The two agree when priorities are equal. A strict-rank
objective was considered and set aside: it needs either weights that differ by many orders of
magnitude, which breaks floating-point solvers, or one solve per priority level. The difference is
stated in the README, the module docs and the guidance for `MAAT-109`.
**Constraints as cliques:** for each instant a pass begins, at most one of the passes in progress
at that station (padded by its turnaround time), or for that satellite, may be chosen. A test
confirms that two passes share a clique exactly when `maat_flight::schedule::conflict` says they
conflict, so the model and the greedy scheduler enforce the same rules by construction.
**Solver choice:** `microlp` is pure Rust. The workspace still builds with only a Rust toolchain,
on Windows as on Linux, and the solver can go to WebAssembly for the Stage 5 demo. HiGHS and CBC
are faster and have time limits but need a C++ toolchain or a system library.
**A finding that shaped the model:** the first formulation subtracted a tiny penalty per
scheduled pass, to stop the solver choosing passes that add nothing once a need is met. With that
penalty a 60-window problem took 15 seconds and a 200-window problem did not finish in a minute;
without it, 1000 windows take about 10 ms. The penalty makes the relaxed problem fractional at
every satellite whose need binds. It was removed, and surplus passes are instead removed after
the solve.
**After the solve:** the answer is checked for conflicts (an error if any), surplus passes are
removed worst first, and any usable pass left out is added best first, repeating until nothing
changes. Neither step lowers the value. They guarantee that every unscheduled candidate has a
concrete reason: its need is met (`MAAT-104`) or it conflicts with a scheduled pass (`MAAT-109`,
naming that pass).
**Verification:**
- 300 random problems of 4 to 16 windows: the plan's value equals the best found by trying every
  subset of windows, to 1e-6. Greedy is strictly beaten in at least 20 of them.
- 60 random problems of 40 to 200 windows: every plan is conflict-free with true reasons, and is
  worth at least as much as the greedy plan.
- A week of five synthetic satellites over four stations (597 windows): worth 2.3 % more than
  the greedy plan.
**Measured run time (release build, one desktop core):** 100 windows in under 5 ms, 1000 in 8 to
16 ms, 3000 in 0.1 to 0.3 s.
**Limits:** no bound on run time and no time limit in the solver; optimality is proven only up to
16 windows; ground only.

### D-018 — The contact plan: one entry per pass, explained at three levels
**Decision:** `plan_contacts` returns a `ContactPlan` with one `PlanEntry` for every pass the
survey found: those the scheduler decided and those the survey had already turned down. Each
entry has the reason code, a detail sentence, which stage decided it, and for a pass that lost to
another, the position of that pass. The plan renders as text and as JSON.
**Three levels of explanation:**
1. the code and one-line summary, in the flight crate, stable and short enough for a downlink;
2. the detail, built on the ground from the actual plan: names, times, priorities, seconds;
3. the guidance (`maat_ground::guidance`, `maat explain`): what the code means and what would
   change the outcome.
**O-4, serialization:** JSON is built in `maat-ground` with `serde_json`. The flight types do not
implement `serde` traits, so the flight crate keeps its empty dependency list. `serde_json` was
already in the tree through Ephemerust.
**JSON contract:** the layout is named `maat.contact-plan/1`. The number changes only if a field
is removed or changes meaning. Reasons appear as an object with `code`, `number`, `class` and
`summary`; `blocked_by` is an index into `entries`; a need of `null` means no limit.
**Value on every plan:** the optimal scheduler's objective is computed for greedy plans too, so
the two can be compared on one number.
**Incomplete plans say so:** if the survey could not search a satellite/station pair, the plan
lists it, the text report says INCOMPLETE on its fourth line, and the JSON has
`"complete": false`.
**Verification:** exact-text tests of each detail sentence; a golden test of the whole text
report; JSON parsed back and compared field by field; an end-to-end test that runs the flight
scheduler directly on a fixed-size array and requires the ground plan to match it decision for
decision.

### D-019 — Six reason codes added in Stage 2
**Decision:** `MAAT-106` (no request), `MAAT-107` (lost a tie), `MAAT-108` (turnaround),
`MAAT-109` (outweighed in the optimal plan), `MAAT-204` (duplicate request) and `MAAT-205`
(duplicate station rule).
**Why each is separate:** each has a different remedy. A tie is fixed by changing a priority; a
turnaround conflict by checking the station's turnaround time; a missing request by adding one.
`MAAT-109` is separate from `MAAT-101` because the optimal scheduler does not compare priorities
pass against pass, so "a higher-priority pass" would be false.
**Guarded by tests:** the frozen list in the flight tests now holds all sixteen codes, and a test
in `maat-ground` requires every code to have its own guidance text.

---

## Open decisions

| # | Question | Needed by |
|---|---|---|
| O-1 | **Final name.** The repository is `IsomorphicAlgo/Ma-at`. `maat` is taken on crates.io; `maat-flight`, `maat-ground` and `maat-cli` were free on 2026-11-14. | First publish |
| O-2 | ~~OMM input.~~ **Resolved:** Ephemerust 1.2 (D-010). | — |
| O-3 | ~~Fixed-size storage for the scheduler.~~ **Resolved:** caller-provided slices (D-016). | — |
| O-4 | ~~Serialization of flight types.~~ **Resolved:** JSON is built on the ground; flight types stay free of `serde` (D-018). | — |
| O-5 | ~~Priority ties.~~ **Resolved:** longer, then earlier, with its own code `MAAT-107` (D-016). | — |
| O-6 | ~~Turnaround between passes.~~ **Resolved for now:** a fixed time per station (D-016). Pointing-dependent turnaround is still open. | Later |
| O-7 | ~~Minimum pass length and data need.~~ **Resolved:** `SatelliteRequest` in the flight crate (D-016). | — |
| O-8 | **Element-set age.** Whether the survey should warn, or refuse, when the search is far from an element set's epoch. Not done in Stage 3. | Stage 4 |
| O-10 | **What priority means in the optimal scheduler.** It is a weight (`priority + 1` per second); greedy treats it as a strict rank (D-017). The owner may prefer a different weighting, or a strict-rank objective. | Owner's call |
| O-11 | **A time limit for the optimal scheduler.** `microlp` has none. A limit would need a different solver or a search of Ma'at's own. | Before heavy use |
| O-12 | **Verifying an uplinked plan onboard.** A flight function that checks a plan made on the ground against the same rules before it is used. | Later |
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

### Stage 1 — Windows: **approved by the owner, 2026-11-14**

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
| Owner sign-off | "Signed off stage 1" | given |

### Stage 2 — Scheduler

| Criterion | Evidence | State |
|---|---|---|
| Greedy priority scheduler, flight-safe | `maat_flight::schedule_greedy`: `no_std`, no allocation, no dependencies, panic lints clean, bounded run time (D-016) | met |
| Optimal scheduler, ILP via `good_lp`, ground only | `Scheduler::Optimal` (D-017) | met |
| Station conflicts | `MAAT-101`, `107`, `108`; one `conflict` function for both schedulers | met |
| Minimum pass length | `SatelliteRequest::with_min_contact`, `MAAT-103` | met |
| Per-satellite data needs | `SatelliteRequest::with_need`, `MAAT-104`; surplus passes are not scheduled | met |
| Greedy plans are sound | 400 random problems checked by an independent verifier; every outcome code exercised | met |
| Optimal plans are optimal | Equal to exhaustive search on 300 problems of up to 16 windows | met, for small problems |
| Optimal is never worse than greedy | 60 random problems of up to 200 windows, and a week of real orbits | met |
| Ground greedy is the flight scheduler | End-to-end test compares the plan with `schedule_greedy` on a fixed array, decision for decision | met |
| Bare-metal build still holds | Flight dependency tree is one line; host `no_std` build passes | met on host; **bare-metal build awaits CI** |
| Owner sign-off | | **pending** |

### Stage 3 — Explainable output

| Criterion | Evidence | State |
|---|---|---|
| Every scheduled or rejected pass says why | One `PlanEntry` per pass found, each with a `MAAT-xxx` code and a detail sentence; tested on every entry of both schedulers' plans | met |
| Stable codes | Sixteen codes in the frozen list; numbers and identifiers guarded by test | met |
| The plan's example sentence | "dropped: overlaps a higher-priority pass at the same station" is `MAAT-101`, word for word | met |
| Reasons are true, not just present | Blockers verified to be scheduled and to conflict; details checked against exact expected text | met |
| Output for people and for programs | `ContactPlan::to_text`, `ContactPlan::to_json` (`maat.contact-plan/1`) | met |
| Guidance for every code | `guidance`, `maat explain`; a test requires each code to have its own text | met |
| Tests green | 326 passing: 247 unit, 53 integration, 26 doctests, on Rust 1.97 (Linux) | met here; **not yet run on Windows or 1.89** |
| Tests catch mistakes | 20 deliberate faults in the schedulers and the plan each failed at least one test (one needed a new test first) | met |
| Lints and docs clean | `cargo fmt --check`, `cargo clippy -D warnings`, `cargo doc -D warnings` | met |
| Owner sign-off | | **pending** |

---

## Attribution

- **Ephemerust** (the owner's astrodynamics crate) provides OMM and TLE parsing and pass
  prediction; it builds on the **`sgp4`** crate. MIT.
- **`chrono`** (MIT/Apache-2.0) and **`thiserror`** (MIT/Apache-2.0), ground layer only.
- **`good_lp`** (MIT) for the integer-program model and **`microlp`** (Apache-2.0), with its
  dependencies `sprs` and `ndarray`, for solving it. Ground layer only.
- **`serde_json`** (MIT/Apache-2.0) for JSON output. Ground layer only.
- Interval scheduling with clique constraints is textbook integer programming; the formulation
  here was written for this project.
- The reference ISS two-line element set in tests and examples is public data, also used in
  Ephemerust's documentation. All other element sets in tests and examples are synthetic.
- The azimuth-rate formula in D-012 is standard tracking geometry (line-of-sight rate divided by
  the cosine of elevation); it is used only as a test cross-check.
- Stages 0 to 3 were drafted with AI assistance (Claude) and are subject to owner review.

## Scope and export control

Like its sibling projects, Ma'at is limited to open, public standards and public data: published
element sets, generic ground-station geometry and textbook scheduling. Examples use public
reference objects such as the ISS, synthetic element sets, and antenna figures typical of amateur
equipment. It contains no controlled technical data and no parameters of any real mission's
operations.
