//! Turning a predicted pass into a flight contact window.

use ephemerust::Pass;
use maat_flight::{ContactWindow, Elevation, SatelliteId, StationId};

use crate::error::GroundError;
use crate::time::{timestamp_ceil, timestamp_floor};

/// Converts one Ephemerust [`Pass`] into a flight [`ContactWindow`].
///
/// Values are rounded **inward**, so the window never overstates the pass:
///
/// | Pass value | Rounding | Why |
/// |---|---|---|
/// | AOS | up to the next whole second | never start before the satellite is above the mask |
/// | LOS | down to the whole second | never continue after it has dropped below the mask |
/// | maximum elevation | down to 0.01 degrees | never claim a better pass than predicted |
///
/// The window is therefore up to two seconds shorter than the pass, and never longer.
///
/// # Errors
///
/// - [`GroundError::InvalidElevation`] if the pass's maximum elevation is NaN, infinite, negative
///   or above 90 degrees.
/// - [`GroundError::Window`] if nothing is left after rounding, which happens only for a pass
///   shorter than about a second.
pub fn window_from_pass(
    satellite: SatelliteId,
    station: StationId,
    pass: &Pass,
) -> Result<ContactWindow, GroundError> {
    let max_elevation = elevation_floor(pass.max_elevation_deg)?;
    let aos = timestamp_ceil(pass.aos);
    let los = timestamp_floor(pass.los);
    Ok(ContactWindow::new(
        satellite,
        station,
        aos,
        los,
        max_elevation,
    )?)
}

/// Converts an elevation in degrees to hundredths of a degree, rounding down.
fn elevation_floor(degrees: f64) -> Result<Elevation, GroundError> {
    // `contains` is false for NaN, so this also rejects every non-finite value.
    if !(0.0..=90.0).contains(&degrees) {
        return Err(GroundError::InvalidElevation { degrees });
    }
    // In range, so the product is between 0 and 9000 and the cast cannot truncate or wrap.
    let centidegrees = (degrees * 100.0).floor() as u16;
    Ok(Elevation::from_centidegrees(centidegrees)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elevation_rounds_down_to_a_hundredth() {
        let cd = |degrees: f64| elevation_floor(degrees).unwrap().centidegrees();
        assert_eq!(cd(0.0), 0);
        assert_eq!(cd(0.009), 0);
        assert_eq!(cd(47.259), 4_725);
        assert_eq!(cd(47.25), 4_725);
        assert_eq!(cd(89.999), 8_999);
        assert_eq!(cd(90.0), 9_000);
    }

    #[test]
    fn elevation_never_rounds_up() {
        // Sweep the whole range in steps that land between hundredths.
        let mut degrees = 0.0_f64;
        while degrees <= 90.0 {
            let stored = f64::from(elevation_floor(degrees).unwrap().centidegrees()) / 100.0;
            assert!(stored <= degrees, "{stored} > {degrees}");
            assert!(
                degrees - stored < 0.01 + 1e-9,
                "lost more than 0.01 at {degrees}"
            );
            degrees += 0.003_7;
        }
    }

    #[test]
    fn elevation_outside_zero_to_ninety_is_rejected() {
        for bad in [
            -0.01,
            90.01,
            1e6,
            f64::NAN,
            f64::INFINITY,
            f64::NEG_INFINITY,
        ] {
            match elevation_floor(bad) {
                Err(GroundError::InvalidElevation { degrees }) => {
                    assert!(degrees == bad || (degrees.is_nan() && bad.is_nan()));
                }
                other => panic!("expected InvalidElevation for {bad}, got {other:?}"),
            }
        }
    }
}
