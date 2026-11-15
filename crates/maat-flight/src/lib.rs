#![no_std]
#![forbid(unsafe_code)]
#![warn(missing_docs)]
// Flight discipline (Methodology D-007): library code must not contain a construct that can
// panic or silently wrap. Each of these is a hard error everywhere except in the unit tests,
// where `unwrap` on a known-good value is the clearest way to write an assertion.
#![cfg_attr(
    not(test),
    deny(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing,
        clippy::arithmetic_side_effects,
        clippy::float_arithmetic
    )
)]
//! # maat-flight
//!
//! The **vehicle layer** of the Ma'at contact scheduler: the types a scheduler works on and the
//! stable reason codes that explain its decisions. Everything here is written to be suitable for
//! running onboard a spacecraft as well as on the ground.
//!
//! ## What "flight-suitable" means here
//!
//! - **`#![no_std]`, no heap.** The crate depends on `core` and nothing else. It never allocates.
//! - **Integer-only.** Time is whole UTC seconds ([`Timestamp`], [`Seconds`]) and elevation is
//!   hundredths of a degree ([`Elevation`]). No floating point means results are bit-identical on
//!   every processor, and every type is `Eq` and `Hash`.
//! - **No panic paths.** `unwrap`, `expect`, `panic!`, slice indexing and unchecked arithmetic
//!   are denied by lint. Fallible operations return `Option` or `Result`.
//! - **No `unsafe`.**
//!
//! This is a design goal, stated honestly: the crate is *designed to be flight-suitable*. It is
//! **not flight-qualified**, and it has not been through any formal verification or certification.
//!
//! ## What this crate does not do
//!
//! It does not predict passes. Pass prediction needs an orbit propagator, calendar time and
//! floating-point trigonometry, which live in the human layer (`maat-ground`, built on
//! Ephemerust). Contact windows arrive here as plain data.
//!
//! ## Conventions
//!
//! - A contact window is the **half-open** interval `[aos, los)`: the satellite is in contact at
//!   `aos` and no longer in contact at `los`. Two windows that merely touch do not overlap.
//! - A higher [`Priority`] value wins.
//! - Reason codes are stable. Once published, a `MAAT-xxx` number never changes meaning.
//!
//! ## Example
//!
//! ```
//! use maat_flight::{ContactWindow, Elevation, ReasonCode, SatelliteId, Seconds, StationId, Timestamp};
//!
//! let iss = SatelliteId::new(25544);
//! let station = StationId::new(1);
//!
//! // A nine-minute pass that peaks at 47.25 degrees.
//! let aos = Timestamp::from_unix_seconds(1_800_000_000);
//! let los = Timestamp::from_unix_seconds(1_800_000_540);
//! let peak = Elevation::from_centidegrees(4_725)?;
//! let pass = ContactWindow::new(iss, station, aos, los, peak)?;
//! assert_eq!(pass.duration(), Seconds::new(540));
//!
//! // Invalid input is refused with a stable, explainable code instead of a panic.
//! let backwards = ContactWindow::new(iss, station, los, aos, peak).unwrap_err();
//! assert_eq!(backwards.reason(), ReasonCode::LosNotAfterAos);
//! assert_eq!(backwards.reason().id(), "MAAT-201");
//! # Ok::<(), maat_flight::WindowError>(())
//! ```

pub mod ids;
pub mod reason;
pub mod time;
pub mod window;

pub use ids::{Priority, SatelliteId, StationId};
pub use reason::{ReasonClass, ReasonCode};
pub use time::{Seconds, Timestamp};
pub use window::{ContactWindow, Elevation, WindowError};
