# maat-flight

The vehicle layer of the [Ma'at](https://github.com/IsomorphicAlgo/Ma-at) contact scheduler.

- `#![no_std]`, no heap, no dependencies, no `unsafe`.
- Integer-only: whole UTC seconds and hundredths of a degree, so results are bit-identical on
  every target.
- No panicking constructs in library code (`unwrap`, `expect`, `panic!`, slice indexing and
  unchecked arithmetic are denied by lint).

It holds the types the scheduler works on (`ContactWindow`, `SatelliteId`, `StationId`,
`Priority`, `Timestamp`) and the stable `MAAT-xxx` reason codes that explain every decision.

This crate does not predict passes. Contact windows are computed on the ground by `maat-ground`
(built on [Ephemerust](https://crates.io/crates/ephemerust)) and handed to this crate as plain data.

Designed to be flight-suitable. It is **not** flight-qualified.

License: MIT.
