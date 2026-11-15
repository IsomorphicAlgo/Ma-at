//! The greedy contact scheduler.
//!
//! Given a list of contact windows and what each satellite is asking for, it decides which
//! windows to use and gives a [`ReasonCode`] for every one, used or not.
//!
//! ## The rule
//!
//! Windows are considered one at a time, **best first**, and a window is scheduled unless
//! something already scheduled rules it out. "Best" is a fixed ranking:
//!
//! 1. higher [`Priority`] first;
//! 2. then the longer pass;
//! 3. then the earlier AOS;
//! 4. then the lower station number, the lower satellite number, and finally the position in
//!    the input.
//!
//! The last steps exist only so that the ranking is total: the same input always gives the same
//! plan, on any processor.
//!
//! A window is turned down, in this order of checks, when:
//!
//! | Check | Code |
//! |---|---|
//! | no request names its satellite | `MAAT-106` |
//! | it is shorter than the satellite's minimum contact | `MAAT-103` |
//! | passes already scheduled meet the satellite's need | `MAAT-104` |
//! | it overlaps a scheduled pass at its station | `MAAT-101` (higher priority) or `MAAT-107` (tie) |
//! | it is too close to a scheduled pass at its station for the station to turn round | `MAAT-108` |
//! | its satellite is scheduled with another station at that time | `MAAT-102` |
//!
//! ## What "greedy" costs
//!
//! Greedy never reconsiders. It can take one long pass that blocks two shorter ones worth more
//! together. The optimal scheduler in `maat-ground` exists to measure and recover that loss; this
//! one exists because it is small, predictable and needs no memory of its own.
//!
//! ## Flight properties
//!
//! - **No allocation.** The caller supplies the output slice. The scheduler keeps no state.
//! - **Bounded run time.** For `n` windows, `r` requests and `s` station rules it makes at most
//!   `n` passes over the windows to pick the next one and `n` passes to judge it, so the work is
//!   proportional to `n² + n·(r + s) + r² + s²` and to nothing else. There is no recursion and
//!   no loop whose length depends on the values of the data.
//! - **No panic paths.** Every index is checked and every sum saturates.
//!
//! ## Assumptions
//!
//! - A station serves one satellite at a time, and needs time between passes: a fixed
//!   turnaround, plus the time to turn its antenna from the end of one pass to the start of the
//!   next when the windows record where the antenna points (see [`StationRule`]).
//! - An antenna turns the short way round in azimuth. A mount with a hard stop is not modelled.
//! - A satellite talks to one station at a time.
//! - A scheduled pass is used whole. A pass is never trimmed to fit.

use core::cmp::Ordering;
use core::fmt;

use crate::ids::{Priority, SatelliteId, StationId};
use crate::reason::ReasonCode;
use crate::time::Seconds;
use crate::window::{ContactWindow, Pointing};

/// What one satellite is asking the scheduler for.
///
/// ```
/// use maat_flight::{Priority, SatelliteId, SatelliteRequest, Seconds};
///
/// // Twenty minutes of contact, in passes of at least two minutes, at high priority.
/// let request = SatelliteRequest::new(SatelliteId::new(25544), Priority::new(200))
///     .with_min_contact(Seconds::new(120))
///     .with_need(Seconds::new(1_200));
/// assert_eq!(request.need(), Seconds::new(1_200));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SatelliteRequest {
    satellite: SatelliteId,
    priority: Priority,
    min_contact: Seconds,
    need: Seconds,
}

impl SatelliteRequest {
    /// A request with no minimum pass length and no limit on contact time: every pass the
    /// satellite can get is wanted.
    #[must_use]
    pub const fn new(satellite: SatelliteId, priority: Priority) -> Self {
        Self {
            satellite,
            priority,
            min_contact: Seconds::ZERO,
            need: Seconds::MAX,
        }
    }

    /// Sets the shortest pass worth scheduling. A shorter pass is turned down with `MAAT-103`.
    #[must_use]
    pub const fn with_min_contact(mut self, min_contact: Seconds) -> Self {
        self.min_contact = min_contact;
        self
    }

    /// Sets how much contact time the satellite needs in total.
    ///
    /// Passes are scheduled until their combined length reaches the need; later passes are
    /// turned down with `MAAT-104`. Because a pass is used whole, the total can go over.
    #[must_use]
    pub const fn with_need(mut self, need: Seconds) -> Self {
        self.need = need;
        self
    }

    /// The satellite this request is for.
    #[must_use]
    pub const fn satellite(&self) -> SatelliteId {
        self.satellite
    }

    /// The satellite's priority. A higher value wins.
    #[must_use]
    pub const fn priority(&self) -> Priority {
        self.priority
    }

    /// The shortest pass worth scheduling.
    #[must_use]
    pub const fn min_contact(&self) -> Seconds {
        self.min_contact
    }

    /// The total contact time wanted. [`Seconds::MAX`] means "as much as possible".
    #[must_use]
    pub const fn need(&self) -> Seconds {
        self.need
    }

    /// `true` if the request puts no limit on contact time, so its need can never be "met".
    #[must_use]
    pub const fn is_unlimited(&self) -> bool {
        self.need.get() == Seconds::MAX.get()
    }

    /// `true` if `granted` seconds of contact satisfy the request.
    ///
    /// Always `false` for an unlimited request, even if the running total has saturated at
    /// [`Seconds::MAX`].
    #[must_use]
    pub const fn is_met_by(&self, granted: Seconds) -> bool {
        !self.is_unlimited() && granted.get() >= self.need.get()
    }
}

/// How fast a station's antenna can turn about each axis, in hundredths of a degree per second.
///
/// The scheduler uses this to work out how long the station needs between two passes: the
/// further apart the end of one pass and the start of the next are in the sky, the longer the
/// turn.
///
/// ```
/// use maat_flight::SlewRates;
///
/// // 6 degrees per second in azimuth, 2.5 in elevation.
/// let rates = SlewRates::new(600, 250).expect("both rates are above zero");
/// assert_eq!(rates.azimuth_centidegrees_per_second(), 600);
/// // A mount that cannot turn has no slew rate.
/// assert!(SlewRates::new(0, 250).is_none());
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SlewRates {
    azimuth: u16,
    elevation: u16,
}

impl SlewRates {
    /// Builds slew rates from hundredths of a degree per second.
    ///
    /// Returns `None` if either rate is zero: an axis that cannot move could never reach the
    /// next pass, and a zero is far more likely to be a missing value than a real one.
    #[must_use]
    pub const fn new(
        azimuth_centidegrees_per_second: u16,
        elevation_centidegrees_per_second: u16,
    ) -> Option<Self> {
        if azimuth_centidegrees_per_second == 0 || elevation_centidegrees_per_second == 0 {
            None
        } else {
            Some(Self {
                azimuth: azimuth_centidegrees_per_second,
                elevation: elevation_centidegrees_per_second,
            })
        }
    }

    /// The azimuth rate in hundredths of a degree per second.
    #[must_use]
    pub const fn azimuth_centidegrees_per_second(&self) -> u16 {
        self.azimuth
    }

    /// The elevation rate in hundredths of a degree per second.
    #[must_use]
    pub const fn elevation_centidegrees_per_second(&self) -> u16 {
        self.elevation
    }

    /// How long the antenna takes to turn from one pointing to another, in whole seconds,
    /// rounded up.
    ///
    /// Both axes move at once, so the time is that of the slower axis. Azimuth goes the short
    /// way round, which assumes a mount that can turn freely through north.
    ///
    /// ```
    /// use maat_flight::{Azimuth, Elevation, Pointing, Seconds, SlewRates};
    ///
    /// let rates = SlewRates::new(600, 250).unwrap(); // 6 and 2.5 degrees per second
    /// let at = |azimuth, elevation| Pointing {
    ///     azimuth: Azimuth::from_centidegrees(azimuth).unwrap(),
    ///     elevation: Elevation::from_centidegrees(elevation).unwrap(),
    /// };
    /// // 90 degrees of azimuth at 6 degrees per second is 15 s; elevation does not change.
    /// assert_eq!(rates.slew_time(at(4_500, 1_000), at(13_500, 1_000)), Seconds::new(15));
    /// // 91 degrees takes 15.17 s, which rounds up to 16.
    /// assert_eq!(rates.slew_time(at(4_500, 1_000), at(13_600, 1_000)), Seconds::new(16));
    /// ```
    #[must_use]
    pub fn slew_time(&self, from: Pointing, to: Pointing) -> Seconds {
        let azimuth = seconds_to_turn(from.azimuth.separation(to.azimuth), self.azimuth);
        let elevation = seconds_to_turn(
            from.elevation
                .centidegrees()
                .abs_diff(to.elevation.centidegrees()),
            self.elevation,
        );
        Seconds::new(azimuth.max(elevation))
    }
}

/// Whole seconds, rounded up, to turn `angle` hundredths of a degree at `rate` per second.
fn seconds_to_turn(angle: u16, rate: u16) -> u32 {
    let (angle, rate) = (u32::from(angle), u32::from(rate));
    // `rate` is never zero (see `SlewRates::new`), but nothing here relies on that: a zero rate
    // reads as "never arrives" instead of dividing by zero.
    angle
        .checked_add(rate.saturating_sub(1))
        .and_then(|rounded_up| rounded_up.checked_div(rate))
        .unwrap_or(u32::MAX)
}

/// A scheduling rule for one ground station: how long it needs between passes.
///
/// The time needed between two passes has two parts:
///
/// - a **fixed turnaround**, the same after every pass: time to reconfigure the radio, settle
///   the mount, or whatever the station needs regardless of where it points;
/// - a **slew time** that depends on the two passes: how long the antenna takes to turn from
///   where the first pass ends to where the next begins. It applies only when the rule has
///   [`SlewRates`] and both windows record their pointing; otherwise it is zero.
///
/// A station that has no rule needs no time between passes.
///
/// ```
/// use maat_flight::{Seconds, SlewRates, StationId, StationRule};
///
/// let rule = StationRule::new(StationId::new(1), Seconds::new(30))
///     .with_slew_rates(SlewRates::new(600, 250).unwrap());
/// assert_eq!(rule.turnaround(), Seconds::new(30));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StationRule {
    station: StationId,
    turnaround: Seconds,
    slew_rates: Option<SlewRates>,
}

impl StationRule {
    /// A rule giving the station a fixed `turnaround` between the end of one pass and the start
    /// of the next, and no slew time.
    #[must_use]
    pub const fn new(station: StationId, turnaround: Seconds) -> Self {
        Self {
            station,
            turnaround,
            slew_rates: None,
        }
    }

    /// The rule for a station that needs no time between passes.
    #[must_use]
    pub const fn none(station: StationId) -> Self {
        Self::new(station, Seconds::ZERO)
    }

    /// Returns the rule with the antenna's slew rates set, so that the time between two passes
    /// also covers the turn from one to the other.
    #[must_use]
    pub const fn with_slew_rates(mut self, rates: SlewRates) -> Self {
        self.slew_rates = Some(rates);
        self
    }

    /// The station this rule is for.
    #[must_use]
    pub const fn station(&self) -> StationId {
        self.station
    }

    /// The fixed time the station needs between passes, before any slew time.
    #[must_use]
    pub const fn turnaround(&self) -> Seconds {
        self.turnaround
    }

    /// The antenna's slew rates, if the rule has them.
    #[must_use]
    pub const fn slew_rates(&self) -> Option<SlewRates> {
        self.slew_rates
    }

    /// How long the antenna takes to turn from the end of `earlier` to the start of `later`.
    ///
    /// Zero if the rule has no slew rates or either window does not record its pointing.
    #[must_use]
    pub fn slew_time(&self, earlier: &ContactWindow, later: &ContactWindow) -> Seconds {
        match (
            self.slew_rates,
            earlier.los_pointing(),
            later.aos_pointing(),
        ) {
            (Some(rates), Some(from), Some(to)) => rates.slew_time(from, to),
            _ => Seconds::ZERO,
        }
    }

    /// The whole time the station needs between the end of `earlier` and the start of `later`:
    /// the fixed turnaround plus the slew time.
    #[must_use]
    pub fn gap_needed(&self, earlier: &ContactWindow, later: &ContactWindow) -> Seconds {
        self.turnaround
            .saturating_add(self.slew_time(earlier, later))
    }
}

/// How two contact windows get in each other's way.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Conflict {
    /// Same station, and the windows share at least one second.
    StationOverlap,
    /// Same station, no shared second, but less time between them than the station needs to
    /// turn round: its fixed turnaround plus the antenna's slew from one pass to the next.
    StationTurnaround,
    /// Same satellite at two different stations, and the windows share at least one second.
    SatelliteOverlap,
}

/// Tells whether two windows can both be in a plan, and if not, why not.
///
/// `rule` is the rule of the station the windows share; it matters only when both windows are at
/// the same station. This one function defines "conflict" for the whole project: the greedy
/// scheduler, the optimal scheduler and the tests all call it.
///
/// ```
/// use maat_flight::{
///     Conflict, ContactWindow, Elevation, SatelliteId, Seconds, StationId, StationRule, Timestamp,
/// };
/// use maat_flight::schedule::conflict;
///
/// let station = StationId::new(1);
/// let window = |sat, aos, los| {
///     ContactWindow::new(
///         SatelliteId::new(sat),
///         station,
///         Timestamp::from_unix_seconds(aos),
///         Timestamp::from_unix_seconds(los),
///         Elevation::ZENITH,
///     )
///     .unwrap()
/// };
/// let first = window(1, 0, 600);
/// let next = window(2, 630, 900); // same station, 30 s later
///
/// assert_eq!(conflict(&first, &next, &StationRule::none(station)), None);
/// let slow = StationRule::new(station, Seconds::new(60));
/// assert_eq!(conflict(&first, &next, &slow), Some(Conflict::StationTurnaround));
/// ```
#[must_use]
pub fn conflict(a: &ContactWindow, b: &ContactWindow, rule: &StationRule) -> Option<Conflict> {
    let overlap = a.overlaps(b);
    if a.station() == b.station() {
        if overlap {
            return Some(Conflict::StationOverlap);
        }
        // No overlap, so one window ends at or before the other begins.
        let (earlier, later) = if a.los() <= b.aos() { (a, b) } else { (b, a) };
        // `seconds_until` is `None` when the gap is too long to hold in a `Seconds`, which is
        // no conflict at all.
        let gap = earlier.los().seconds_until(later.aos());
        let needed = rule.gap_needed(earlier, later);
        if gap.is_some_and(|gap| gap < needed) {
            return Some(Conflict::StationTurnaround);
        }
        return None;
    }
    if overlap && a.satellite() == b.satellite() {
        return Some(Conflict::SatelliteOverlap);
    }
    None
}

/// The scheduler's verdict on one window.
///
/// Decisions are written into a slice the caller provides, one per window, in the same order as
/// the windows. Create that slice from [`Decision::PENDING`]:
///
/// ```
/// use maat_flight::Decision;
///
/// let mut decisions = [Decision::PENDING; 64];
/// assert!(decisions.iter().all(|d| d.reason().is_none()));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Decision {
    reason: Option<ReasonCode>,
    priority: Priority,
    blocked_by: Option<usize>,
}

impl Decision {
    /// A slot the scheduler has not written. Use it to build the output slice.
    pub const PENDING: Self = Self {
        reason: None,
        priority: Priority::LOWEST,
        blocked_by: None,
    };

    const fn pending(priority: Priority) -> Self {
        Self {
            reason: None,
            priority,
            blocked_by: None,
        }
    }

    const fn decided(reason: ReasonCode, priority: Priority, blocked_by: Option<usize>) -> Self {
        Self {
            reason: Some(reason),
            priority,
            blocked_by,
        }
    }

    /// Why the window is, or is not, in the plan. `None` only for a slot that was never written.
    #[must_use]
    pub const fn reason(&self) -> Option<ReasonCode> {
        self.reason
    }

    /// `true` if the window is in the plan.
    #[must_use]
    pub const fn is_scheduled(&self) -> bool {
        matches!(self.reason, Some(ReasonCode::Scheduled))
    }

    /// For a window that lost to another, the position of that other window in the input.
    ///
    /// Set for `MAAT-101`, `102`, `107` and `108`. `None` for everything else.
    #[must_use]
    pub const fn blocked_by(&self) -> Option<usize> {
        self.blocked_by
    }
}

/// Totals for a finished schedule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct ScheduleSummary {
    /// How many windows are in the plan.
    pub scheduled: usize,
    /// How many windows were turned down.
    pub rejected: usize,
    /// Total length of the scheduled windows. Stops at [`Seconds::MAX`] instead of overflowing.
    pub contact: Seconds,
}

/// Why a schedule could not be produced at all.
///
/// These are problems with the request. When one is returned, nothing has been written to the
/// output slice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ScheduleError {
    /// The output slice has fewer slots than there are windows.
    OutputTooSmall {
        /// The number of windows, which is the number of slots needed.
        needed: usize,
        /// The number of slots that were provided.
        capacity: usize,
    },
    /// Two requests name the same satellite.
    DuplicateRequest {
        /// The satellite named twice.
        satellite: SatelliteId,
    },
    /// Two station rules name the same station.
    DuplicateStationRule {
        /// The station named twice.
        station: StationId,
    },
}

impl ScheduleError {
    /// The stable reason code for this error.
    #[must_use]
    pub const fn reason(&self) -> ReasonCode {
        match self {
            Self::OutputTooSmall { .. } => ReasonCode::CapacityExceeded,
            Self::DuplicateRequest { .. } => ReasonCode::DuplicateRequest,
            Self::DuplicateStationRule { .. } => ReasonCode::DuplicateStationRule,
        }
    }
}

impl fmt::Display for ScheduleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let id = self.reason().id();
        match self {
            Self::OutputTooSmall { needed, capacity } => write!(
                f,
                "{id}: there are {needed} windows to decide but the output slice holds only \
                 {capacity} decisions; it needs one slot per window"
            ),
            Self::DuplicateRequest { satellite } => write!(
                f,
                "{id}: satellite {satellite} has more than one contact request; combine them, \
                 because the scheduler cannot tell which priority and need to use"
            ),
            Self::DuplicateStationRule { station } => write!(
                f,
                "{id}: station {station} has more than one rule; give each station at most one"
            ),
        }
    }
}

impl core::error::Error for ScheduleError {}

/// Finds the request for a satellite, and applies the checks that depend on the window alone.
///
/// Returns the request if the window is a candidate for scheduling. Both schedulers screen
/// windows with this function, so they cannot disagree about which windows are candidates.
///
/// # Errors
///
/// The [`ReasonCode`] that rules the window out: `MAAT-106` if no request names its satellite,
/// `MAAT-103` if it is shorter than the request's minimum contact.
pub fn screen<'r>(
    window: &ContactWindow,
    requests: &'r [SatelliteRequest],
) -> Result<&'r SatelliteRequest, ReasonCode> {
    let Some(request) = find_request(requests, window.satellite()) else {
        return Err(ReasonCode::NoRequest);
    };
    if window.duration() < request.min_contact() {
        return Err(ReasonCode::BelowMinimumDuration);
    }
    Ok(request)
}

/// Decides the order in which two candidate windows are considered. `Less` means `a` goes first.
///
/// This is the ranking described in the [module docs](self): priority, then length, then AOS,
/// then station and satellite number.
#[must_use]
pub fn rank(
    a_priority: Priority,
    a: &ContactWindow,
    b_priority: Priority,
    b: &ContactWindow,
) -> Ordering {
    b_priority
        .cmp(&a_priority)
        .then(b.duration().cmp(&a.duration()))
        .then(a.aos().cmp(&b.aos()))
        .then(a.station().cmp(&b.station()))
        .then(a.satellite().cmp(&b.satellite()))
}

/// Schedules contact windows greedily, best first. See the [module docs](self) for the rules.
///
/// `decisions` receives one [`Decision`] per window, in the order of `windows`. It must have at
/// least `windows.len()` slots; any further slots are left untouched.
///
/// # Errors
///
/// - [`ScheduleError::OutputTooSmall`] if `decisions` is shorter than `windows`.
/// - [`ScheduleError::DuplicateRequest`] if two requests name one satellite.
/// - [`ScheduleError::DuplicateStationRule`] if two rules name one station.
///
/// # Example
///
/// ```
/// use maat_flight::{
///     ContactWindow, Decision, Elevation, Priority, ReasonCode, SatelliteId, SatelliteRequest,
///     StationId, Timestamp, schedule_greedy,
/// };
///
/// let station = StationId::new(1);
/// let window = |sat, aos, los| {
///     ContactWindow::new(
///         SatelliteId::new(sat),
///         station,
///         Timestamp::from_unix_seconds(aos),
///         Timestamp::from_unix_seconds(los),
///         Elevation::ZENITH,
///     )
/// };
/// // Two satellites want the same station at overlapping times.
/// let windows = [window(10, 0, 600)?, window(20, 300, 900)?];
/// let requests = [
///     SatelliteRequest::new(SatelliteId::new(10), Priority::new(1)),
///     SatelliteRequest::new(SatelliteId::new(20), Priority::new(9)),
/// ];
///
/// let mut decisions = [Decision::PENDING; 2];
/// let summary = schedule_greedy(&windows, &requests, &[], &mut decisions).unwrap();
///
/// // The higher priority wins the station; the other pass is told why, and by whom.
/// assert_eq!(summary.scheduled, 1);
/// assert_eq!(decisions[1].reason(), Some(ReasonCode::Scheduled));
/// assert_eq!(decisions[0].reason(), Some(ReasonCode::StationBusy));
/// assert_eq!(decisions[0].blocked_by(), Some(1));
/// # Ok::<(), maat_flight::WindowError>(())
/// ```
pub fn schedule_greedy(
    windows: &[ContactWindow],
    requests: &[SatelliteRequest],
    stations: &[StationRule],
    decisions: &mut [Decision],
) -> Result<ScheduleSummary, ScheduleError> {
    validate(requests, stations)?;
    let capacity = decisions.len();
    let Some(slots) = decisions.get_mut(..windows.len()) else {
        return Err(ScheduleError::OutputTooSmall {
            needed: windows.len(),
            capacity,
        });
    };

    // Screen: settle every window that can be decided on its own, and mark the rest pending.
    for (slot, window) in slots.iter_mut().zip(windows) {
        *slot = match screen(window, requests) {
            Ok(request) => Decision::pending(request.priority()),
            Err(reason) => Decision::decided(reason, Priority::LOWEST, None),
        };
    }

    // Decide: one window per round, best first. Each round settles exactly one pending window,
    // so `windows.len()` rounds are always enough.
    for _ in 0..windows.len() {
        let Some(best) = best_pending(windows, slots) else {
            break;
        };
        let verdict = judge(best, windows, slots, requests, stations);
        if let Some(slot) = slots.get_mut(best) {
            *slot = verdict;
        }
    }

    let scheduled = slots.iter().filter(|d| d.is_scheduled()).count();
    let contact = slots
        .iter()
        .zip(windows)
        .filter(|(decision, _)| decision.is_scheduled())
        .fold(Seconds::ZERO, |total, (_, window)| {
            total.saturating_add(window.duration())
        });
    Ok(ScheduleSummary {
        scheduled,
        rejected: slots.len().saturating_sub(scheduled),
        contact,
    })
}

/// The position of the best window still waiting for a decision, or `None` if none is waiting.
fn best_pending(windows: &[ContactWindow], slots: &[Decision]) -> Option<usize> {
    let mut best: Option<(usize, Priority, &ContactWindow)> = None;
    for (index, (window, slot)) in windows.iter().zip(slots).enumerate() {
        if slot.reason.is_some() {
            continue;
        }
        let better = match best {
            None => true,
            // Strictly better only: on a complete tie the earlier position keeps its place.
            Some((_, best_priority, best_window)) => {
                rank(slot.priority, window, best_priority, best_window) == Ordering::Less
            }
        };
        if better {
            best = Some((index, slot.priority, window));
        }
    }
    best.map(|(index, _, _)| index)
}

/// Decides one window against everything scheduled so far.
fn judge(
    index: usize,
    windows: &[ContactWindow],
    slots: &[Decision],
    requests: &[SatelliteRequest],
    stations: &[StationRule],
) -> Decision {
    // Both lookups succeeded when the window was screened; if either fails here the window is
    // reported as having no request, which is the conservative answer.
    let Some(window) = windows.get(index) else {
        return Decision::decided(ReasonCode::NoRequest, Priority::LOWEST, None);
    };
    let Some(request) = find_request(requests, window.satellite()) else {
        return Decision::decided(ReasonCode::NoRequest, Priority::LOWEST, None);
    };
    let priority = request.priority();
    let rule = rule_for(stations, window.station());

    let mut granted = Seconds::ZERO;
    let mut station_overlap: Option<(usize, Priority)> = None;
    let mut station_turnaround: Option<usize> = None;
    let mut satellite_overlap: Option<usize> = None;

    for (other_index, (other, slot)) in windows.iter().zip(slots).enumerate() {
        if !slot.is_scheduled() {
            continue;
        }
        if other.satellite() == window.satellite() {
            granted = granted.saturating_add(other.duration());
        }
        // Keep the first blocker of each kind, so the answer does not depend on scan details.
        match conflict(window, other, &rule) {
            Some(Conflict::StationOverlap) => {
                station_overlap = station_overlap.or(Some((other_index, slot.priority)));
            }
            Some(Conflict::StationTurnaround) => {
                station_turnaround = station_turnaround.or(Some(other_index));
            }
            Some(Conflict::SatelliteOverlap) => {
                satellite_overlap = satellite_overlap.or(Some(other_index));
            }
            None => {}
        }
    }

    if request.is_met_by(granted) {
        return Decision::decided(ReasonCode::NeedAlreadyMet, priority, None);
    }
    if let Some((blocker, blocker_priority)) = station_overlap {
        let reason = if blocker_priority > priority {
            ReasonCode::StationBusy
        } else {
            ReasonCode::StationTieLost
        };
        return Decision::decided(reason, priority, Some(blocker));
    }
    if let Some(blocker) = station_turnaround {
        return Decision::decided(ReasonCode::StationTurnaround, priority, Some(blocker));
    }
    if let Some(blocker) = satellite_overlap {
        return Decision::decided(ReasonCode::SatelliteBusy, priority, Some(blocker));
    }
    Decision::decided(ReasonCode::Scheduled, priority, None)
}

/// Checks the requests and station rules that every scheduler takes.
///
/// # Errors
///
/// [`ScheduleError::DuplicateRequest`] or [`ScheduleError::DuplicateStationRule`] if a satellite
/// or a station is named twice.
pub fn validate(
    requests: &[SatelliteRequest],
    stations: &[StationRule],
) -> Result<(), ScheduleError> {
    check_unique_requests(requests)?;
    check_unique_rules(stations)
}

/// The rule for a station: its own if it has one, otherwise a rule that needs no time between
/// passes.
#[must_use]
pub fn rule_for(stations: &[StationRule], station: StationId) -> StationRule {
    stations
        .iter()
        .find(|rule| rule.station == station)
        .copied()
        .unwrap_or(StationRule::none(station))
}

fn find_request(
    requests: &[SatelliteRequest],
    satellite: SatelliteId,
) -> Option<&SatelliteRequest> {
    requests
        .iter()
        .find(|request| request.satellite == satellite)
}

fn check_unique_requests(requests: &[SatelliteRequest]) -> Result<(), ScheduleError> {
    for (index, request) in requests.iter().enumerate() {
        let repeated = requests
            .iter()
            .take(index)
            .any(|earlier| earlier.satellite == request.satellite);
        if repeated {
            return Err(ScheduleError::DuplicateRequest {
                satellite: request.satellite,
            });
        }
    }
    Ok(())
}

fn check_unique_rules(stations: &[StationRule]) -> Result<(), ScheduleError> {
    for (index, rule) in stations.iter().enumerate() {
        let repeated = stations
            .iter()
            .take(index)
            .any(|earlier| earlier.station == rule.station);
        if repeated {
            return Err(ScheduleError::DuplicateStationRule {
                station: rule.station,
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;
    use crate::time::Timestamp;
    use crate::window::{Azimuth, Elevation};
    use std::format;
    use std::string::ToString;
    use std::vec;
    use std::vec::Vec;

    // --- Helpers -------------------------------------------------------------------------------

    fn window(satellite: u32, station: u16, aos: i64, los: i64) -> ContactWindow {
        ContactWindow::new(
            SatelliteId::new(satellite),
            StationId::new(station),
            Timestamp::from_unix_seconds(aos),
            Timestamp::from_unix_seconds(los),
            Elevation::ZENITH,
        )
        .unwrap()
    }

    fn request(satellite: u32, priority: u8) -> SatelliteRequest {
        SatelliteRequest::new(SatelliteId::new(satellite), Priority::new(priority))
    }

    fn rule(station: u16, turnaround: u32) -> StationRule {
        StationRule::new(StationId::new(station), Seconds::new(turnaround))
    }

    /// `conflict` for two windows whose station has only a fixed turnaround time.
    fn clash(a: &ContactWindow, b: &ContactWindow, turnaround: Seconds) -> Option<Conflict> {
        conflict(a, b, &StationRule::new(a.station(), turnaround))
    }

    fn at(azimuth: u16, elevation: u16) -> Pointing {
        Pointing {
            azimuth: Azimuth::from_centidegrees(azimuth).unwrap(),
            elevation: Elevation::from_centidegrees(elevation).unwrap(),
        }
    }

    /// A window that rises at `rise_azimuth` and sets at `set_azimuth`, both at 10 degrees up.
    fn pointed(
        satellite: u32,
        station: u16,
        aos: i64,
        los: i64,
        rise_azimuth: u16,
        set_azimuth: u16,
    ) -> ContactWindow {
        window(satellite, station, aos, los)
            .with_pointing(at(rise_azimuth, 1_000), at(set_azimuth, 1_000))
    }

    /// 6 degrees per second in azimuth and 2.5 in elevation: a common amateur rotator.
    fn rotator() -> SlewRates {
        SlewRates::new(600, 250).unwrap()
    }

    /// Runs the scheduler and returns the decisions as `(reason, blocked_by)` pairs.
    fn run(
        windows: &[ContactWindow],
        requests: &[SatelliteRequest],
        stations: &[StationRule],
    ) -> Vec<(ReasonCode, Option<usize>)> {
        let mut decisions = vec![Decision::PENDING; windows.len()];
        schedule_greedy(windows, requests, stations, &mut decisions).unwrap();
        decisions
            .iter()
            .map(|d| (d.reason().expect("every window is decided"), d.blocked_by()))
            .collect()
    }

    const OK: (ReasonCode, Option<usize>) = (ReasonCode::Scheduled, None);

    fn lost(reason: ReasonCode, to: usize) -> (ReasonCode, Option<usize>) {
        (reason, Some(to))
    }

    // --- conflict ------------------------------------------------------------------------------

    #[test]
    fn overlapping_windows_at_one_station_conflict() {
        let a = window(1, 1, 0, 100);
        let b = window(2, 1, 99, 200);
        assert_eq!(clash(&a, &b, Seconds::ZERO), Some(Conflict::StationOverlap));
        assert_eq!(clash(&b, &a, Seconds::ZERO), Some(Conflict::StationOverlap));
    }

    #[test]
    fn back_to_back_windows_at_one_station_do_not_conflict_without_turnaround() {
        let a = window(1, 1, 0, 100);
        let b = window(2, 1, 100, 200);
        assert_eq!(clash(&a, &b, Seconds::ZERO), None);
        assert_eq!(clash(&b, &a, Seconds::ZERO), None);
    }

    #[test]
    fn turnaround_needs_a_gap_at_least_as_long_as_itself() {
        let a = window(1, 1, 0, 100);
        let gap_59 = window(2, 1, 159, 300);
        let gap_60 = window(2, 1, 160, 300);
        let turnaround = Seconds::new(60);
        for (x, y) in [(&a, &gap_59), (&gap_59, &a)] {
            assert_eq!(clash(x, y, turnaround), Some(Conflict::StationTurnaround));
        }
        for (x, y) in [(&a, &gap_60), (&gap_60, &a)] {
            assert_eq!(clash(x, y, turnaround), None);
        }
    }

    #[test]
    fn turnaround_does_not_apply_between_different_stations() {
        let a = window(1, 1, 0, 100);
        let b = window(2, 2, 100, 200);
        assert_eq!(clash(&a, &b, Seconds::MAX), None);
    }

    #[test]
    fn one_satellite_cannot_use_two_stations_at_once() {
        let a = window(7, 1, 0, 100);
        let b = window(7, 2, 50, 150);
        assert_eq!(
            clash(&a, &b, Seconds::ZERO),
            Some(Conflict::SatelliteOverlap)
        );
        // Back to back at different stations is a handover, not a conflict.
        let handover = window(7, 2, 100, 200);
        assert_eq!(clash(&a, &handover, Seconds::ZERO), None);
    }

    #[test]
    fn different_satellites_at_different_stations_never_conflict() {
        let a = window(1, 1, 0, 100);
        let b = window(2, 2, 0, 100);
        assert_eq!(clash(&a, &b, Seconds::MAX), None);
    }

    #[test]
    fn a_gap_too_long_to_measure_is_not_a_conflict() {
        // More than u32::MAX seconds apart: `seconds_until` gives None, which must read as
        // "no conflict", even with the longest possible turnaround.
        let a = window(1, 1, 0, 100);
        let far = window(2, 1, 10_000_000_000, 10_000_000_100);
        assert_eq!(clash(&a, &far, Seconds::MAX), None);
        assert_eq!(clash(&far, &a, Seconds::MAX), None);
    }

    #[test]
    fn conflict_is_symmetric() {
        // Every pair from a small grid of windows, with and without turnaround.
        let mut windows = Vec::new();
        for satellite in 1..=2 {
            for station in 1..=2 {
                for aos in [0, 40, 100, 130] {
                    windows.push(window(satellite, station, aos, aos + 60));
                }
            }
        }
        for turnaround in [Seconds::ZERO, Seconds::new(45)] {
            for a in &windows {
                for b in &windows {
                    assert_eq!(clash(a, b, turnaround), clash(b, a, turnaround));
                }
            }
        }
    }

    // --- Slew time -----------------------------------------------------------------------------

    #[test]
    fn slew_rates_must_be_above_zero_on_both_axes() {
        assert!(SlewRates::new(1, 1).is_some());
        assert!(SlewRates::new(0, 250).is_none());
        assert!(SlewRates::new(600, 0).is_none());
        let rates = rotator();
        assert_eq!(rates.azimuth_centidegrees_per_second(), 600);
        assert_eq!(rates.elevation_centidegrees_per_second(), 250);
    }

    #[test]
    fn slew_time_rounds_up_to_whole_seconds() {
        let rates = rotator(); // 6 deg/s in azimuth
        let turn = |degrees_x100: u16| rates.slew_time(at(0, 1_000), at(degrees_x100, 1_000));
        assert_eq!(turn(0), Seconds::ZERO);
        assert_eq!(turn(1), Seconds::new(1)); // any turn at all takes a second
        assert_eq!(turn(600), Seconds::new(1)); // exactly one second's worth
        assert_eq!(turn(601), Seconds::new(2));
        assert_eq!(turn(9_000), Seconds::new(15)); // 90 degrees
        assert_eq!(turn(18_000), Seconds::new(30)); // half a turn, the most there can be
    }

    #[test]
    fn slew_time_goes_the_short_way_round_in_azimuth() {
        let rates = rotator();
        // 350 to 10 degrees is 20 degrees through north: 4 s at 6 deg/s (3.33 rounded up).
        assert_eq!(
            rates.slew_time(at(35_000, 1_000), at(1_000, 1_000)),
            Seconds::new(4)
        );
        // 270 degrees apart on the dial is 90 the short way.
        assert_eq!(
            rates.slew_time(at(0, 1_000), at(27_000, 1_000)),
            Seconds::new(15)
        );
    }

    #[test]
    fn slew_time_is_that_of_the_slower_axis() {
        let rates = rotator(); // 6 deg/s azimuth, 2.5 deg/s elevation
        // 30 degrees of azimuth (5 s) and 50 degrees of elevation (20 s): elevation decides.
        assert_eq!(
            rates.slew_time(at(0, 1_000), at(3_000, 6_000)),
            Seconds::new(20)
        );
        // 120 degrees of azimuth (20 s) and 10 degrees of elevation (4 s): azimuth decides.
        assert_eq!(
            rates.slew_time(at(0, 1_000), at(12_000, 2_000)),
            Seconds::new(20)
        );
        // The same going the other way.
        assert_eq!(
            rates.slew_time(at(12_000, 2_000), at(0, 1_000)),
            Seconds::new(20)
        );
    }

    #[test]
    fn the_slowest_mount_and_the_longest_turn_do_not_overflow() {
        let crawl = SlewRates::new(1, 1).unwrap(); // 0.01 deg/s
        assert_eq!(
            crawl.slew_time(at(0, 0), at(18_000, 9_000)),
            Seconds::new(18_000)
        );
        let fast = SlewRates::new(u16::MAX, u16::MAX).unwrap();
        assert_eq!(fast.slew_time(at(0, 0), at(18_000, 9_000)), Seconds::new(1));
    }

    #[test]
    fn seconds_to_turn_treats_a_zero_rate_as_never_arriving() {
        // `SlewRates::new` refuses a zero rate, so this cannot be reached through the public
        // interface. The helper must still not divide by zero.
        assert_eq!(seconds_to_turn(100, 0), u32::MAX);
        assert_eq!(seconds_to_turn(0, 5), 0);
        assert_eq!(seconds_to_turn(u16::MAX, u16::MAX), 1);
    }

    #[test]
    fn a_rule_without_slew_rates_needs_only_its_fixed_turnaround() {
        let earlier = pointed(1, 1, 0, 600, 0, 9_000);
        let later = pointed(2, 1, 700, 900, 27_000, 0);
        let fixed = rule(1, 45);
        assert_eq!(fixed.slew_rates(), None);
        assert_eq!(fixed.slew_time(&earlier, &later), Seconds::ZERO);
        assert_eq!(fixed.gap_needed(&earlier, &later), Seconds::new(45));
        assert_eq!(
            StationRule::none(StationId::new(1)).gap_needed(&earlier, &later),
            Seconds::ZERO
        );
    }

    #[test]
    fn a_rule_with_slew_rates_adds_the_turn_between_the_two_passes() {
        // The first pass sets due east (90 degrees); the next rises due west (270 degrees).
        // That is half a turn: 30 s at 6 deg/s, on top of 45 s fixed.
        let earlier = pointed(1, 1, 0, 600, 0, 9_000);
        let later = pointed(2, 1, 700, 900, 27_000, 0);
        let slewing = rule(1, 45).with_slew_rates(rotator());
        assert_eq!(slewing.slew_rates(), Some(rotator()));
        assert_eq!(slewing.slew_time(&earlier, &later), Seconds::new(30));
        assert_eq!(slewing.gap_needed(&earlier, &later), Seconds::new(75));
    }

    #[test]
    fn the_turn_is_measured_from_where_the_first_pass_ends_to_where_the_second_begins() {
        // Direction matters. Pass A rises north and sets east; pass B rises south and sets west.
        let a = pointed(1, 1, 0, 600, 0, 9_000);
        let b = pointed(2, 1, 700, 900, 18_000, 27_000);
        let slewing = rule(1, 0).with_slew_rates(rotator());
        // A then B: east (90) to south (180) is 90 degrees, 15 s.
        assert_eq!(slewing.slew_time(&a, &b), Seconds::new(15));
        // B then A: west (270) to north (0) is 90 degrees too, but from different ends.
        assert_eq!(slewing.slew_time(&b, &a), Seconds::new(15));
        // Change where A sets and only the A-then-B turn changes.
        let a_sets_south = pointed(1, 1, 0, 600, 0, 18_000);
        assert_eq!(slewing.slew_time(&a_sets_south, &b), Seconds::ZERO);
        assert_eq!(slewing.slew_time(&b, &a_sets_south), Seconds::new(15));
    }

    #[test]
    fn slew_time_needs_pointing_on_both_windows() {
        let slewing = rule(1, 20).with_slew_rates(rotator());
        let with = pointed(1, 1, 0, 600, 0, 9_000);
        let without = window(2, 1, 700, 900);
        assert_eq!(slewing.slew_time(&with, &without), Seconds::ZERO);
        assert_eq!(slewing.slew_time(&without, &with), Seconds::ZERO);
        // The fixed turnaround still applies.
        assert_eq!(slewing.gap_needed(&with, &without), Seconds::new(20));
    }

    #[test]
    fn gap_needed_saturates_instead_of_overflowing() {
        let earlier = pointed(1, 1, 0, 600, 0, 9_000);
        let later = pointed(2, 1, 700, 900, 27_000, 0);
        let huge = StationRule::new(StationId::new(1), Seconds::MAX).with_slew_rates(rotator());
        assert_eq!(huge.gap_needed(&earlier, &later), Seconds::MAX);
    }

    #[test]
    fn a_long_turn_makes_a_conflict_out_of_a_gap_that_a_short_turn_fits_in() {
        // Twenty seconds between the passes, no fixed turnaround, 6 deg/s.
        let slewing = rule(1, 0).with_slew_rates(rotator());
        let first = pointed(1, 1, 0, 600, 0, 9_000); // sets due east
        let near = pointed(2, 1, 620, 900, 12_000, 0); // rises 30 degrees away: 5 s
        let far = pointed(2, 1, 620, 900, 27_000, 0); // rises half a turn away: 30 s
        assert_eq!(conflict(&first, &near, &slewing), None);
        assert_eq!(
            conflict(&first, &far, &slewing),
            Some(Conflict::StationTurnaround)
        );
        // Symmetric in its arguments: the earlier window is found by time, not by position.
        assert_eq!(
            conflict(&far, &first, &slewing),
            Some(Conflict::StationTurnaround)
        );
    }

    #[test]
    fn a_gap_exactly_as_long_as_the_turn_is_enough() {
        let slewing = rule(1, 10).with_slew_rates(rotator());
        let first = pointed(1, 1, 0, 600, 0, 9_000);
        // Half a turn: 30 s of slew plus 10 s fixed is 40 s.
        let just_enough = pointed(2, 1, 640, 900, 27_000, 0);
        let one_short = pointed(2, 1, 639, 900, 27_000, 0);
        assert_eq!(conflict(&first, &just_enough, &slewing), None);
        assert_eq!(
            conflict(&first, &one_short, &slewing),
            Some(Conflict::StationTurnaround)
        );
    }

    #[test]
    fn slew_time_does_not_apply_between_stations() {
        let slewing = rule(1, 0).with_slew_rates(SlewRates::new(1, 1).unwrap());
        let a = pointed(1, 1, 0, 600, 0, 9_000);
        let b = pointed(2, 2, 600, 900, 27_000, 0);
        assert_eq!(conflict(&a, &b, &slewing), None);
    }

    #[test]
    fn rule_for_finds_a_stations_rule_or_gives_one_that_needs_nothing() {
        let rules = [rule(1, 30), rule(2, 60).with_slew_rates(rotator())];
        assert_eq!(rule_for(&rules, StationId::new(2)), rules[1]);
        assert_eq!(
            rule_for(&rules, StationId::new(9)),
            StationRule::none(StationId::new(9))
        );
        assert_eq!(rule_for(&[], StationId::new(1)).turnaround(), Seconds::ZERO);
    }

    // --- rank ----------------------------------------------------------------------------------

    #[test]
    fn rank_puts_higher_priority_first_whatever_the_length() {
        let short = window(1, 1, 500, 510);
        let long = window(2, 1, 0, 900);
        let (high, low) = (Priority::new(9), Priority::new(1));
        assert_eq!(rank(high, &short, low, &long), Ordering::Less);
        assert_eq!(rank(low, &long, high, &short), Ordering::Greater);
    }

    #[test]
    fn rank_breaks_ties_by_length_then_aos_then_station_then_satellite() {
        let p = Priority::new(5);
        let base = window(2, 2, 100, 200);
        let longer = window(2, 2, 100, 201);
        let earlier = window(2, 2, 99, 199);
        let lower_station = window(2, 1, 100, 200);
        let lower_satellite = window(1, 2, 100, 200);
        for better in [&longer, &earlier, &lower_station, &lower_satellite] {
            assert_eq!(rank(p, better, p, &base), Ordering::Less, "{better:?}");
            assert_eq!(rank(p, &base, p, better), Ordering::Greater, "{better:?}");
        }
        assert_eq!(rank(p, &base, p, &base), Ordering::Equal);
        // Length is checked before AOS: a longer pass that starts later still goes first.
        let longer_but_later = window(2, 2, 150, 300);
        assert_eq!(rank(p, &longer_but_later, p, &earlier), Ordering::Less);
    }

    // --- screen --------------------------------------------------------------------------------

    #[test]
    fn screen_returns_the_matching_request() {
        let requests = [request(1, 3), request(2, 7)];
        let found = screen(&window(2, 1, 0, 100), &requests).unwrap();
        assert_eq!(found.priority(), Priority::new(7));
    }

    #[test]
    fn screen_rejects_a_satellite_nobody_asked_for() {
        assert_eq!(
            screen(&window(9, 1, 0, 100), &[request(1, 3)]),
            Err(ReasonCode::NoRequest)
        );
        assert_eq!(
            screen(&window(9, 1, 0, 100), &[]),
            Err(ReasonCode::NoRequest)
        );
    }

    #[test]
    fn screen_accepts_a_pass_exactly_as_long_as_the_minimum() {
        let requests = [request(1, 3).with_min_contact(Seconds::new(100))];
        assert!(screen(&window(1, 1, 0, 100), &requests).is_ok());
        assert_eq!(
            screen(&window(1, 1, 0, 99), &requests),
            Err(ReasonCode::BelowMinimumDuration)
        );
    }

    // --- The simple cases ----------------------------------------------------------------------

    #[test]
    fn nothing_to_schedule_is_an_empty_plan() {
        let summary = schedule_greedy(&[], &[], &[], &mut []).unwrap();
        assert_eq!(
            (summary.scheduled, summary.rejected, summary.contact),
            (0, 0, Seconds::ZERO)
        );
    }

    #[test]
    fn a_lone_window_is_scheduled() {
        assert_eq!(run(&[window(1, 1, 0, 600)], &[request(1, 1)], &[]), [OK]);
    }

    #[test]
    fn windows_that_do_not_compete_are_all_scheduled() {
        let windows = [
            window(1, 1, 0, 600),
            window(2, 2, 0, 600),   // other station, other satellite
            window(1, 1, 600, 900), // same station, back to back
            window(2, 1, 900, 1_200),
        ];
        assert_eq!(run(&windows, &[request(1, 1), request(2, 1)], &[]), [OK; 4]);
    }

    #[test]
    fn the_summary_counts_passes_and_adds_up_contact_time() {
        let windows = [
            window(1, 1, 0, 600),
            window(2, 1, 300, 500),
            window(3, 2, 0, 50),
        ];
        let requests = [request(1, 5), request(2, 1), request(3, 1)];
        let mut decisions = [Decision::PENDING; 3];
        let summary = schedule_greedy(&windows, &requests, &[], &mut decisions).unwrap();
        assert_eq!(summary.scheduled, 2);
        assert_eq!(summary.rejected, 1);
        assert_eq!(summary.contact, Seconds::new(650));
    }

    // --- Screening inside the scheduler --------------------------------------------------------

    #[test]
    fn a_satellite_without_a_request_is_dropped_and_blocks_nobody() {
        let windows = [window(9, 1, 0, 900), window(1, 1, 100, 200)];
        assert_eq!(
            run(&windows, &[request(1, 1)], &[]),
            [(ReasonCode::NoRequest, None), OK]
        );
    }

    #[test]
    fn a_pass_below_the_minimum_is_dropped_and_blocks_nobody() {
        let windows = [window(1, 1, 0, 90), window(2, 1, 50, 300)];
        let requests = [
            request(1, 9).with_min_contact(Seconds::new(120)),
            request(2, 1),
        ];
        assert_eq!(
            run(&windows, &requests, &[]),
            [(ReasonCode::BelowMinimumDuration, None), OK]
        );
    }

    // --- Station conflicts ---------------------------------------------------------------------

    #[test]
    fn the_higher_priority_takes_a_contested_station() {
        let windows = [window(1, 1, 0, 600), window(2, 1, 300, 900)];
        let requests = [request(1, 1), request(2, 9)];
        assert_eq!(
            run(&windows, &requests, &[]),
            [lost(ReasonCode::StationBusy, 1), OK]
        );
    }

    #[test]
    fn priority_beats_length() {
        // A ten-second high-priority pass displaces a fifteen-minute low-priority one.
        let windows = [window(1, 1, 0, 900), window(2, 1, 400, 410)];
        let requests = [request(1, 1), request(2, 2)];
        assert_eq!(
            run(&windows, &requests, &[]),
            [lost(ReasonCode::StationBusy, 1), OK]
        );
    }

    #[test]
    fn at_equal_priority_the_longer_pass_wins_and_the_loser_is_told_it_was_a_tie() {
        let windows = [window(1, 1, 0, 300), window(2, 1, 200, 600)];
        let requests = [request(1, 5), request(2, 5)];
        assert_eq!(
            run(&windows, &requests, &[]),
            [lost(ReasonCode::StationTieLost, 1), OK]
        );
    }

    #[test]
    fn at_equal_priority_and_length_the_earlier_pass_wins() {
        let windows = [window(1, 1, 100, 400), window(2, 1, 0, 300)];
        let requests = [request(1, 5), request(2, 5)];
        assert_eq!(
            run(&windows, &requests, &[]),
            [lost(ReasonCode::StationTieLost, 1), OK]
        );
    }

    #[test]
    fn at_equal_priority_length_and_time_the_lower_satellite_number_wins() {
        let windows = [window(8, 1, 0, 300), window(3, 1, 0, 300)];
        let requests = [request(8, 5), request(3, 5)];
        assert_eq!(
            run(&windows, &requests, &[]),
            [lost(ReasonCode::StationTieLost, 1), OK]
        );
    }

    #[test]
    fn an_exact_duplicate_loses_to_the_copy_that_came_first() {
        let windows = [window(1, 1, 0, 300), window(1, 1, 0, 300)];
        assert_eq!(
            run(&windows, &[request(1, 5)], &[]),
            [OK, lost(ReasonCode::StationTieLost, 0)]
        );
    }

    #[test]
    fn a_dropped_pass_does_not_block_anyone() {
        // B loses the station to A. C overlaps B but not A, so C must be scheduled: greedy
        // compares against what is in the plan, not against everything it has looked at.
        let windows = [
            window(1, 1, 0, 300),   // A, priority 9
            window(2, 1, 200, 500), // B, priority 5, loses to A
            window(3, 1, 400, 700), // C, priority 1, overlaps B only
        ];
        let requests = [request(1, 9), request(2, 5), request(3, 1)];
        assert_eq!(
            run(&windows, &requests, &[]),
            [OK, lost(ReasonCode::StationBusy, 0), OK]
        );
    }

    #[test]
    fn a_pass_blocked_by_two_others_names_the_one_that_comes_first_in_the_input() {
        // The middle pass overlaps both scheduled passes. Either would be a true answer; the
        // rule is the earlier position in the input, so the explanation is reproducible.
        let windows = [
            window(1, 1, 0, 100),
            window(2, 1, 200, 300),
            window(3, 1, 50, 250),
        ];
        let requests = [request(1, 9), request(2, 9), request(3, 1)];
        assert_eq!(
            run(&windows, &requests, &[]),
            [OK, OK, lost(ReasonCode::StationBusy, 0)]
        );
        // Swap the two winners in the input and the answer follows the position, not the time.
        let swapped = [windows[1], windows[0], windows[2]];
        assert_eq!(
            run(&swapped, &requests, &[]),
            [OK, OK, lost(ReasonCode::StationBusy, 0)]
        );
    }

    #[test]
    fn greedy_takes_one_long_pass_over_two_short_ones_worth_more() {
        // The known weakness, pinned down so it stays a documented behaviour: at equal priority
        // the 400 s pass goes first and blocks two 300 s passes that would total 600 s.
        let windows = [
            window(1, 1, 0, 300),
            window(2, 1, 250, 650),
            window(3, 1, 600, 900),
        ];
        let requests = [request(1, 5), request(2, 5), request(3, 5)];
        assert_eq!(
            run(&windows, &requests, &[]),
            [
                lost(ReasonCode::StationTieLost, 1),
                OK,
                lost(ReasonCode::StationTieLost, 1)
            ]
        );
    }

    // --- Turnaround ----------------------------------------------------------------------------

    #[test]
    fn a_pass_inside_the_turnaround_gap_is_dropped() {
        let windows = [window(1, 1, 0, 600), window(2, 1, 630, 900)];
        let requests = [request(1, 9), request(2, 1)];
        assert_eq!(run(&windows, &requests, &[rule(1, 0)]), [OK, OK]);
        assert_eq!(
            run(&windows, &requests, &[rule(1, 60)]),
            [OK, lost(ReasonCode::StationTurnaround, 0)]
        );
        // Thirty seconds is exactly the gap, so thirty is enough and thirty-one is not.
        assert_eq!(run(&windows, &requests, &[rule(1, 30)]), [OK, OK]);
        assert_eq!(
            run(&windows, &requests, &[rule(1, 31)]),
            [OK, lost(ReasonCode::StationTurnaround, 0)]
        );
    }

    #[test]
    fn turnaround_applies_before_a_scheduled_pass_as_well_as_after() {
        let windows = [window(1, 1, 0, 570), window(2, 1, 600, 900)];
        let requests = [request(1, 1), request(2, 9)];
        assert_eq!(
            run(&windows, &requests, &[rule(1, 60)]),
            [lost(ReasonCode::StationTurnaround, 1), OK]
        );
    }

    #[test]
    fn the_scheduler_gives_the_antenna_time_to_turn_between_passes() {
        // Three passes at one station, 20 s apart, no fixed turnaround. The second rises half a
        // turn from where the first sets (30 s to get there); the third rises close to where
        // the first sets (5 s).
        let windows = [
            pointed(1, 1, 0, 600, 0, 9_000),
            pointed(2, 1, 620, 1_000, 27_000, 0),
            pointed(3, 1, 620, 900, 12_000, 0),
        ];
        let requests = [request(1, 9), request(2, 5), request(3, 1)];

        // Without slew rates the middle-priority pass follows the first; the third overlaps it.
        assert_eq!(
            run(&windows, &requests, &[rule(1, 0)]),
            [OK, OK, lost(ReasonCode::StationBusy, 1)]
        );
        // With them, the antenna cannot reach the second pass in time, and the third, which it
        // can reach, is scheduled instead.
        assert_eq!(
            run(
                &windows,
                &requests,
                &[rule(1, 0).with_slew_rates(rotator())]
            ),
            [OK, lost(ReasonCode::StationTurnaround, 0), OK]
        );
    }

    #[test]
    fn windows_without_pointing_are_scheduled_on_the_fixed_turnaround_alone() {
        let windows = [window(1, 1, 0, 600), window(2, 1, 620, 900)];
        let requests = [request(1, 9), request(2, 1)];
        let slewing = rule(1, 0).with_slew_rates(SlewRates::new(1, 1).unwrap());
        assert_eq!(run(&windows, &requests, &[slewing]), [OK, OK]);
    }

    #[test]
    fn a_rule_for_one_station_leaves_the_others_alone() {
        let windows = [window(1, 2, 0, 600), window(2, 2, 610, 900)];
        let requests = [request(1, 9), request(2, 1)];
        assert_eq!(run(&windows, &requests, &[rule(1, 300)]), [OK, OK]);
    }

    #[test]
    fn an_overlap_is_reported_as_an_overlap_even_when_turnaround_is_set() {
        let windows = [window(1, 1, 0, 600), window(2, 1, 500, 900)];
        let requests = [request(1, 9), request(2, 1)];
        assert_eq!(
            run(&windows, &requests, &[rule(1, 120)]),
            [OK, lost(ReasonCode::StationBusy, 0)]
        );
    }

    // --- Satellite conflicts -------------------------------------------------------------------

    #[test]
    fn a_satellite_visible_from_two_stations_gets_the_longer_pass() {
        let windows = [window(7, 1, 0, 300), window(7, 2, 100, 600)];
        assert_eq!(
            run(&windows, &[request(7, 5)], &[]),
            [lost(ReasonCode::SatelliteBusy, 1), OK]
        );
    }

    #[test]
    fn a_handover_between_stations_keeps_both_passes() {
        let windows = [window(7, 1, 0, 300), window(7, 2, 300, 600)];
        assert_eq!(run(&windows, &[request(7, 5)], &[]), [OK, OK]);
    }

    // --- Contact need --------------------------------------------------------------------------

    #[test]
    fn passes_are_scheduled_until_the_need_is_met() {
        // Need 500 s. Best first: 400 s, then 300 s (total 700, need met), then the rest drop.
        let windows = [
            window(1, 1, 0, 200),
            window(1, 1, 1_000, 1_400),
            window(1, 1, 2_000, 2_300),
            window(1, 1, 3_000, 3_100),
        ];
        let requests = [request(1, 5).with_need(Seconds::new(500))];
        let need_met = (ReasonCode::NeedAlreadyMet, None);
        assert_eq!(run(&windows, &requests, &[]), [need_met, OK, OK, need_met]);
    }

    #[test]
    fn a_need_met_exactly_stops_further_passes() {
        let windows = [window(1, 1, 0, 300), window(1, 1, 1_000, 1_200)];
        let requests = [request(1, 5).with_need(Seconds::new(300))];
        assert_eq!(
            run(&windows, &requests, &[]),
            [OK, (ReasonCode::NeedAlreadyMet, None)]
        );
    }

    #[test]
    fn a_need_of_zero_schedules_nothing() {
        let windows = [window(1, 1, 0, 300)];
        let requests = [request(1, 5).with_need(Seconds::ZERO)];
        assert_eq!(
            run(&windows, &requests, &[]),
            [(ReasonCode::NeedAlreadyMet, None)]
        );
    }

    #[test]
    fn one_satellites_contact_does_not_count_toward_anothers_need() {
        let windows = [window(1, 1, 0, 900), window(2, 2, 0, 100)];
        let requests = [
            request(1, 5).with_need(Seconds::new(100)),
            request(2, 5).with_need(Seconds::new(100)),
        ];
        assert_eq!(run(&windows, &requests, &[]), [OK, OK]);
    }

    #[test]
    fn a_met_need_frees_the_station_for_a_lower_priority() {
        // Satellite 1 has all it needs after its first pass, so its second pass is dropped and
        // the low-priority satellite gets the station instead of being blocked by it.
        let windows = [
            window(1, 1, 0, 600),
            window(1, 1, 1_000, 1_500),
            window(2, 1, 1_100, 1_400),
        ];
        let requests = [request(1, 9).with_need(Seconds::new(600)), request(2, 1)];
        assert_eq!(
            run(&windows, &requests, &[]),
            [OK, (ReasonCode::NeedAlreadyMet, None), OK]
        );
    }

    #[test]
    fn a_met_need_is_reported_ahead_of_a_conflict() {
        // The second pass both overlaps a scheduled pass and is surplus to need. The need is
        // the reason given: freeing the station would not get this pass scheduled.
        let windows = [
            window(1, 1, 0, 600),
            window(1, 2, 2_000, 2_300),
            window(2, 2, 1_900, 2_500),
        ];
        let requests = [request(1, 1).with_need(Seconds::new(600)), request(2, 9)];
        assert_eq!(
            run(&windows, &requests, &[]),
            [OK, (ReasonCode::NeedAlreadyMet, None), OK]
        );
    }

    #[test]
    fn an_unlimited_need_is_never_met() {
        let unlimited = request(1, 1);
        assert!(unlimited.is_unlimited());
        assert!(!unlimited.is_met_by(Seconds::ZERO));
        assert!(!unlimited.is_met_by(Seconds::MAX));

        let limited = request(1, 1).with_need(Seconds::new(600));
        assert!(!limited.is_unlimited());
        assert!(!limited.is_met_by(Seconds::new(599)));
        assert!(limited.is_met_by(Seconds::new(600)));
        assert!(
            request(1, 1)
                .with_need(Seconds::ZERO)
                .is_met_by(Seconds::ZERO)
        );
    }

    #[test]
    fn contact_totals_saturate_instead_of_overflowing() {
        // The second window is also the regression test for an unlimited need: once the running
        // total saturates at `Seconds::MAX` it equals the "no limit" value, and must still not
        // count as a need that has been met.
        // Two windows of the longest possible length: their sum does not fit in a u32.
        let longest = i64::from(u32::MAX);
        let windows = [window(1, 1, 0, longest), window(1, 2, longest, 2 * longest)];
        let mut decisions = [Decision::PENDING; 2];
        let summary = schedule_greedy(&windows, &[request(1, 1)], &[], &mut decisions).unwrap();
        assert_eq!(summary.scheduled, 2);
        assert_eq!(summary.contact, Seconds::MAX);
    }

    // --- Request errors ------------------------------------------------------------------------

    #[test]
    fn an_output_slice_that_is_too_short_is_refused_and_left_untouched() {
        let windows = [window(1, 1, 0, 100), window(1, 1, 200, 300)];
        let mut decisions = [Decision::PENDING; 1];
        let error = schedule_greedy(&windows, &[request(1, 1)], &[], &mut decisions).unwrap_err();
        assert_eq!(
            error,
            ScheduleError::OutputTooSmall {
                needed: 2,
                capacity: 1
            }
        );
        assert_eq!(error.reason(), ReasonCode::CapacityExceeded);
        assert_eq!(decisions, [Decision::PENDING]);
    }

    #[test]
    fn spare_output_slots_are_left_untouched() {
        let mut decisions = [Decision::PENDING; 4];
        schedule_greedy(
            &[window(1, 1, 0, 100)],
            &[request(1, 1)],
            &[],
            &mut decisions,
        )
        .unwrap();
        assert!(decisions[0].is_scheduled());
        assert_eq!(decisions[1..], [Decision::PENDING; 3]);
    }

    #[test]
    fn two_requests_for_one_satellite_are_refused() {
        let requests = [request(1, 1), request(2, 1), request(1, 9)];
        let mut decisions = [Decision::PENDING; 1];
        let error =
            schedule_greedy(&[window(1, 1, 0, 100)], &requests, &[], &mut decisions).unwrap_err();
        assert_eq!(
            error,
            ScheduleError::DuplicateRequest {
                satellite: SatelliteId::new(1)
            }
        );
        assert_eq!(error.reason(), ReasonCode::DuplicateRequest);
        assert_eq!(decisions, [Decision::PENDING]);
    }

    #[test]
    fn two_rules_for_one_station_are_refused() {
        let rules = [rule(1, 10), rule(1, 20)];
        let mut decisions = [Decision::PENDING; 1];
        let error = schedule_greedy(
            &[window(1, 1, 0, 100)],
            &[request(1, 1)],
            &rules,
            &mut decisions,
        )
        .unwrap_err();
        assert_eq!(
            error,
            ScheduleError::DuplicateStationRule {
                station: StationId::new(1)
            }
        );
        assert_eq!(error.reason(), ReasonCode::DuplicateStationRule);
    }

    #[test]
    fn error_messages_lead_with_the_code_and_name_the_numbers() {
        let too_small = ScheduleError::OutputTooSmall {
            needed: 40,
            capacity: 32,
        }
        .to_string();
        assert!(too_small.starts_with("MAAT-301: "), "{too_small}");
        assert!(too_small.contains("40") && too_small.contains("32"));

        let request = ScheduleError::DuplicateRequest {
            satellite: SatelliteId::new(25544),
        }
        .to_string();
        assert!(
            request.starts_with("MAAT-204: ") && request.contains("25544"),
            "{request}"
        );

        let station = ScheduleError::DuplicateStationRule {
            station: StationId::new(7),
        }
        .to_string();
        assert!(
            station.starts_with("MAAT-205: ") && station.contains("station 7"),
            "{station}"
        );
    }

    // --- Decision ------------------------------------------------------------------------------

    #[test]
    fn a_pending_decision_has_no_reason_and_is_not_scheduled() {
        assert_eq!(Decision::PENDING.reason(), None);
        assert!(!Decision::PENDING.is_scheduled());
        assert_eq!(Decision::PENDING.blocked_by(), None);
    }

    #[test]
    fn request_and_rule_builders_keep_their_values() {
        let r = request(5, 7)
            .with_min_contact(Seconds::new(60))
            .with_need(Seconds::new(900));
        assert_eq!(r.satellite(), SatelliteId::new(5));
        assert_eq!(r.priority(), Priority::new(7));
        assert_eq!(r.min_contact(), Seconds::new(60));
        assert_eq!(r.need(), Seconds::new(900));
        // Defaults: any pass is long enough and there is no limit.
        assert_eq!(request(5, 7).min_contact(), Seconds::ZERO);
        assert_eq!(request(5, 7).need(), Seconds::MAX);

        let s = rule(3, 45);
        assert_eq!(s.station(), StationId::new(3));
        assert_eq!(s.turnaround(), Seconds::new(45));
    }

    // --- Randomized invariants -----------------------------------------------------------------

    /// A small deterministic pseudo-random generator, so the test needs no dependency and
    /// produces the same instances on every run and every machine.
    struct Lcg(u64);

    impl Lcg {
        fn next(&mut self) -> u64 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            self.0 >> 33
        }

        fn below(&mut self, n: u64) -> u64 {
            self.next() % n
        }
    }

    struct Instance {
        windows: Vec<ContactWindow>,
        requests: Vec<SatelliteRequest>,
        rules: Vec<StationRule>,
    }

    /// A crowded random problem: few stations and a short horizon, so conflicts are common.
    fn random_instance(rng: &mut Lcg) -> Instance {
        let satellites = 2 + rng.below(5); // 2..=6, plus one satellite with no request
        let stations = 1 + rng.below(3); // 1..=3
        let count = 5 + rng.below(40); // 5..=44 windows
        let windows = (0..count)
            .map(|_| {
                let aos = i64::try_from(rng.below(6_000)).unwrap();
                let length = i64::try_from(30 + rng.below(600)).unwrap();
                let plain = window(
                    u32::try_from(1 + rng.below(satellites + 1)).unwrap(),
                    u16::try_from(1 + rng.below(stations)).unwrap(),
                    aos,
                    aos + length,
                );
                // Most windows record where the antenna points; a few do not.
                if rng.below(5) == 0 {
                    plain
                } else {
                    let mut direction = || u16::try_from(rng.below(36_000)).unwrap();
                    let (rise, set) = (direction(), direction());
                    plain.with_pointing(at(rise, 1_000), at(set, 1_000))
                }
            })
            .collect();
        let requests = (1..=satellites)
            .map(|satellite| {
                let mut r = request(
                    u32::try_from(satellite).unwrap(),
                    u8::try_from(rng.below(4)).unwrap(), // few levels, so ties are common
                );
                if rng.below(2) == 0 {
                    r = r.with_min_contact(Seconds::new(u32::try_from(rng.below(200)).unwrap()));
                }
                if rng.below(2) == 0 {
                    r = r.with_need(Seconds::new(u32::try_from(rng.below(2_000)).unwrap()));
                }
                r
            })
            .collect();
        let mut rules = Vec::new();
        for station in 1..=stations {
            if rng.below(2) == 0 {
                let mut station_rule = rule(
                    u16::try_from(station).unwrap(),
                    u32::try_from(rng.below(120)).unwrap(),
                );
                // Half the rules also have a slow mount: 1 to 4 degrees per second.
                if rng.below(2) == 0 {
                    let rate = u16::try_from(100 + rng.below(300)).unwrap();
                    station_rule =
                        station_rule.with_slew_rates(SlewRates::new(rate, rate).unwrap());
                }
                rules.push(station_rule);
            }
        }
        Instance {
            windows,
            requests,
            rules,
        }
    }

    fn granted(windows: &[ContactWindow], decisions: &[Decision], satellite: SatelliteId) -> u64 {
        windows
            .iter()
            .zip(decisions)
            .filter(|(w, d)| d.is_scheduled() && w.satellite() == satellite)
            .map(|(w, _)| u64::from(w.duration().get()))
            .sum()
    }

    /// Checks a finished schedule against the rules, written independently of the scheduler:
    /// the plan is free of conflicts, and every stated reason is true.
    fn assert_schedule_is_sound(instance: &Instance, decisions: &[Decision]) {
        let Instance {
            windows,
            requests,
            rules,
        } = instance;
        let priority_of = |w: &ContactWindow| {
            requests
                .iter()
                .find(|r| r.satellite() == w.satellite())
                .map(SatelliteRequest::priority)
        };

        // 1. No two scheduled windows conflict.
        for (i, a) in windows.iter().enumerate() {
            for (j, b) in windows.iter().enumerate().skip(i + 1) {
                if decisions[i].is_scheduled() && decisions[j].is_scheduled() {
                    let clash = conflict(a, b, &rule_for(rules, a.station()));
                    assert_eq!(clash, None, "scheduled windows {i} and {j} conflict");
                }
            }
        }

        // 2. Every reason is true.
        for (i, (w, decision)) in windows.iter().zip(decisions).enumerate() {
            let reason = decision.reason().expect("every window is decided");
            let request = requests.iter().find(|r| r.satellite() == w.satellite());
            let rule = rule_for(rules, w.station());
            let blocker = decision.blocked_by().map(|b| {
                assert!(
                    decisions[b].is_scheduled(),
                    "window {i} is blocked by unscheduled {b}"
                );
                assert_ne!(b, i, "window {i} is blocked by itself");
                &windows[b]
            });

            match reason {
                ReasonCode::NoRequest => assert!(request.is_none(), "window {i}"),
                ReasonCode::BelowMinimumDuration => {
                    assert!(w.duration() < request.unwrap().min_contact(), "window {i}");
                }
                ReasonCode::NeedAlreadyMet => {
                    assert!(!request.unwrap().is_unlimited(), "window {i}");
                    let need = u64::from(request.unwrap().need().get());
                    assert!(
                        granted(windows, decisions, w.satellite()) >= need,
                        "window {i}"
                    );
                }
                ReasonCode::StationBusy | ReasonCode::StationTieLost => {
                    let b = blocker.expect("a station conflict names its blocker");
                    assert_eq!(conflict(w, b, &rule), Some(Conflict::StationOverlap));
                    let (mine, theirs) = (priority_of(w).unwrap(), priority_of(b).unwrap());
                    if reason == ReasonCode::StationBusy {
                        assert!(
                            theirs > mine,
                            "window {i}: MAAT-101 needs a higher priority"
                        );
                    } else {
                        assert_eq!(theirs, mine, "window {i}: MAAT-107 needs equal priority");
                        assert_eq!(rank(theirs, b, mine, w), Ordering::Less, "window {i}");
                    }
                }
                ReasonCode::StationTurnaround => {
                    let b = blocker.expect("a turnaround conflict names its blocker");
                    assert_eq!(conflict(w, b, &rule), Some(Conflict::StationTurnaround));
                }
                ReasonCode::SatelliteBusy => {
                    let b = blocker.expect("a satellite conflict names its blocker");
                    assert_eq!(conflict(w, b, &rule), Some(Conflict::SatelliteOverlap));
                }
                ReasonCode::Scheduled => {
                    assert!(blocker.is_none());
                    let r = request.expect("a scheduled window has a request");
                    assert!(w.duration() >= r.min_contact(), "window {i}");
                    // It was wanted when it was taken: the passes that outrank it leave the
                    // need unmet.
                    let before: u64 = windows
                        .iter()
                        .zip(decisions)
                        .filter(|(o, d)| {
                            d.is_scheduled()
                                && o.satellite() == w.satellite()
                                && rank(r.priority(), o, r.priority(), w) == Ordering::Less
                        })
                        .map(|(o, _)| u64::from(o.duration().get()))
                        .sum();
                    assert!(before < u64::from(r.need().get()), "window {i} was surplus");
                }
                other => panic!("window {i} got {other:?}, which the scheduler never emits"),
            }

            // 3. Nothing was left on the table: a window turned down for a conflict really is
            //    blocked, which the checks above establish, and one turned down for any other
            //    reason could not have been scheduled either.
            if !decision.is_scheduled() && decision.blocked_by().is_none() {
                assert!(
                    matches!(
                        reason,
                        ReasonCode::NoRequest
                            | ReasonCode::BelowMinimumDuration
                            | ReasonCode::NeedAlreadyMet
                    ),
                    "window {i}: {reason:?} without a blocker"
                );
            }
        }
    }

    #[test]
    fn random_schedules_are_conflict_free_and_every_reason_is_true() {
        let mut rng = Lcg(0x4d61_6174); // "Maat"
        let mut seen = std::collections::BTreeMap::new();
        for _ in 0..400 {
            let instance = random_instance(&mut rng);
            let mut decisions = vec![Decision::PENDING; instance.windows.len()];
            let summary = schedule_greedy(
                &instance.windows,
                &instance.requests,
                &instance.rules,
                &mut decisions,
            )
            .unwrap();
            assert_schedule_is_sound(&instance, &decisions);

            assert_eq!(summary.scheduled + summary.rejected, instance.windows.len());
            assert_eq!(
                summary.scheduled,
                decisions.iter().filter(|d| d.is_scheduled()).count()
            );
            for decision in &decisions {
                *seen
                    .entry(decision.reason().unwrap().code())
                    .or_insert(0_u32) += 1;
            }
        }
        // The instances must actually exercise every outcome, or the test proves little.
        for code in [1, 101, 102, 103, 104, 106, 107, 108] {
            assert!(
                seen.get(&code).copied().unwrap_or(0) >= 20,
                "MAAT-{code:03} came up only {:?} times: {seen:?}",
                seen.get(&code)
            );
        }
    }

    #[test]
    fn the_plan_does_not_depend_on_the_order_of_the_input() {
        // Shuffle the windows, schedule again, and compare the set of scheduled windows.
        // Exact duplicates are removed first: which copy of two identical windows wins is
        // decided by input position, by design.
        let mut rng = Lcg(0x6f72_6465); // "orde"
        for _ in 0..200 {
            let mut instance = random_instance(&mut rng);
            let mut unique: Vec<ContactWindow> = Vec::new();
            for w in &instance.windows {
                if !unique.contains(w) {
                    unique.push(*w);
                }
            }
            instance.windows = unique;

            let schedule = |windows: &[ContactWindow]| {
                let mut decisions = vec![Decision::PENDING; windows.len()];
                schedule_greedy(windows, &instance.requests, &instance.rules, &mut decisions)
                    .unwrap();
                let mut scheduled: Vec<_> = windows
                    .iter()
                    .zip(&decisions)
                    .filter(|(_, d)| d.is_scheduled())
                    .map(|(w, _)| format!("{w:?}"))
                    .collect();
                scheduled.sort();
                scheduled
            };

            let mut shuffled = instance.windows.clone();
            for i in (1..shuffled.len()).rev() {
                let j = usize::try_from(rng.below(u64::try_from(i + 1).unwrap())).unwrap();
                shuffled.swap(i, j);
            }
            assert_eq!(schedule(&instance.windows), schedule(&shuffled));
        }
    }

    #[test]
    fn scheduling_twice_gives_the_same_decisions() {
        let mut rng = Lcg(7);
        let instance = random_instance(&mut rng);
        let run_once = || {
            let mut decisions = vec![Decision::PENDING; instance.windows.len()];
            schedule_greedy(
                &instance.windows,
                &instance.requests,
                &instance.rules,
                &mut decisions,
            )
            .unwrap();
            decisions
        };
        assert_eq!(run_once(), run_once());
    }
}
