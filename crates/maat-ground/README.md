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
  optional antenna `SlewLimits`.
- `contact_windows`: every contact opportunity for N satellites over M stations, as a sorted list
  of flight `ContactWindow`s. Passes that are found but cannot be used come back separately, each
  with a `MAAT-xxx` reason code. A satellite that cannot be propagated is reported without
  stopping the rest.
- `window_from_pass` and the time conversions underneath it, which round inward so a window never
  claims contact time the pass did not have.

```text
cargo run -p maat-ground --example window_survey
```

License: MIT.
