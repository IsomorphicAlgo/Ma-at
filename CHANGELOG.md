# Changelog

All notable changes to this workspace are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/). Before 1.0, a minor version bump may
contain breaking changes.

## [Unreleased]

### Added — Stage 1: contact windows

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

### Added — Stage 0: core types and the `no_std` boundary

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
