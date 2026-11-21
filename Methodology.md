# Methodology — Ma'at

A living record of **why** the project is built the way it is: major decisions, trade-offs and
the reasoning behind them. Append new entries as decisions are made. Do not silently rewrite
history; mark an entry superseded or amended instead.

> **Status (2026-11-15): version 0.6.0. Stages 0 to 4 approved by the owner. Stage 5 built and
> awaiting sign-off.** Ma'at schedules contact windows with a greedy scheduler in the flight
> crate or an optimal one on the ground, and every pass in the resulting plan carries a reason
> code and a detail sentence. `maat plan` builds a plan from files. `demo/` runs the same code in
> a browser.

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
is a scheduling constraint and is not part of Stage 1; it was added in Stage 2 (D-020).

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
**Publishing:** When this was written nothing was on crates.io. A version here records a state
of the repository; a git tag (`v0.2.0`) marks it. Publishing is a separate decision, and `maat`
itself is taken there. **Updated 2026-11-15:** 0.2.0 of all three crates has since been
published (D-028); 0.4.0 and 0.5.0 have not.
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
- **O-6, turnaround:** a fixed time per station (`StationRule`), applied before and after
  every scheduled pass. A pass inside the gap gets `MAAT-108`. The time to turn the antenna
  between two passes was added on top of this before sign-off (D-020).
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
**Limits:** run time is bounded only by the time limit (D-021); the solver's answers are checked
against exhaustive search only up to 16 windows; ground only.
**Corrected 2026-11-15:** this entry first said that `microlp` has no time limit. That was wrong.
`microlp` 0.6 accepts a time limit and a starting solution, and `good_lp` exposes both
(`WithTimeLimit`, `Variable::initial`). The error was in the README, this file and the module
docs of 0.4.0 as first delivered, and is corrected in all three. See D-021.

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
**Guarded by tests:** the frozen list in the flight tests now holds all sixteen codes (seventeen
since `MAAT-206`, D-020), and a test
in `maat-ground` requires every code to have its own guidance text.

---

### D-020 — Time between passes includes the turn of the antenna
**Decision (owner, 2026-11-15: "Scheduler should account for turning"):** the time a station
needs between two passes is its fixed turnaround plus the time to turn its antenna from where
the first pass ends to where the second begins.
**In the flight crate:**
- `Azimuth` (hundredths of a degree, `0..36000`, clockwise from north) and `Pointing` (azimuth
  and elevation). A `ContactWindow` may record its pointing at AOS and at LOS. Pointing is
  optional so that a window built from times alone is still valid.
- `SlewRates` (hundredths of a degree per second on each axis, never zero) on a `StationRule`.
- `StationRule::gap_needed(earlier, later)` = fixed turnaround + `slew_time`, where the slew time
  is that of the slower axis, in whole seconds **rounded up**, with azimuth taken the short way
  round. It is zero unless the rule has rates and both windows record pointing.
- `schedule::conflict(a, b, rule)` now takes the rule and applies `gap_needed`. Both schedulers
  and every verifier call it, so the rule still has one definition.
- One new code, `MAAT-206` (azimuth out of range). A pass lost to turning time keeps `MAAT-108`:
  the cause and the remedy are the same kind as for the fixed turnaround, and the detail
  sentence gives both parts ("120 s fixed plus 38 s to turn its antenna").
**On the ground:** `contact_windows` records each window's pointing from Ephemerust's look angles
at the window's own whole-second AOS and LOS (azimuth to the nearest hundredth, elevation rounded
down). `GroundStation::rule` passes the station's slew limits on, rounded **down**, so the
scheduler never assumes a faster mount than the station has.
**In the optimal scheduler:** the clique constraints still cover overlap and the fixed
turnaround. Turning time differs for every pair, so each pair that is too close only because of
it gets a two-window constraint of its own. A test confirms that the cliques and pairs together
rule out exactly the pairs `conflict` rejects.
**What the model assumes:** both axes move at once at constant top rates; azimuth can go either
way round. Acceleration, cable-wrap limits and keyhole behaviour near the zenith are not
modelled; the fixed turnaround is the place to allow for them.
**Effect, measured:** on the week of five satellites over four stations, with Seattle turning at
2 degrees per second, the greedy plan goes from 166 passes to 165 and the optimal from 173 to
171. The example prints one pass that clears the fixed 120 s by 2 s and is still refused,
because the turn takes 38 s.
**Cost, measured (release build):** the greedy scheduler is unchanged at 17 to 27 ms for 3000
windows. The optimal scheduler takes 16 to 52 ms for 1000 windows and 0.1 to 0.5 s for 3000 in
eight of nine problems; one problem with two slewing stations took 2.8 s. Pairwise constraints
are weaker than cliques, so more turning conflicts mean more branching. The figures in D-016 and
D-017 were measured before this change.
**Verification:** 17 flight unit tests on slew time, gap and conflict, including both rounding
boundaries and the overflow cases; the random-problem verifiers in both crates now generate
pointing and slew rates; an end-to-end test recomputes the needed gap at Seattle from the
recorded pointing, without calling `conflict`, for every consecutive pair in both plans; pointing
is checked against Ephemerust's look angles for every surveyed window.

### D-021 — The optimal scheduler starts from the greedy plan and stops at a time limit
**Decision:** `plan_contacts_with` takes `PlanOptions`, which carry a time limit for the optimal
scheduler (default 60 s). The solver is given the greedy plan as its starting point. The plan
records whether it is proven optimal (`ContactPlan::optimality`), and says so in text and JSON.
**The owner's question (2026-11-15):** whether the limit could be worked out from the orbit, for
example from how many stations a satellite meets in one orbit, with 24 hours as a fallback.
**Why the limit is not derived from the orbit:** how long a problem takes to solve depends on how
many passes compete, not on the orbital period, and in practice it is short: under a second for
3000 windows. What the orbit does set is the **deadline**. A plan is of no use for a pass that
has begun, so the time until the first pass is the longest a search can usefully run.
`time_until_first_pass(survey, now)` returns it, and the caller decides how much of it to spend.
The default is one minute and not hours because an hour-long search would be a fault to
investigate, not a normal run.
**Three guarantees, whatever the limit:**
1. The plan obeys every rule. It is checked for conflicts after the solve.
2. The plan is worth at least as much as the greedy plan. If the limit passes before the solver
   has a plan of its own, or its plan is worth less, the greedy plan is used.
3. A plan is called proven only when the solver finished.
**What this does not guarantee:** that each satellite gets at least what the greedy plan would
have given it. The guarantee is on the plan's total value. A per-satellite floor is possible
(one more constraint per satellite) and has not been built; see O-13.
**A plan cut short is not reproducible across machines.** A faster machine gets further in the
same time. A proven plan does not have this problem. The text report says "NOT PROVEN OPTIMAL"
at the top when it applies.
**Verification:** with a limit of zero, 40 random problems still give sound plans worth at least
the greedy plan. With limits either side of the solve time, plans are sound, never above the
optimum, and proven only when equal to it. The case in between, where the solver is stopped
holding an unproven plan of its own, depends on timing; a test times the full solve and then
stops the solver at 16 points across it (it arose 12 to 17 times in 96 tries on the development
machine). The mapping from solver status to "proven" is tested directly, so that guarantee 3
does not rest on timing.
**Measured:** the greedy starting point does not change the answer or, noticeably, the time on
problems that finish. Its value is on problems that do not: on two 400-window problems stopped
at 40 and 80 ms (debug build), a solver without a starting point had nothing to return where the
started one returned a plan.
**Two deliberate faults that no test catches, and why:** removing the starting point (the
answer is the same whenever the solver finishes, and the fallback covers it when it does not),
and making the fallback an empty plan (the "worth at least greedy" check after it puts the
greedy plan back). Both are second lines of defence behind another check.

### D-022 — Owner's answers to the Stage 2 questions (2026-11-15)
- **Priority is a rank in greedy and a weight in optimal:** accepted as it is (closes O-10).
- **Storage in caller-provided slices, no dependencies:** confirmed (D-016).
- **Ties: longer pass, then earlier AOS:** confirmed (D-016).
- **Turning time between passes:** build it now (D-020).
- **A time limit for the optimal scheduler:** wanted (D-021).
- **Station files for the CLI:** TOML (O-9; to be built in Stage 4).

### D-023 — A floor for each satellite in the optimal plan
**Decision (owner, 2026-11-15: "Yes on the per satellite floor"):** an option,
`PlanOptions::with_satellite_floor(true)` (`--floor`), under which the optimal scheduler must
give every satellite at least the useful contact the greedy plan gives it. Off by default.
**Why it was wanted:** D-021 guarantees the optimal plan's total. It says nothing per satellite.
In the example week the optimal plan is worth 2.2 % more and EXAMPLE-B goes from 35 317 s to
18 216 s, because EXAMPLE-C has the same priority and shares its orbit: the two are worth the
same to the objective, and the solver has no reason to keep greedy's tie-break.
**How:** one constraint per satellite, `useful contact >= greedy's useful contact`, where useful
contact is contact counted up to the satellite's need. For a satellite whose need can be reached
the constraint is on the same capped variable the objective uses; otherwise on its total contact.
The greedy plan satisfies every such constraint, so it remains a valid starting point and
fallback. After the solve the floor is checked again on the final plan; if it ever failed, the
greedy plan would be used.
**What "proven optimal" means with a floor:** no plan that respects the floor is worth more.
**What it costs, measured:**
- Example week: **all of the gain.** With the floor the optimal plan equals the greedy plan in
  value (285 216) and gives every satellite exactly its greedy share. Three of the four
  requested satellites share an orbit and compete for the same passes, so any gain for one is a
  loss for another.
- 60 crowded random problems, of which the optimal plan beat greedy in 51: the floor kept 84 %
  of the gain on average, all of it in 31 and none of it in 4.
- Run time (release build): on five problems of 1000 to 3000 windows the floor made no
  difference in three (33 to 363 ms either way), took 48 ms against 28 ms in one, and took 8 to
  10 s against about 0.5 s in one (two runs).
**In the plan:** `Optimality::satellite_floor`; `SatelliteSummary::floor`; a "Floor:" line and a
FLOOR column in the text report, only for a plan built with one; `floor_s` and
`optimality.satellite_floor` in the JSON. The detail of a pass that lost says that a swap
"would not raise the plan's total value, or would leave a satellite with less contact than the
greedy plan gives it".
**Verification:** three hand-built cases (the floor binds; a gain that costs nobody survives;
the floor counts contact only up to a need); 60 random problems in which every satellite is at
or above its greedy share and the value lies between greedy's and the unconstrained optimum's;
200 small problems in which the plan's value equals the best found by trying every subset that
respects the floor; the example week end to end.
**Closes O-13.**

### D-024 — Stage 4: the `maat plan` command and its files
**Decision:** one new command, `maat plan`, which reads a satellite catalog and a TOML station
file and writes a plan. No separate survey command: the plan already lists every pass the
survey found, usable or not.
**Station and request files (owner's choice of TOML, D-022):** `[[station]]` and `[[request]]`
tables, in one file or two. The site keys are those of Chronus Gateway's `[station]` section
(`latitude_deg`, `longitude_deg`, `altitude_m`, `minimum_elevation_deg`), so a station is
described the same way in both projects. Unknown keys are errors (`deny_unknown_fields`): a
misspelt option is reported with its line and never silently ignored.
**Where the file reading lives:** in `maat-cli`. `maat-ground` stays a library of values and
functions; it gains no file format and no new dependency.
**New dependencies, CLI only:** `toml` 1.x with only its `parse` and `serde` features, and
`serde` with `derive`. Both MIT OR Apache-2.0; `toml` needs Rust 1.85, below the workspace's
1.89. `chrono`'s `now` feature is enabled in the CLI for a plan that starts at the present.
**Still no `clap`:** the parser is about a dozen options and is tested option by option.
**Standard output is the plan and nothing else**, so it can be piped into a file or another
program. Everything else a person should know goes to standard error: the search window used,
an incomplete survey, a plan not proven optimal, old element sets, skipped passes in an export,
and requests for satellites that are not in the catalog.
**Exit status:** 0 for a plan (including an incomplete or unproven one, which says so), 1 when
the inputs could not be turned into a plan, 2 for a command line that makes no sense.
**Defaults:** search from now for 24 hours, greedy scheduler, text output. A plan from the same
files and the same `--start` is identical from run to run.
**Verification:** 40 unit tests of the option parser, the TOML reader and the age warning; 22
tests that run the real binary on real files and check standard output, standard error and exit
status, including every error path above. The example files in `examples/` are the ones the
tests use, and a test fails if the two drift apart.

### D-025 — Loading catalogs of two-line element sets
**Decision:** `satellites_from_tle` reads a file of two-line element sets, named or not, and
`satellites_from_catalog` detects whether text is TLE or OMM. **Closes the open part of O-9.**
**How sets are split:** blank lines are dropped; a set ends at a line 2 that follows a line 1;
each set is handed to Ephemerust's `Tle::parse`, which checks its checksums. Lines left over
after the last complete set are an error, as is a file with no set in it.
**Detection:** text is TLE if a line 1 is directly followed by a line 2 (each at least 60
characters, beginning `1 ` and `2 `). Anything else goes to the OMM reader, which detects JSON,
KVN, XML and CSV itself.
**Limit:** TLE cannot carry a catalog number above 99999. OMM can.
**A deliberate fault no test catches:** dropping the 60-character test. A short line that begins
`2 ` then ends a set early, the set fails Ephemerust's own checks, and the result is the same
error by another route.

### D-026 — What the two exports are, and what they have not been tested against
**SatNOGS.** The SatNOGS Network's API takes, for a new observation, an object with four
required fields: `ground_station` (the network's station number), `transmitter_uuid` (22
characters, from SatNOGS DB), `start` and `end` (date-times). Source: the network's OpenAPI
document, which its API page (<https://satnogs-network.readthedocs.io/en/latest/api.html>) names
as the description of the API; the new-observation serializer was read on 2026-11-15 in a
rendering of that document. `satnogs_schedule` writes a JSON array of exactly those
four fields per scheduled pass, with times in ISO 8601 UTC.
- Ma'at cannot know SatNOGS's numbers, so they are supplied: `satnogs_id` on a station and
  `satnogs_transmitter` on a request. A scheduled pass lacking either is left out of the array
  and listed with the reason; the two lists together always account for every scheduled pass.
- **Not done:** the output has never been posted to the network. No claim is made that the
  network would accept a given observation: it has rules of its own (who may schedule on a
  station, which transmitters a station can receive, how far ahead, and overlap with existing
  observations).
**Chronus Gateway.** The plan called for a "Chronus export". The gateway was examined at
1.0.0-rc.1 (commit `0383109` in the owner's working copy):
- its configuration is one station and one orbit source, and `[station]` requires a carrier
  frequency and element text that Ma'at does not hold, so Ma'at cannot write a gateway
  configuration;
- it has **no schedule or pass-list input**. A pass route (`chronus.passes.v1`) is planned for a
  later release and is not defined yet.
So the export is Ma'at's own document, `maat.station-schedule/1`: for one station, the site
(in the gateway's key names) and its scheduled passes with satellite, AOS, LOS, duration, peak
elevation, AOS and LOS azimuth, and priority. It is named in Ma'at's namespace and not the
gateway's, because the gateway has not adopted it. **Open for the owner (O-15):** whether the
gateway should read this document as it is, or define its own and have Ma'at follow.
**Verification:** unit tests of both exports on a hand-built plan (order, fields, the passes
left out and why, transmitter validation), and the real binary on the example files with the
exact JSON expected.

### D-027 — A warning for old element sets (closes O-8)
**Decision:** `maat plan` warns, and still plans, when any satellite's element set is more than
14 days from the search window, measured to whichever end of the search is further from the
epoch. One line, naming how many satellites and the furthest.
**Why warn and not refuse:** an old element set gives a worse plan, not a meaningless one, and
the person may have nothing newer.
**Why 14 days:** a rule of thumb. Along-track error in a low-orbit two-line element set grows by
roughly a kilometre or more a day, which moves AOS and LOS by a growing fraction of a second and
then by seconds. **This figure is not measured by Ma'at**, and it is the same for every orbit,
which is too lenient for a low, draggy orbit and too strict for a high one.
**Not done:** nothing in the plan itself changes with element age. Windows are not widened and
no confidence is attached to a pass.

### D-028 — The name is Ma'at
**Decision (owner, 2026-11-15: "lets set it as the official name"):** the project is named
Ma'at. It is no longer a working name. **Closes O-1.**
**What carries the name:** the repository (`IsomorphicAlgo/Ma-at`), the three crates
(`maat-flight`, `maat-ground`, `maat-cli`), the `maat` command, the `MAAT-xxx` reason codes and
the `maat.contact-plan/1` and `maat.station-schedule/1` document layouts. Nothing had to be
renamed.
**On crates.io (seen 2026-11-15):** `maat-flight`, `maat-ground` and `maat-cli` are published at
0.2.0 with this repository as their source, so the three names are held. Earlier entries in
this file and in the changelog said nothing had been published; they are corrected.
**One thing to know:** the bare name `maat` on crates.io belongs to an unrelated project, a
programming language. Ma'at therefore has no umbrella crate called `maat`, and
`cargo install maat-cli` installs a command called `maat` that could collide with a command of
the same name from that project if both are installed.

### D-029 — The station-file reader moves into `maat-ground`, behind a feature
**Decision:** `maat_ground::scenario` (the TOML reader written for the CLI in D-024) now lives in
`maat-ground` behind an optional `scenario` feature, off by default.
**Why this reverses part of D-024:** D-024 kept file reading in the CLI so that the library
gained no dependency. Stage 5 adds a second front end that must read the same file, and two
copies of a parser drift. A feature keeps both aims: with default features `maat-ground` still
has no `serde` derive and no `toml`; a front end that wants the reader asks for it.
**Verified:** `maat-ground` builds and passes its tests with and without the feature (190 and
200 unit tests), and its dependency tree without the feature has neither crate.

### D-030 — Stage 5: the browser demo
**Decision:** a new crate, `maat-wasm`, and a static page, `demo/`.
**The bridge adds no scheduling logic.** `maat-wasm` exposes four functions to JavaScript
(`plan`, `catalog`, `scenario`, `version`). Each takes text and returns JSON text. `plan` calls
`satellites_from_catalog`, `Scenario::add_toml`, `contact_windows` and `plan_contacts_with`,
the same four calls the `maat` command makes, and returns the `maat.contact-plan/1` document.
Strings on both sides keep the boundary small enough to read in one sitting.
**The same file as the command line.** The page turns the chosen satellites, priorities and
stations into the TOML station file and hands that to the bridge. It also shows the file, so a
plan made in the browser can be repeated with `maat plan`.
**Testable without a browser.** The bridge's functions are ordinary Rust; the exported ones are
one-line wrappers. Eleven native tests cover them, including every error path.
**"Watch the schedule build":** the greedy scheduler takes passes best first. The bridge returns
that order (`build_order`) by sorting the scheduler-decided passes with
`maat_flight::schedule::rank`, the flight crate's own ranking, and the page replays it. A test
replays a week in that order and checks that every scheduled pass is free of conflict with all
passes scheduled before it, and that every pass turned down for another finds that other
already scheduled. If the order were not the scheduler's, one of those would fail. The optimal
scheduler decides all passes together, so it has no order and the page says so.
**No new workspace-wide rule broken, one adjusted:** the workspace forbids `unsafe`.
`#[wasm_bindgen]` generates binding code and marks it `#[allow(unsafe_code)]`, which `forbid`
rejects, so `maat-wasm` alone uses `deny`. No `unsafe` is written by hand anywhere.
**The page:** plain HTML, CSS and one JavaScript module. No framework, no package manager, no
font or script fetched from elsewhere, nothing sent to a server. The example catalog and station
file are the ones in `examples/`; a test fails if the page's copy differs.
**Design:** the timeline is the first thing on the page. Each satellite takes one colour, named
for a stone of Egyptian jewellery (lapis, carnelian, malachite, turquoise, gold), so the
satellite picker is also the legend. A scheduled pass is a solid bar; a pass that lost is the
same colour in outline; a pass that never reached the scheduler's ranking is a grey dotted box.
The only motion is the replay, which the reader can stop, step through, or skip: with "reduce
motion" set, the finished plan is shown at once.
**Not published:** `maat-wasm` has `publish = false`. It is a front end.
**Verification:**
- *Builds for the browser.* `maat-wasm` compiles for `wasm32-unknown-unknown`, with the optimal
  solver included; its time limit works there because `microlp` reads the clock through
  `web-time`.
- *Same plan as the native build.* The WebAssembly module was run in Node on a week of the
  example fleet, greedy and optimal, and compared field by field with `maat plan --format json`.
  Every pass, time, reason, blocker and total matched. Four of 615 peak elevations differed, by at
  most 6e-14 degrees: the two builds use different sine and cosine routines. `demo/check.mjs`
  repeats this.
- *The page works.* Headless Chrome 1320 px and 390 px wide: loads, plans six hours greedy (4 of
  14 passes, as the command line gives), replays six decisions, selects a pass by mouse and by
  keyboard, plans a week with the optimal scheduler and a floor, shows a bad start time as an
  error with its fix, and does not scroll sideways at phone width. No console errors.
- *Not done:* other browsers; an automated browser test in the repository; a run on Windows.
**A note on how the browser build was made here:** the prebuilt `wasm32` standard library could
not be downloaded in the development environment, so the standard library was compiled from
source (`-Z build-std`). A normal installation uses `rustup target add wasm32-unknown-unknown`,
as the build scripts say. The same method gave the first check of the **bare-metal build**:
`maat-flight` compiles for `thumbv7em-none-eabihf` with `core` built from source. Both should
be confirmed by CI with the prebuilt targets.

## Open decisions

| # | Question | Needed by |
|---|---|---|
| O-1 | ~~Final name.~~ **Resolved:** Ma'at (D-028). | — |
| O-2 | ~~OMM input.~~ **Resolved:** Ephemerust 1.2 (D-010). | — |
| O-3 | ~~Fixed-size storage for the scheduler.~~ **Resolved:** caller-provided slices (D-016). | — |
| O-4 | ~~Serialization of flight types.~~ **Resolved:** JSON is built on the ground; flight types stay free of `serde` (D-018). | — |
| O-5 | ~~Priority ties.~~ **Resolved:** longer, then earlier, with its own code `MAAT-107` (D-016). | — |
| O-6 | ~~Turnaround between passes.~~ **Resolved:** a fixed time per station (D-016) plus the time to turn the antenna between the two passes (D-020). | — |
| O-7 | ~~Minimum pass length and data need.~~ **Resolved:** `SatelliteRequest` in the flight crate (D-016). | — |
| O-8 | ~~Element-set age.~~ **Resolved:** `maat plan` warns beyond 14 days (D-027). Open: a measured, orbit-dependent threshold. | — |
| O-10 | ~~What priority means in the optimal scheduler.~~ **Resolved:** a weight in optimal, a strict rank in greedy; accepted by the owner (D-022). | — |
| O-11 | ~~A time limit for the optimal scheduler.~~ **Resolved:** `PlanOptions::optimal_time_limit`, default 60 s (D-021). | — |
| O-12 | **Verifying an uplinked plan onboard.** A flight function that checks a plan made on the ground against the same rules before it is used. | Later |
| O-9 | ~~Station file format and multi-TLE loading.~~ **Resolved:** TOML (D-024); `satellites_from_tle` (D-025). | — |
| O-13 | ~~A floor per satellite in the optimal plan.~~ **Resolved:** built as an option (D-023). | — |
| O-15 | **The Chronus Gateway side of the station schedule.** The gateway has no schedule input (D-026). Should it read `maat.station-schedule/1` as it is, or define its own format for Ma'at to follow? | Owner, when the gateway's pass view is built |
| O-16 | **Send the SatNOGS export to the network once**, on a station the owner may schedule on, to learn which of the network's own rules a plan must respect (D-026). | Owner |
| O-14 | **A fuller mount model.** Acceleration, cable-wrap limits and zenith keyhole in the time to turn between passes (D-020 assumes constant rates and the short way round). | Later |

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

### Stage 2 — Scheduler: **approved by the owner, 2026-11-15**

| Criterion | Evidence | State |
|---|---|---|
| Greedy priority scheduler, flight-safe | `maat_flight::schedule_greedy`: `no_std`, no allocation, no dependencies, panic lints clean, bounded run time (D-016) | met |
| Optimal scheduler, ILP via `good_lp`, ground only | `Scheduler::Optimal` (D-017) | met |
| Station conflicts | `MAAT-101`, `107`, `108`; one `conflict` function for both schedulers | met |
| Time to turn the antenna between passes (added at the owner's request) | `Pointing` on windows, `SlewRates` on station rules, `StationRule::gap_needed` (D-020) | met, with the mount model's limits stated in D-020 |
| A time limit for the optimal scheduler (added at the owner's request) | `PlanOptions`, `plan_contacts_with`, `ContactPlan::optimality`, `time_until_first_pass` (D-021) | met |
| Minimum pass length | `SatelliteRequest::with_min_contact`, `MAAT-103` | met |
| Per-satellite data needs | `SatelliteRequest::with_need`, `MAAT-104`; surplus passes are not scheduled | met |
| Greedy plans are sound | 400 random problems checked by an independent verifier; every outcome code exercised | met |
| Optimal plans are optimal | Equal to exhaustive search on 300 problems of up to 16 windows | met, for small problems |
| Optimal is never worse than greedy | By construction since D-021 (starts from, and falls back to, the greedy plan); tested on 60 random problems of up to 200 windows, a week of real orbits, and with no solving time at all | met, for total value; **not per satellite** (O-13) |
| Ground greedy is the flight scheduler | End-to-end test compares the plan with `schedule_greedy` on a fixed array, decision for decision | met |
| Bare-metal build still holds | Flight dependency tree is one line; host `no_std` build passes | met on host; **bare-metal build awaits CI** |
| Owner sign-off | "I sign off on 2 and 3" | given |

### Stage 3 — Explainable output: **approved by the owner, 2026-11-15**

| Criterion | Evidence | State |
|---|---|---|
| Every scheduled or rejected pass says why | One `PlanEntry` per pass found, each with a `MAAT-xxx` code and a detail sentence; tested on every entry of both schedulers' plans | met |
| Stable codes | Seventeen codes in the frozen list; numbers and identifiers guarded by test | met |
| The plan's example sentence | "dropped: overlaps a higher-priority pass at the same station" is `MAAT-101`, word for word | met |
| Reasons are true, not just present | Blockers verified to be scheduled and to conflict; details checked against exact expected text | met |
| Output for people and for programs | `ContactPlan::to_text`, `ContactPlan::to_json` (`maat.contact-plan/1`) | met |
| Guidance for every code | `guidance`, `maat explain`; a test requires each code to have its own text | met |
| Tests green | 385 passing: 296 unit, 55 integration, 34 doctests, on Rust 1.97 (Linux). Run on Windows by the owner on 2026-11-15: the run reached and passed its last target (15 ground doctests), which `cargo test` does only when every earlier target passed, and the `contact_plan` example printed the same plan, line for line | met on Linux and Windows; **not yet run on 1.89** |
| Tests catch mistakes | 20 deliberate faults in the schedulers and the plan each failed at least one test (one needed a new test first). For turning time and the time limit, 23 more: 20 caught, 1 caught after a test was added, 2 not caught and explained in D-021 | met |
| Lints and docs clean | `cargo fmt --check`, `cargo clippy -D warnings`, `cargo doc -D warnings` | met |
| Owner sign-off | "I sign off on 2 and 3" | given |

---


### Stage 4 — Plug-ins: **approved by the owner, 2026-11-15**

| Deliverable | Evidence | State |
|---|---|---|
| Full CLI | `maat plan`: catalog (OMM or TLE) and TOML station/request files in; text, JSON, SatNOGS or station schedule out (D-024) | met |
| Station file format | TOML, owner's choice; unknown keys are errors with their line (D-024) | met |
| SatNOGS-compatible format | `satnogs_schedule`: the four fields of the network's new-observation request (D-026) | met as a format; **never sent to the network** (O-16) |
| Chronus export | `station_schedule`, `maat.station-schedule/1` (D-026) | met as Ma'at's side only; **the gateway has no schedule input to read it** (O-15) |
| Multi-TLE catalogs | `satellites_from_tle`, `satellites_from_catalog` (D-025) | met |
| Element-age warning (O-8) | `maat plan` warns beyond 14 days (D-027) | met; the threshold is a rule of thumb |
| Per-satellite floor (added at the owner's request) | `PlanOptions::with_satellite_floor`, `--floor` (D-023) | met; costs the whole gain on the example week |
| Errors explain themselves | Every refused file or option names the file, the line or the option, and the fix where there is one; each has a test | met |
| Tests green | 471 passing: 356 unit, 78 integration, 37 doctests, on Rust 1.97 (Linux). Run on Windows by the owner on 2026-11-15: the run reached its last target, the ground doctests, which `cargo test` does only when every earlier target passed, and those include the 22 tests that run the real `maat` binary on files; the result line of that last target was not in what was shared. The `contact_plan` example printed the same output as on Linux, line for line, including the per-satellite table | met on Linux; on Windows as described; **not yet run on 1.89** |
| Tests catch mistakes | 37 deliberate faults in the floor, the exports, the catalog loader and the CLI: 36 caught, 1 not caught and explained in D-025 | met |
| No new dependency in the libraries | `toml` and `serde` are in `maat-cli` only; `maat-flight` still has none | met |
| Owner sign-off | "Approve stage 4" | given |


### Stage 5 — Browser demo: **built, awaiting sign-off**

| Deliverable | Evidence | State |
|---|---|---|
| Pick satellites and stations | The page's pickers, with a priority for each satellite; a pasted catalog (OMM or TLE) replaces the example fleet | met |
| Watch the schedule build | Replay of the greedy scheduler's decisions in its own order (D-030); stop, step, or skip | met for greedy; the optimal scheduler has no order to replay |
| Same scheduling code as flight | `maat-wasm` calls `maat-ground`, which calls `maat-flight`'s `schedule_greedy`; the bridge adds no scheduling logic | met |
| Same plan as the native build | Week of the example fleet, WebAssembly in Node against `maat plan`: every decision identical, four numbers differ in their last digits | met here; `demo/check.mjs` repeats it |
| Every pass has a reason on the page | Table of every pass with code, summary and detail; selecting a pass highlights what beat it | met |
| The page works | Headless Chrome, desktop and phone widths, mouse and keyboard, no console errors | met in one browser; **not tried in Firefox or Safari, and not on Windows** |
| Bare-metal build of the flight crate | `thumbv7em-none-eabihf`, with `core` compiled from source | met here for the first time; **CI with the prebuilt target still to confirm** |
| Tests green | 483 passing: 356 unit, 90 integration, 37 doctests, on Rust 1.97 (Linux) | met here; **not yet run on Windows or 1.89** |
| Tests catch mistakes | 7 deliberate faults in the bridge, each caught. An eighth, removing the upper bound on the search length, sent the test into a ten-year search and was stopped, not waited for | met |
| Owner sign-off | | **pending** |

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
