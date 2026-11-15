# Changelog

All notable changes to this workspace are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/). Before 1.0, a minor version bump may
contain breaking changes. Until 1.0 there is one minor version per completed stage of the plan.

## [Unreleased]

## [0.4.0] - 2026-11-15

Stages 2 and 3: the schedulers and explained contact plans. Not published to crates.io. Version
0.3.0 was not issued, because the two stages were built together.

### Added — Stage 2: scheduling

- **`maat-flight`: the greedy scheduler.** `schedule_greedy(windows, requests, stations,
  decisions)` considers windows best first (priority, then length, then AOS) and writes one
  `Decision` per window into a slice the caller provides. No allocation, no dependencies, and a
  run time proportional to the square of the number of windows.
  - `SatelliteRequest`: priority, minimum contact and contact need for one satellite.
  - `StationRule`: a station's turnaround time between passes.
  - `Decision`, `ScheduleSummary`, `ScheduleError`.
  - `schedule::conflict`, `rank`, `screen`, `validate` and `turnaround_for`: the rules, exposed
    so the optimal scheduler and the tests use the same ones.
- **`maat-ground`: the optimal scheduler.** `Scheduler::Optimal` solves the same problem as an
  integer linear program (through `good_lp`, with the pure-Rust `microlp` solver) and returns
  the plan of greatest value: `priority + 1` per useful second of contact.
- **`GroundStation::with_turnaround`**, `turnaround` and `rule`.
- **Six reason codes**: `MAAT-106` (no request), `MAAT-107` (lost a tie), `MAAT-108`
  (turnaround), `MAAT-109` (outweighed in the optimal plan), `MAAT-204` (duplicate request) and
  `MAAT-205` (duplicate station rule). `MAAT-101`, `102`, `104` and `301`, reserved since Stage
  0, are now emitted.
- `GroundError::Schedule` and `GroundError::Solver`.

### Added — Stage 3: explained output

- **`maat-ground`: the contact plan.** `plan_contacts(survey, satellites, stations, requests,
  scheduler)` returns a `ContactPlan` with one `PlanEntry` for every pass that was found, each
  with its reason code, a detail sentence naming the satellites, stations, times and numbers
  involved, and for a pass that lost to another, which one. Summaries per satellite
  (`SatelliteSummary`, including whether its need was met) and per station (`StationSummary`).
- **`ContactPlan::to_text`**: the schedule, every unscheduled pass with its reason, and the
  summaries, for a person.
- **`ContactPlan::to_json`** and `to_json_value`: the same plan for a program, in a layout named
  `maat.contact-plan/1` (`JSON_SCHEMA`).
- **`ContactPlan::value`**: one number on which a greedy and an optimal plan can be compared.
- **`guidance(reason)`**: a paragraph for each reason code on what it means and what would
  change the outcome. `parse_reason_code` reads a code as a person would type it.
- **`maat-cli`: `maat explain <CODE>`.**
- Example `contact_plan`: a printed plan, and greedy against optimal over a week.

### Changed

- New dependencies in `maat-ground` only: `good_lp` (with `microlp`) and `serde_json`. The
  flight crate still has none.
- The wording of the `MAAT-1xx` class: "a valid pass lost to a scheduling rule or a station
  limit".

### Tests

- 326 in total (247 unit, 53 integration, 26 doctests).
- Greedy plans: 400 random problems checked by a verifier written independently of the
  scheduler.
- Optimal plans: equal to exhaustive search on 300 random problems of up to 16 windows, and
  never worth less than the greedy plan on larger ones.
- End to end: the ground's greedy plan matches the flight scheduler run on a fixed-size array,
  decision for decision.

## [0.2.0] - 2026-11-14

Stage 1: contact windows. Not published to crates.io.

### Added

- **`maat-ground`: satellites.** `Satellite` (identifier, name, epoch and an initialized SGP4
  propagator) built with `from_omm`, `from_omm_with_id` or `from_tle`, and `satellites_from_omm`,
  which reads a whole catalog from OMM text in JSON, KVN, XML or CSV. Catalog numbers of up to
  nine digits are supported.
- **`maat-ground`: slew limits.** `SlewLimits` and `GroundStation::with_slew_limits`.
- **`maat-ground`: the window survey.** `contact_windows(satellites, stations, start, end)`
  returns a `WindowSurvey`: usable flight `ContactWindow`s sorted by AOS, `RejectedPass`es with a
  reason code and the numbers behind it, and `SurveyFailure`s for pairs that could not be
  searched. One satellite failing does not stop the others.
- **Orbit sanity check.** An element set whose orbit passes through the Earth is refused with
  `GroundError::ImpossibleOrbit`. SGP4 itself accepts such elements.
- **`maat-flight`: `MAAT-105`** (`ReasonCode::SlewRateExceeded`): tracking the pass needs a
  faster antenna slew than the station allows.
- **`GroundError`**: new variants `Catalog`, `Elements`, `ImpossibleOrbit`,
  `MissingCatalogNumber`, `DuplicateSatellite`, `DuplicateStation`, `InvalidSearchWindow` and
  `PassPrediction`, and `GroundError::hint()`, which passes on Ephemerust's corrective hints.
- Example `window_survey`.
- **Tests**: 176 in total (125 unit, 33 integration, 18 doctests), including a comparison of the
  survey with a one-second brute-force elevation scan.

### Changed

- Requires **Ephemerust 1.2.1** (was 1.1): OMM input, one propagator shared across stations, and
  a pass search that cannot step over a short pass.
- `GroundError` keeps the Ephemerust error as its `source` and no longer implements `Clone` or
  `PartialEq`.
- The repository is `https://github.com/IsomorphicAlgo/Ma-at`.

## [0.1.0] - 2026-11-14

Stage 0: core types and the `no_std` boundary. Not published to crates.io.

### Added

- **Workspace** with three crates: `maat-flight` (vehicle layer), `maat-ground` (human layer) and
  `maat-cli` (the `maat` command).
- **`maat-flight`**: `#![no_std]`, no heap, no dependencies, no `unsafe`, integer-only.
  - `SatelliteId`, `StationId` and `Priority` newtypes.
  - `Timestamp` (whole seconds since the Unix epoch) and `Seconds`, with checked arithmetic.
  - `Elevation` (hundredths of a degree) and `ContactWindow` (a validated, half-open
    `[AOS, LOS)` interval) with `contains` and `overlaps`.
  - `ReasonCode` and `ReasonClass`: the stable `MAAT-xxx` catalogue, with nine initial codes.
  - `WindowError`, mapped to `MAAT-2xx` codes.
  - Lints deny `unwrap`, `expect`, `panic!`, slice indexing, unchecked arithmetic and floating
    point in library code.
- **`maat-ground`**:
  - `GroundStation` and `StationSite`, with range-checked construction.
  - `timestamp_floor`, `timestamp_ceil` and `utc_from_timestamp` conversions.
  - `window_from_pass`: an Ephemerust `Pass` to a flight `ContactWindow`, rounding inward.
  - `GroundError`, with `reason()` returning the `MAAT-xxx` code where one applies.
  - Example `iss_windows`.
- **`maat-cli`**: `maat codes`, `maat --version`, `maat --help`.
- **CI**: tests on the 1.89 MSRV and stable, a Windows job, a bare-metal `no_std` build of
  `maat-flight`, rustfmt, clippy, rustdoc, `cargo audit` and `cargo deny`.
- **Tests**: 104 in total (76 unit, 14 integration, 14 doctests).
