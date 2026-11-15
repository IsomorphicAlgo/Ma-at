# maat-ground

The human layer of the [Ma'at](https://github.com/IsomorphicAlgo/Ma-at) contact scheduler.

This crate is where the conveniences live: calendar time, floating-point angles, `String`s, rich
errors and pass prediction through [Ephemerust](https://crates.io/crates/ephemerust). It turns
those into the plain integer types that `maat-flight` schedules, and re-exports the flight crate
as `maat_ground::flight`.

It provides:

- `Satellite` and `satellites_from_omm`: satellites from OMM text (JSON, KVN, XML or CSV, detected
  automatically) or from a TLE. Catalog numbers of up to nine digits are supported.
- `GroundStation`: a validated station with an identifier, a name, a site, an elevation mask and
  optional antenna `SlewLimits` and a fixed turnaround time.
- `contact_windows`: every contact opportunity for N satellites over M stations, as a sorted list
  of flight `ContactWindow`s, each recording where the antenna points at its start and end.
  Passes that are found but cannot be used come back separately, each
  with a `MAAT-xxx` reason code. A satellite that cannot be propagated is reported without
  stopping the rest.
- `plan_contacts`: a `ContactPlan` built with the greedy scheduler (the flight crate's, unchanged)
  or the optimal one (an integer program, ground only). Every pass that was found is in the plan
  once, with its reason code and a sentence of detail.
- `satellites_from_tle` and `satellites_from_catalog`: a file of two-line element sets, or
  either kind of catalog with the format detected.
- `satnogs_schedule` and `station_schedule`: a plan's scheduled passes as SatNOGS Network
  observations, or as one station's schedule. See the `export` module for what each is tested
  against, which is less than the names suggest.
- `PlanOptions::with_satellite_floor`: the optimal scheduler gives no satellite less than the
  greedy plan would.
- `plan_contacts_with` and `PlanOptions`: the same, with a time limit for the optimal scheduler.
  The search starts from the greedy plan, so its result is never worth less, and the plan says
  whether it is proven best. `time_until_first_pass` gives the longest limit that makes sense.
- `ContactPlan::to_text` and `ContactPlan::to_json`: the plan for a person and for a program.
- `guidance`: what each reason code means and what would change the outcome.
- `window_from_pass` and the time conversions underneath it, which round inward so a window never
  claims contact time the pass did not have.

```text
cargo run -p maat-ground --example contact_plan
cargo run -p maat-ground --example window_survey
```

License: MIT.
