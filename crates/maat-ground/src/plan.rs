//! The contact plan: every pass that was found, with the decision on it and the reason.
//!
//! A [`ContactPlan`] is what Ma'at exists to produce. It joins the two stages that decide a
//! pass's fate:
//!
//! - the **survey** ([`contact_windows`](crate::contact_windows)), which can turn a pass down
//!   before scheduling, for example because the antenna cannot follow it;
//! - the **scheduler** (greedy or optimal), which chooses among the usable windows.
//!
//! Every pass appears exactly once, as a [`PlanEntry`] with a stable `MAAT-xxx` reason code and
//! a sentence of detail that names the satellites, stations, times and numbers involved. A plan
//! can be printed for a person ([`ContactPlan::to_text`]) or written as JSON for a program
//! ([`ContactPlan::to_json`]).

use std::fmt::Write as _;
use std::time::Duration;

use chrono::{DateTime, Utc};
use maat_flight::schedule::{Conflict, conflict, rank, rule_for, schedule_greedy};
use maat_flight::{
    ContactWindow, Decision, Pointing, Priority, ReasonCode, SatelliteId, SatelliteRequest,
    Seconds, SlewRates, StationId, StationRule,
};
use serde_json::{Value, json};

use crate::error::GroundError;
use crate::optimal::{Verdict, plan_value, schedule_optimal};
use crate::satellite::Satellite;
use crate::station::GroundStation;
use crate::survey::WindowSurvey;
use crate::time::utc_from_timestamp;

/// Which scheduler builds the plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Scheduler {
    /// Best window first, never reconsidered. Small, predictable and bounded; the same code runs
    /// onboard. Priority is a strict rank. See [`maat_flight::schedule`].
    Greedy,
    /// The plan of greatest total value, found by integer programming. Ground only. Priority is
    /// a weight. The search starts from the greedy plan and stops at a time limit, so the result
    /// is never worth less than the greedy plan, and says whether it is proven best.
    Optimal,
}

impl Scheduler {
    /// The scheduler's name in lower case: `"greedy"` or `"optimal"`.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Greedy => "greedy",
            Self::Optimal => "optimal",
        }
    }
}

/// How long the optimal scheduler searches when no limit is given: one minute.
///
/// Problems of a few thousand windows finish in well under a second, so this is reached only by
/// a problem that is unusually hard. When it is reached, the plan is the best found so far.
pub const DEFAULT_OPTIMAL_TIME_LIMIT: Duration = Duration::from_secs(60);

/// How a plan should be built.
///
/// ```
/// use std::time::Duration;
/// use maat_ground::{PlanOptions, Scheduler};
///
/// let options = PlanOptions::new(Scheduler::Optimal)
///     .with_optimal_time_limit(Duration::from_secs(5));
/// assert_eq!(options.optimal_time_limit, Duration::from_secs(5));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct PlanOptions {
    /// Which scheduler builds the plan.
    pub scheduler: Scheduler,
    /// How long the optimal scheduler may search. Ignored by the greedy scheduler, whose run
    /// time is bounded by the size of the problem.
    pub optimal_time_limit: Duration,
    /// Whether the optimal scheduler must give every satellite at least the useful contact the
    /// greedy plan gives it. Off by default. Ignored by the greedy scheduler.
    pub satellite_floor: bool,
}

impl PlanOptions {
    /// Options for the given scheduler, with [`DEFAULT_OPTIMAL_TIME_LIMIT`].
    #[must_use]
    pub fn new(scheduler: Scheduler) -> Self {
        Self {
            scheduler,
            optimal_time_limit: DEFAULT_OPTIMAL_TIME_LIMIT,
            satellite_floor: false,
        }
    }

    /// Sets whether the optimal scheduler keeps a floor under every satellite.
    ///
    /// Without a floor the optimal plan is worth at least as much as the greedy plan **in
    /// total**, but a satellite can get less than the greedy plan would give it: the solver
    /// moves contact to wherever it adds most, and between two satellites of equal priority it
    /// has no preference. With a floor, no satellite gets less useful contact (contact up to
    /// its need) than under the greedy plan, and the optimal plan improves on it only where
    /// nobody pays. The total gain is smaller or the same.
    ///
    /// ```
    /// use maat_ground::{PlanOptions, Scheduler};
    ///
    /// let options = PlanOptions::new(Scheduler::Optimal).with_satellite_floor(true);
    /// assert!(options.satellite_floor);
    /// ```
    #[must_use]
    pub fn with_satellite_floor(mut self, floor: bool) -> Self {
        self.satellite_floor = floor;
        self
    }

    /// Sets how long the optimal scheduler may search.
    ///
    /// The right limit is how long you can wait for the plan. A plan is needed before its first
    /// pass begins, so [`time_until_first_pass`] gives the longest limit that makes sense.
    #[must_use]
    pub fn with_optimal_time_limit(mut self, limit: Duration) -> Self {
        self.optimal_time_limit = limit;
        self
    }
}

/// How long there is from `now` until the first surveyed window begins.
///
/// A plan is of no use for a pass that has already started, so this is the longest the optimal
/// scheduler can usefully be given. `None` if the survey found no windows, or if the first one
/// has already begun.
///
/// ```
/// use chrono::Duration;
/// use maat_ground::flight::StationId;
/// use maat_ground::{
///     GroundStation, PlanOptions, Scheduler, StationSite, contact_windows, satellites_from_omm,
///     time_until_first_pass,
/// };
///
/// let satellites = satellites_from_omm(
///     r#"{"OBJECT_NAME":"EXAMPLE-A","NORAD_CAT_ID":65001,"EPOCH":"2026-08-01T12:00:00",
///         "MEAN_MOTION":15.49,"ECCENTRICITY":0.0002,"INCLINATION":51.64,"RA_OF_ASC_NODE":221.0,
///         "ARG_OF_PERICENTER":89.0,"MEAN_ANOMALY":280.0,"BSTAR":0.00003}"#,
/// )?;
/// let stations = [GroundStation::new(
///     StationId::new(1),
///     "Seattle",
///     StationSite { latitude_deg: 47.61, longitude_deg: -122.33, altitude_m: 0.0 },
///     10.0,
/// )?];
/// let start = satellites[0].epoch();
/// let survey = contact_windows(&satellites, &stations, start, start + Duration::days(1))?;
///
/// // Planning an hour before the search window opens: the plan must be ready before the first
/// // pass, and the optimal scheduler is told to stop a minute before that at the latest.
/// let now = start - Duration::hours(1);
/// let available = time_until_first_pass(&survey, now).expect("there are passes ahead");
/// let limit = available.saturating_sub(std::time::Duration::from_secs(60));
/// let options = PlanOptions::new(Scheduler::Optimal).with_optimal_time_limit(limit);
/// assert!(options.optimal_time_limit >= std::time::Duration::from_secs(59 * 60));
/// # Ok::<(), maat_ground::GroundError>(())
/// ```
#[must_use]
pub fn time_until_first_pass(survey: &WindowSurvey, now: DateTime<Utc>) -> Option<Duration> {
    let first = survey.windows.iter().map(ContactWindow::aos).min()?;
    let first = utc_from_timestamp(first).ok()?;
    // `to_std` fails for a negative duration, which is a first pass already under way.
    (first - now).to_std().ok().filter(|wait| !wait.is_zero())
}

/// Whether an optimal plan is proven to be the best, and the limit it was searched under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Optimality {
    /// `true` if the solver proved that no plan is worth more. `false` if it reached its time
    /// limit first: the plan is then the best it had found, and worth at least as much as the
    /// greedy plan.
    pub proven: bool,
    /// The time limit the solver was given.
    pub time_limit: Duration,
    /// `true` if the plan was built with a floor under every satellite
    /// ([`PlanOptions::with_satellite_floor`]). "Proven" then means that no plan respecting the
    /// floor is worth more.
    pub satellite_floor: bool,
}

/// Which stage decided a pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum DecidedBy {
    /// The window survey turned the pass down before scheduling.
    Survey,
    /// The scheduler accepted the pass or turned it down.
    Scheduler,
}

impl DecidedBy {
    /// The stage's name in lower case: `"survey"` or `"scheduler"`.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Survey => "survey",
            Self::Scheduler => "scheduler",
        }
    }
}

/// One pass and the decision on it.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct PlanEntry {
    /// The satellite.
    pub satellite: SatelliteId,
    /// The satellite's name.
    pub satellite_name: String,
    /// The ground station.
    pub station: StationId,
    /// The station's name.
    pub station_name: String,
    /// Acquisition of signal. Whole seconds for a pass that reached the scheduler; as predicted
    /// for one the survey turned down.
    pub aos: DateTime<Utc>,
    /// Loss of signal.
    pub los: DateTime<Utc>,
    /// The highest elevation of the pass, in degrees.
    pub max_elevation_deg: f64,
    /// The satellite's priority, if a request names it.
    pub priority: Option<Priority>,
    /// Which stage made the decision.
    pub decided_by: DecidedBy,
    /// The stable reason code. [`ReasonCode::Scheduled`] means the pass is in the plan.
    pub reason: ReasonCode,
    /// The specifics behind the reason: who, where, when and how much.
    pub detail: String,
    /// For a pass that lost to another, the position of that other pass in
    /// [`ContactPlan::entries`].
    pub blocked_by: Option<usize>,
    /// The flight contact window, for a pass that reached the scheduler.
    pub window: Option<ContactWindow>,
}

impl PlanEntry {
    /// `true` if the pass is in the plan.
    #[must_use]
    pub fn is_scheduled(&self) -> bool {
        self.reason == ReasonCode::Scheduled
    }

    /// The length of the pass in seconds.
    #[must_use]
    pub fn duration_seconds(&self) -> f64 {
        (self.los - self.aos).num_milliseconds() as f64 / 1000.0
    }

    /// The full explanation on one line: the code, its summary and the detail.
    ///
    /// ```text
    /// MAAT-101: dropped: overlaps a higher-priority pass at the same station (EXAMPLE-B, ...)
    /// ```
    #[must_use]
    pub fn explanation(&self) -> String {
        format!("{} ({})", self.reason, self.detail)
    }
}

/// What one satellite asked for and what it got.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct SatelliteSummary {
    /// The satellite.
    pub satellite: SatelliteId,
    /// The satellite's name.
    pub name: String,
    /// Its contact request, if it has one.
    pub request: Option<SatelliteRequest>,
    /// How many passes were found for it, usable or not.
    pub passes_found: usize,
    /// How many of them are in the plan.
    pub passes_scheduled: usize,
    /// Total scheduled contact time.
    pub contact_scheduled: Seconds,
    /// In a plan built with a floor: the useful contact the plan was required to give this
    /// satellite, which is what the greedy plan gives it. `None` otherwise, and for a satellite
    /// with no request.
    pub floor: Option<Seconds>,
}

impl SatelliteSummary {
    /// Whether the satellite's need is met: `Some(true)` or `Some(false)` for a satellite that
    /// asked for a definite amount, `None` for one with no request or no limit.
    #[must_use]
    pub fn need_met(&self) -> Option<bool> {
        let request = self.request.as_ref()?;
        if request.is_unlimited() {
            None
        } else {
            Some(request.is_met_by(self.contact_scheduled))
        }
    }
}

/// How busy one ground station is under the plan.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct StationSummary {
    /// The station.
    pub station: StationId,
    /// The station's name.
    pub name: String,
    /// The fixed time it needs between passes.
    pub turnaround: Seconds,
    /// How fast it turns its antenna, if it has slew limits. The schedulers add the time to
    /// turn from the end of one pass to the start of the next to the turnaround.
    pub slew_rates: Option<SlewRates>,
    /// How many passes were found over it, usable or not.
    pub passes_found: usize,
    /// How many of them are in the plan.
    pub passes_scheduled: usize,
    /// Total scheduled contact time.
    pub contact_scheduled: Seconds,
}

/// A finished contact plan. See the [module docs](self).
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct ContactPlan {
    /// The scheduler that built it.
    pub scheduler: Scheduler,
    /// For a plan from the optimal scheduler, whether it is proven best. `None` for a greedy
    /// plan.
    pub optimality: Option<Optimality>,
    /// Every pass that was found, in order of AOS, then station and satellite.
    pub entries: Vec<PlanEntry>,
    /// One summary per satellite, in order of satellite number.
    pub satellites: Vec<SatelliteSummary>,
    /// One summary per station, in order of station number.
    pub stations: Vec<StationSummary>,
    /// Satellite/station pairs the survey could not search. When this is not empty the plan is
    /// built from incomplete information and says so wherever it is printed.
    pub failures: Vec<String>,
    /// The plan's value: for each satellite, `priority + 1` times its useful contact seconds
    /// (contact up to its need), added up. This is what the optimal scheduler maximizes, and it
    /// is computed the same way for a greedy plan so that the two can be compared.
    pub value: f64,
}

impl ContactPlan {
    /// The passes that are in the plan, in order of AOS.
    pub fn scheduled(&self) -> impl Iterator<Item = &PlanEntry> {
        self.entries.iter().filter(|entry| entry.is_scheduled())
    }

    /// The passes that are not in the plan, in order of AOS.
    pub fn not_scheduled(&self) -> impl Iterator<Item = &PlanEntry> {
        self.entries.iter().filter(|entry| !entry.is_scheduled())
    }

    /// Total scheduled contact time over all satellites.
    #[must_use]
    pub fn scheduled_contact(&self) -> Seconds {
        self.satellites
            .iter()
            .fold(Seconds::ZERO, |total, satellite| {
                total.saturating_add(satellite.contact_scheduled)
            })
    }

    /// `true` if the survey behind the plan searched every satellite/station pair.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.failures.is_empty()
    }
}

/// Builds a contact plan from a window survey.
///
/// `satellites` and `stations` supply names and each station's turnaround time; they should be
/// the lists the survey was made from. `requests` says what each satellite is asking for: a
/// satellite without a request is not scheduled (`MAAT-106`).
///
/// # Errors
///
/// - [`GroundError::Schedule`] if two requests name one satellite (`MAAT-204`).
/// - [`GroundError::Solver`] if the optimal scheduler's solver fails.
/// - [`GroundError::TimeOutOfRange`] if a window's time cannot be shown as a calendar date.
///
/// # Example
///
/// ```
/// use chrono::Duration;
/// use maat_ground::flight::{Priority, ReasonCode, SatelliteRequest, Seconds, StationId};
/// use maat_ground::{
///     GroundStation, Scheduler, StationSite, contact_windows, plan_contacts, satellites_from_omm,
/// };
///
/// let satellites = satellites_from_omm(
///     r#"[{"OBJECT_NAME":"EXAMPLE-A","NORAD_CAT_ID":65001,"EPOCH":"2026-08-01T12:00:00",
///          "MEAN_MOTION":15.49,"ECCENTRICITY":0.0002,"INCLINATION":51.64,"RA_OF_ASC_NODE":221.0,
///          "ARG_OF_PERICENTER":89.0,"MEAN_ANOMALY":280.0,"BSTAR":0.00003},
///         {"OBJECT_NAME":"EXAMPLE-B","NORAD_CAT_ID":100123,"EPOCH":"2026-08-01T12:00:00",
///          "MEAN_MOTION":15.2,"ECCENTRICITY":0.0012,"INCLINATION":97.5,"RA_OF_ASC_NODE":120.0,
///          "ARG_OF_PERICENTER":45.0,"MEAN_ANOMALY":315.0,"BSTAR":0.0001}]"#,
/// )?;
/// let stations = [GroundStation::new(
///     StationId::new(1),
///     "Seattle",
///     StationSite { latitude_deg: 47.61, longitude_deg: -122.33, altitude_m: 0.0 },
///     10.0,
/// )?];
/// let start = satellites[0].epoch();
/// let survey = contact_windows(&satellites, &stations, start, start + Duration::days(2))?;
///
/// // EXAMPLE-A wants ten minutes in total; EXAMPLE-B wants everything, at lower priority.
/// let requests = [
///     SatelliteRequest::new(satellites[0].id(), Priority::new(9)).with_need(Seconds::new(600)),
///     SatelliteRequest::new(satellites[1].id(), Priority::new(1)),
/// ];
/// let plan = plan_contacts(&survey, &satellites, &stations, &requests, Scheduler::Greedy)?;
///
/// // Every pass is accounted for, and every one that is not scheduled says why.
/// assert_eq!(plan.entries.len(), survey.windows.len() + survey.rejected.len());
/// assert!(plan.scheduled().count() > 0);
/// for entry in plan.not_scheduled() {
///     assert_ne!(entry.reason, ReasonCode::Scheduled);
///     assert!(!entry.detail.is_empty());
/// }
/// assert_eq!(plan.satellites[0].need_met(), Some(true));
/// # Ok::<(), maat_ground::GroundError>(())
/// ```
pub fn plan_contacts(
    survey: &WindowSurvey,
    satellites: &[Satellite],
    stations: &[GroundStation],
    requests: &[SatelliteRequest],
    scheduler: Scheduler,
) -> Result<ContactPlan, GroundError> {
    plan_contacts_with(
        survey,
        satellites,
        stations,
        requests,
        &PlanOptions::new(scheduler),
    )
}

/// Builds a contact plan as [`plan_contacts`] does, with every option set by the caller.
///
/// Use this to give the optimal scheduler a time limit other than
/// [`DEFAULT_OPTIMAL_TIME_LIMIT`].
///
/// # Errors
///
/// As for [`plan_contacts`]. Reaching the time limit is not an error: the plan returned is the
/// best found, and [`ContactPlan::optimality`] says it is not proven.
pub fn plan_contacts_with(
    survey: &WindowSurvey,
    satellites: &[Satellite],
    stations: &[GroundStation],
    requests: &[SatelliteRequest],
    options: &PlanOptions,
) -> Result<ContactPlan, GroundError> {
    let scheduler = options.scheduler;
    let windows = &survey.windows;
    let rules: Vec<StationRule> = stations.iter().map(GroundStation::rule).collect();
    let mut floors: Option<Vec<u64>> = None;
    let (verdicts, optimality) = match scheduler {
        Scheduler::Greedy => (greedy_verdicts(windows, requests, &rules)?, None),
        Scheduler::Optimal => {
            let outcome = schedule_optimal(
                windows,
                requests,
                &rules,
                options.optimal_time_limit,
                options.satellite_floor,
            )?;
            let optimality = Optimality {
                proven: outcome.proven_optimal,
                time_limit: options.optimal_time_limit,
                satellite_floor: options.satellite_floor,
            };
            floors = outcome.floors;
            (outcome.verdicts, Some(optimality))
        }
    };

    let names = Names {
        satellites,
        stations,
    };
    let context = Context {
        windows,
        verdicts: &verdicts,
        requests,
        rules: &rules,
        names: &names,
        satellite_floor: floors.is_some(),
    };

    // One entry per window, then one per pass the survey turned down. `blocked_by` still refers
    // to positions in `windows` here; it is translated after sorting.
    let mut entries = Vec::with_capacity(windows.len() + survey.rejected.len());
    for (index, window) in windows.iter().enumerate() {
        entries.push(PlanEntry {
            satellite: window.satellite(),
            satellite_name: names.satellite(window.satellite()),
            station: window.station(),
            station_name: names.station(window.station()),
            aos: utc_from_timestamp(window.aos())?,
            los: utc_from_timestamp(window.los())?,
            max_elevation_deg: f64::from(window.max_elevation().centidegrees()) / 100.0,
            priority: priority_of(requests, window.satellite()),
            decided_by: DecidedBy::Scheduler,
            reason: verdicts[index].reason,
            detail: context.detail(index)?,
            blocked_by: verdicts[index].blocked_by,
            window: Some(*window),
        });
    }
    for rejected in &survey.rejected {
        entries.push(PlanEntry {
            satellite: rejected.satellite,
            satellite_name: names.satellite(rejected.satellite),
            station: rejected.station,
            station_name: names.station(rejected.station),
            aos: rejected.aos,
            los: rejected.los,
            max_elevation_deg: rejected.max_elevation_deg,
            priority: priority_of(requests, rejected.satellite),
            decided_by: DecidedBy::Survey,
            reason: rejected.reason,
            detail: rejected.detail.clone(),
            blocked_by: None,
            window: None,
        });
    }

    // Sort by time, then translate each `blocked_by` from a window position to an entry position.
    let mut order: Vec<usize> = (0..entries.len()).collect();
    order.sort_by_key(|&index| {
        let entry = &entries[index];
        (entry.aos, entry.station, entry.satellite, index)
    });
    let mut position_of = vec![0; entries.len()];
    for (position, &index) in order.iter().enumerate() {
        position_of[index] = position;
    }
    let mut slots: Vec<Option<PlanEntry>> = entries.into_iter().map(Some).collect();
    let entries: Vec<PlanEntry> = order
        .iter()
        .map(|&index| {
            let mut entry = slots[index].take().expect("each entry is moved once");
            entry.blocked_by = entry.blocked_by.map(|blocker| position_of[blocker]);
            entry
        })
        .collect();

    let scheduled: Vec<bool> = verdicts.iter().map(Verdict::is_scheduled).collect();
    Ok(ContactPlan {
        scheduler,
        optimality,
        satellites: satellite_summaries(&entries, requests, &names, floors.as_deref()),
        stations: station_summaries(&entries, stations, &rules, &names),
        failures: survey
            .failures
            .iter()
            .map(|failure| failure.error.to_string())
            .collect(),
        value: plan_value(windows, &scheduled, requests),
        entries,
    })
}

/// Runs the flight scheduler and converts its decisions.
fn greedy_verdicts(
    windows: &[ContactWindow],
    requests: &[SatelliteRequest],
    rules: &[StationRule],
) -> Result<Vec<Verdict>, GroundError> {
    let mut decisions = vec![Decision::PENDING; windows.len()];
    schedule_greedy(windows, requests, rules, &mut decisions)?;
    Ok(decisions
        .iter()
        .map(|decision| Verdict {
            reason: decision
                .reason()
                .expect("the scheduler decides every window"),
            blocked_by: decision.blocked_by(),
        })
        .collect())
}

fn priority_of(requests: &[SatelliteRequest], satellite: SatelliteId) -> Option<Priority> {
    find_request(requests, satellite).map(SatelliteRequest::priority)
}

fn find_request(
    requests: &[SatelliteRequest],
    satellite: SatelliteId,
) -> Option<&SatelliteRequest> {
    requests
        .iter()
        .find(|request| request.satellite() == satellite)
}

/// Looks up display names, falling back to the number for anything not in the lists.
struct Names<'a> {
    satellites: &'a [Satellite],
    stations: &'a [GroundStation],
}

impl Names<'_> {
    fn satellite(&self, id: SatelliteId) -> String {
        self.satellites
            .iter()
            .find(|satellite| satellite.id() == id)
            .map_or_else(|| format!("satellite {id}"), |s| s.name().to_owned())
    }

    fn station(&self, id: StationId) -> String {
        self.stations
            .iter()
            .find(|station| station.id() == id)
            .map_or_else(|| format!("station {id}"), |s| s.name().to_owned())
    }
}

/// Everything needed to write the detail sentence for a window.
struct Context<'a> {
    windows: &'a [ContactWindow],
    verdicts: &'a [Verdict],
    requests: &'a [SatelliteRequest],
    rules: &'a [StationRule],
    names: &'a Names<'a>,
    /// Whether the plan was built with a floor under every satellite.
    satellite_floor: bool,
}

/// The format for times inside detail sentences.
const TIME: &str = "%Y-%m-%d %H:%M:%S";

impl Context<'_> {
    /// "from 2026-08-01 12:00:00 to 2026-08-01 12:09:30"
    fn span(window: &ContactWindow) -> Result<String, GroundError> {
        Ok(format!(
            "from {} to {}",
            utc_from_timestamp(window.aos())?.format(TIME),
            utc_from_timestamp(window.los())?.format(TIME)
        ))
    }

    fn granted(&self, satellite: SatelliteId) -> u64 {
        self.windows
            .iter()
            .zip(self.verdicts)
            .filter(|(window, verdict)| verdict.is_scheduled() && window.satellite() == satellite)
            .map(|(window, _)| u64::from(window.duration().get()))
            .sum()
    }

    /// The detail sentence for the window at `index`.
    fn detail(&self, index: usize) -> Result<String, GroundError> {
        let window = &self.windows[index];
        let verdict = &self.verdicts[index];
        let satellite = self.names.satellite(window.satellite());
        let station = self.names.station(window.station());
        let request = find_request(self.requests, window.satellite());
        let seconds = window.duration().get();
        let blocker = verdict.blocked_by.map(|blocker| &self.windows[blocker]);

        let detail = match (verdict.reason, request, blocker) {
            (ReasonCode::Scheduled, Some(request), _) => {
                let mut text = format!("priority {}, {seconds} s of contact", request.priority());
                if !request.is_unlimited() {
                    let _ = write!(
                        text,
                        "; {satellite} gets {} s in all of the {} s it asked for",
                        self.granted(window.satellite()),
                        request.need().get()
                    );
                }
                text
            }
            (ReasonCode::NoRequest, _, _) => format!(
                "no contact request names {satellite} (catalog number {})",
                window.satellite()
            ),
            (ReasonCode::BelowMinimumDuration, Some(request), _) => format!(
                "the pass lasts {seconds} s and {satellite} asks for passes of at least {} s",
                request.min_contact().get()
            ),
            (ReasonCode::NeedAlreadyMet, Some(request), _) => format!(
                "{satellite} asked for {} s of contact and its scheduled passes give it {} s",
                request.need().get(),
                self.granted(window.satellite())
            ),
            (ReasonCode::StationBusy, Some(request), Some(other)) => format!(
                "{} (priority {}) has {station} {}; this pass has priority {}",
                self.names.satellite(other.satellite()),
                priority_text(self.requests, other.satellite()),
                Self::span(other)?,
                request.priority()
            ),
            (ReasonCode::StationTieLost, Some(request), Some(other)) => format!(
                "{} has the same priority ({}) and has {station} {}; it wins the tie because {}",
                self.names.satellite(other.satellite()),
                request.priority(),
                Self::span(other)?,
                tie_break(window, other, index, verdict.blocked_by)
            ),
            (ReasonCode::StationTurnaround, _, Some(other)) => {
                let rule = rule_for(self.rules, window.station());
                // The pass that comes first in time, and how far apart the two are.
                let (earlier, later, position) = if other.los() <= window.aos() {
                    (other, window, "begins")
                } else {
                    (window, other, "ends")
                };
                let gap = earlier
                    .los()
                    .seconds_until(later.aos())
                    .map_or_else(|| "too close".to_owned(), |gap| format!("{} s", gap.get()));
                let side = if position == "begins" {
                    "after"
                } else {
                    "before"
                };
                let needs = match rule.slew_time(earlier, later).get() {
                    0 => format!(
                        "{station} needs {} s between passes",
                        rule.turnaround().get()
                    ),
                    slew => format!(
                        "{station} needs {} s between these two passes ({} s fixed plus {slew} s \
                         to turn its antenna from the end of one to the start of the other)",
                        rule.gap_needed(earlier, later).get(),
                        rule.turnaround().get()
                    ),
                };
                format!(
                    "{needs} and this pass {position} {gap} {side} the pass of {} ({})",
                    self.names.satellite(other.satellite()),
                    Self::span(other)?
                )
            }
            (ReasonCode::SatelliteBusy, _, Some(other)) => format!(
                "{satellite} is scheduled with {} {} and can use one station at a time",
                self.names.station(other.station()),
                Self::span(other)?
            ),
            (ReasonCode::OutweighedByPlan, _, Some(other)) => {
                let rule = rule_for(self.rules, window.station());
                let clash = match conflict(window, other, &rule) {
                    Some(Conflict::SatelliteOverlap) => format!(
                        "{satellite} is scheduled with {} {}",
                        self.names.station(other.station()),
                        Self::span(other)?
                    ),
                    Some(Conflict::StationTurnaround) => {
                        let (earlier, later) = if other.los() <= window.aos() {
                            (other, window)
                        } else {
                            (window, other)
                        };
                        format!(
                            "{} has {station} {}, and {station} needs {} s between the two passes",
                            self.names.satellite(other.satellite()),
                            Self::span(other)?,
                            rule.gap_needed(earlier, later).get()
                        )
                    }
                    _ => format!(
                        "{} has {station} {}",
                        self.names.satellite(other.satellite()),
                        Self::span(other)?
                    ),
                };
                if self.satellite_floor {
                    format!(
                        "{clash}; using this pass instead would not raise the plan's total \
                         value, or would leave a satellite with less contact than the greedy \
                         plan gives it"
                    )
                } else {
                    format!(
                        "{clash}; using this pass instead would not raise the plan's total value"
                    )
                }
            }
            // A reason this function has no sentence for. The summary still explains it.
            (reason, _, _) => reason.summary().to_owned(),
        };
        Ok(detail)
    }
}

fn priority_text(requests: &[SatelliteRequest], satellite: SatelliteId) -> String {
    priority_of(requests, satellite).map_or_else(|| "none".to_owned(), |p| p.to_string())
}

/// Says in words which step of the ranking decided a tie between `loser` and `winner`.
fn tie_break(
    loser: &ContactWindow,
    winner: &ContactWindow,
    loser_index: usize,
    winner_index: Option<usize>,
) -> String {
    if winner.duration() != loser.duration() {
        format!(
            "its pass is longer ({} s against {} s)",
            winner.duration().get(),
            loser.duration().get()
        )
    } else if winner.aos() != loser.aos() {
        "the passes are equally long and its pass begins earlier".to_owned()
    } else if winner.satellite() != loser.satellite() {
        "the passes are identical in time and it has the lower catalog number".to_owned()
    } else if winner_index.is_some_and(|winner_index| winner_index < loser_index) {
        "the two passes are identical and it came first in the input".to_owned()
    } else {
        // Reached only if the ranking and this explanation ever disagree.
        debug_assert_eq!(
            rank(Priority::LOWEST, winner, Priority::LOWEST, loser),
            std::cmp::Ordering::Less
        );
        "it ranks higher".to_owned()
    }
}

fn satellite_summaries(
    entries: &[PlanEntry],
    requests: &[SatelliteRequest],
    names: &Names<'_>,
    floors: Option<&[u64]>,
) -> Vec<SatelliteSummary> {
    // `floors` is in the order of `requests`.
    let floor_of = |satellite: SatelliteId| {
        let position = requests.iter().position(|r| r.satellite() == satellite)?;
        let seconds = *floors?.get(position)?;
        Some(Seconds::new(u32::try_from(seconds).unwrap_or(u32::MAX)))
    };
    let mut ids: Vec<SatelliteId> = requests
        .iter()
        .map(SatelliteRequest::satellite)
        .chain(entries.iter().map(|entry| entry.satellite))
        .collect();
    ids.sort();
    ids.dedup();
    ids.into_iter()
        .map(|satellite| {
            let mine = || entries.iter().filter(move |e| e.satellite == satellite);
            SatelliteSummary {
                satellite,
                name: names.satellite(satellite),
                request: find_request(requests, satellite).copied(),
                passes_found: mine().count(),
                passes_scheduled: mine().filter(|e| e.is_scheduled()).count(),
                contact_scheduled: contact(mine()),
                floor: floor_of(satellite),
            }
        })
        .collect()
}

fn station_summaries(
    entries: &[PlanEntry],
    stations: &[GroundStation],
    rules: &[StationRule],
    names: &Names<'_>,
) -> Vec<StationSummary> {
    let mut ids: Vec<StationId> = stations
        .iter()
        .map(GroundStation::id)
        .chain(entries.iter().map(|entry| entry.station))
        .collect();
    ids.sort();
    ids.dedup();
    ids.into_iter()
        .map(|station| {
            let mine = || entries.iter().filter(move |e| e.station == station);
            StationSummary {
                station,
                name: names.station(station),
                turnaround: rule_for(rules, station).turnaround(),
                slew_rates: rule_for(rules, station).slew_rates(),
                passes_found: mine().count(),
                passes_scheduled: mine().filter(|e| e.is_scheduled()).count(),
                contact_scheduled: contact(mine()),
            }
        })
        .collect()
}

/// Total length of the scheduled entries in an iterator.
fn contact<'a>(entries: impl Iterator<Item = &'a PlanEntry>) -> Seconds {
    entries
        .filter(|entry| entry.is_scheduled())
        .filter_map(|entry| entry.window)
        .fold(Seconds::ZERO, |total, window| {
            total.saturating_add(window.duration())
        })
}

// ---------------------------------------------------------------------------------------------
// Text
// ---------------------------------------------------------------------------------------------

impl ContactPlan {
    /// Renders the plan for a person to read: the schedule, then every pass that is not in it
    /// with its reason, then what each satellite got and how busy each station is.
    #[must_use]
    pub fn to_text(&self) -> String {
        let mut out = String::new();
        let scheduled = self.scheduled().count();
        let _ = writeln!(out, "Contact plan ({} scheduler)", self.scheduler.name());
        let _ = writeln!(
            out,
            "  {} passes found: {scheduled} scheduled, {} not scheduled",
            self.entries.len(),
            self.entries.len() - scheduled
        );
        let _ = writeln!(
            out,
            "  {} s of scheduled contact, plan value {}",
            self.scheduled_contact().get(),
            self.value
        );
        let floored = self.optimality.is_some_and(|o| o.satellite_floor);
        if floored {
            let _ = writeln!(
                out,
                "  Floor: no satellite has less useful contact than the greedy plan gives it"
            );
        }
        if let Some(Optimality {
            proven: false,
            time_limit,
            ..
        }) = self.optimality
        {
            let _ = writeln!(
                out,
                "  NOT PROVEN OPTIMAL: the search reached its time limit of {} s; this is the \
                 best plan it had found, worth at least as much as the greedy plan",
                time_limit.as_secs_f64()
            );
        }
        if !self.is_complete() {
            let _ = writeln!(
                out,
                "  INCOMPLETE: {} satellite/station pairs could not be searched (listed below)",
                self.failures.len()
            );
        }

        let name_width = |names: &mut dyn Iterator<Item = &str>, heading: &str| {
            names.map(str::len).max().unwrap_or(0).max(heading.len())
        };
        let sat_width = name_width(
            &mut self.entries.iter().map(|e| e.satellite_name.as_str()),
            "SATELLITE",
        );
        let station_width = name_width(
            &mut self.entries.iter().map(|e| e.station_name.as_str()),
            "STATION",
        );
        let row = |out: &mut String, entry: &PlanEntry| {
            let priority = entry
                .priority
                .map_or_else(|| "-".to_owned(), |p| p.to_string());
            let _ = writeln!(
                out,
                "  {}  {:>6.0} s  {:<sat_width$}  {:<station_width$}  {:>5.2} deg  {:>3}",
                entry.aos.format(TIME),
                entry.duration_seconds(),
                entry.satellite_name,
                entry.station_name,
                entry.max_elevation_deg,
                priority,
            );
        };
        let header = |out: &mut String| {
            let _ = writeln!(
                out,
                "  {:<19}  {:>8}  {:<sat_width$}  {:<station_width$}  {:>9}  {:>3}",
                "AOS (UTC)", "LENGTH", "SATELLITE", "STATION", "PEAK", "PRI",
            );
        };

        let _ = writeln!(out, "\nSCHEDULED");
        if scheduled == 0 {
            let _ = writeln!(out, "  (none)");
        } else {
            header(&mut out);
            for entry in self.scheduled() {
                row(&mut out, entry);
            }
        }

        let _ = writeln!(out, "\nNOT SCHEDULED");
        if scheduled == self.entries.len() {
            let _ = writeln!(out, "  (none)");
        } else {
            header(&mut out);
            for entry in self.not_scheduled() {
                row(&mut out, entry);
                let _ = writeln!(out, "      {}", entry.reason);
                let _ = writeln!(out, "      {}", entry.detail);
            }
        }

        let _ = writeln!(out, "\nSATELLITES");
        let width = name_width(&mut self.satellites.iter().map(|s| s.name.as_str()), "NAME");
        // The FLOOR column is printed only for a plan built with a floor.
        let floor_heading = if floored { "      FLOOR  " } else { "" };
        let _ = writeln!(
            out,
            "  {:<width$}  {:>3}  {:>9}  {floor_heading}{:>9}  {:>6}  NEED MET",
            "NAME", "PRI", "NEED", "SCHEDULED", "PASSES",
        );
        for satellite in &self.satellites {
            let (priority, need) = match &satellite.request {
                None => ("-".to_owned(), "-".to_owned()),
                Some(request) if request.is_unlimited() => {
                    (request.priority().to_string(), "all".to_owned())
                }
                Some(request) => (
                    request.priority().to_string(),
                    format!("{} s", request.need().get()),
                ),
            };
            let met = match satellite.need_met() {
                Some(true) => "yes",
                Some(false) => "NO",
                None => "-",
            };
            let floor = match (floored, satellite.floor) {
                (false, _) => String::new(),
                (true, Some(floor)) => format!("{:>9} s  ", floor.get()),
                (true, None) => format!("{:>11}  ", "-"),
            };
            let _ = writeln!(
                out,
                "  {:<width$}  {priority:>3}  {need:>9}  {floor}{:>7} s  {:>2} of {:<2}  {met}",
                satellite.name,
                satellite.contact_scheduled.get(),
                satellite.passes_scheduled,
                satellite.passes_found,
            );
        }

        let _ = writeln!(out, "\nSTATIONS");
        let width = name_width(&mut self.stations.iter().map(|s| s.name.as_str()), "NAME");
        // Slew rates as "azimuth/elevation" in degrees per second, or "-" for a station
        // without slew limits.
        let slew: Vec<String> = self
            .stations
            .iter()
            .map(|station| match station.slew_rates {
                Some(rates) => format!(
                    "{:.2}/{:.2} deg/s",
                    f64::from(rates.azimuth_centidegrees_per_second()) / 100.0,
                    f64::from(rates.elevation_centidegrees_per_second()) / 100.0
                ),
                None => "-".to_owned(),
            })
            .collect();
        const SLEW_HEADING: &str = "SLEW AZ/EL";
        let slew_width = name_width(&mut slew.iter().map(String::as_str), SLEW_HEADING);
        let _ = writeln!(
            out,
            "  {:<width$}  {:>10}  {:>slew_width$}  {:>9}  {:>6}",
            "NAME", "TURNAROUND", SLEW_HEADING, "SCHEDULED", "PASSES",
        );
        for (station, slew) in self.stations.iter().zip(&slew) {
            let _ = writeln!(
                out,
                "  {:<width$}  {:>8} s  {:>slew_width$}  {:>7} s  {:>2} of {}",
                station.name,
                station.turnaround.get(),
                slew,
                station.contact_scheduled.get(),
                station.passes_scheduled,
                station.passes_found,
            );
        }

        if !self.is_complete() {
            let _ = writeln!(out, "\nNOT SEARCHED");
            for failure in &self.failures {
                let _ = writeln!(out, "  {failure}");
            }
        }
        out
    }
}

// ---------------------------------------------------------------------------------------------
// JSON
// ---------------------------------------------------------------------------------------------

/// The identifier of the JSON layout. The number changes only if a field is removed or changes
/// meaning; adding a field does not change it.
pub const JSON_SCHEMA: &str = "maat.contact-plan/1";

/// The format for times in JSON: ISO 8601 in UTC, with milliseconds.
const JSON_TIME: &str = "%Y-%m-%dT%H:%M:%S%.3fZ";

/// A window's pointing at AOS and LOS in degrees, if it records one.
fn pointing_json(window: ContactWindow) -> Option<Value> {
    let degrees = |pointing: Pointing| {
        json!({
            "azimuth_deg": f64::from(pointing.azimuth.centidegrees()) / 100.0,
            "elevation_deg": f64::from(pointing.elevation.centidegrees()) / 100.0,
        })
    };
    Some(json!({
        "aos": degrees(window.aos_pointing()?),
        "los": degrees(window.los_pointing()?),
    }))
}

fn reason_json(reason: ReasonCode) -> Value {
    json!({
        "code": reason.id(),
        "number": reason.code(),
        "class": reason.class().label(),
        "summary": reason.summary(),
    })
}

impl ContactPlan {
    /// The plan as a JSON value. See [`ContactPlan::to_json`] for the layout.
    #[must_use]
    pub fn to_json_value(&self) -> Value {
        let entries: Vec<Value> = self
            .entries
            .iter()
            .enumerate()
            .map(|(index, entry)| {
                json!({
                    "index": index,
                    "satellite": { "id": entry.satellite.get(), "name": entry.satellite_name },
                    "station": { "id": entry.station.get(), "name": entry.station_name },
                    "aos": entry.aos.format(JSON_TIME).to_string(),
                    "los": entry.los.format(JSON_TIME).to_string(),
                    "duration_s": entry.duration_seconds(),
                    "max_elevation_deg": entry.max_elevation_deg,
                    // Where the antenna points at each end of the window, or `null`.
                    "pointing": entry.window.and_then(pointing_json),
                    "priority": entry.priority.map(Priority::get),
                    "scheduled": entry.is_scheduled(),
                    "decided_by": entry.decided_by.name(),
                    "reason": reason_json(entry.reason),
                    "detail": entry.detail,
                    "blocked_by": entry.blocked_by,
                })
            })
            .collect();

        let satellites: Vec<Value> = self
            .satellites
            .iter()
            .map(|satellite| {
                let request = satellite.request.as_ref().map(|request| {
                    json!({
                        "priority": request.priority().get(),
                        "min_contact_s": request.min_contact().get(),
                        // `null` means no limit: every pass is wanted.
                        "need_s": (!request.is_unlimited()).then(|| request.need().get()),
                    })
                });
                json!({
                    "id": satellite.satellite.get(),
                    "name": satellite.name,
                    "request": request,
                    "passes_found": satellite.passes_found,
                    "passes_scheduled": satellite.passes_scheduled,
                    "contact_scheduled_s": satellite.contact_scheduled.get(),
                    // `null` unless the plan was built with a floor.
                    "floor_s": satellite.floor.map(Seconds::get),
                    "need_met": satellite.need_met(),
                })
            })
            .collect();

        let stations: Vec<Value> = self
            .stations
            .iter()
            .map(|station| {
                json!({
                    "id": station.station.get(),
                    "name": station.name,
                    "turnaround_s": station.turnaround.get(),
                    // `null` for a station without slew limits.
                    "slew_rates_deg_per_s": station.slew_rates.map(|rates| json!({
                        "azimuth": f64::from(rates.azimuth_centidegrees_per_second()) / 100.0,
                        "elevation": f64::from(rates.elevation_centidegrees_per_second()) / 100.0,
                    })),
                    "passes_found": station.passes_found,
                    "passes_scheduled": station.passes_scheduled,
                    "contact_scheduled_s": station.contact_scheduled.get(),
                })
            })
            .collect();

        let scheduled = self.scheduled().count();
        json!({
            "schema": JSON_SCHEMA,
            "scheduler": self.scheduler.name(),
            // `null` for a greedy plan, which has nothing to prove.
            "optimality": self.optimality.map(|optimality| json!({
                "proven": optimality.proven,
                "time_limit_s": optimality.time_limit.as_secs_f64(),
                "satellite_floor": optimality.satellite_floor,
            })),
            "complete": self.is_complete(),
            "totals": {
                "passes": self.entries.len(),
                "scheduled": scheduled,
                "not_scheduled": self.entries.len() - scheduled,
                "contact_scheduled_s": self.scheduled_contact().get(),
                "value": self.value,
            },
            "entries": entries,
            "satellites": satellites,
            "stations": stations,
            "failures": self.failures,
        })
    }

    /// Renders the plan as indented JSON for another program to read.
    ///
    /// The top level has `schema` ([`JSON_SCHEMA`]), `scheduler`, `optimality`, `complete`, `totals`,
    /// `entries`, `satellites`, `stations` and `failures`. Each entry carries its `reason` as an
    /// object with the stable `code` (`"MAAT-101"`), its `number`, `class` and `summary`, plus
    /// the `detail` sentence and, for a pass that lost to another, `blocked_by`: the `index` of
    /// that pass in `entries`. Times are ISO 8601 in UTC. Keys are in alphabetical order.
    #[must_use]
    pub fn to_json(&self) -> String {
        // Serializing a `Value` cannot fail: it has no non-string keys and no custom types.
        serde_json::to_string_pretty(&self.to_json_value()).unwrap_or_else(|_| "{}".to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::station::SlewLimits;
    use crate::station::StationSite;
    use crate::survey::{RejectedPass, SurveyFailure};
    use crate::testkit::{Lcg, pointed, random_instance, request, window};
    use chrono::TimeZone;

    // --- Helpers -------------------------------------------------------------------------------

    /// Times in these tests count from 2026-08-01 12:00:00 UTC.
    const T0: i64 = 1_785_585_600;

    fn at(offset: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(T0 + offset, 0).unwrap()
    }

    fn w(satellite: u32, station: u16, aos: i64, los: i64) -> ContactWindow {
        window(satellite, station, T0 + aos, T0 + los)
    }

    fn station(id: u16, name: &str, turnaround: u32) -> GroundStation {
        GroundStation::new(
            StationId::new(id),
            name,
            StationSite {
                latitude_deg: 10.0,
                longitude_deg: 20.0,
                altitude_m: 0.0,
            },
            10.0,
        )
        .unwrap()
        .with_turnaround(Seconds::new(turnaround))
    }

    fn survey(windows: Vec<ContactWindow>) -> WindowSurvey {
        WindowSurvey {
            windows,
            rejected: Vec::new(),
            failures: Vec::new(),
        }
    }

    fn plan(
        windows: Vec<ContactWindow>,
        stations: &[GroundStation],
        requests: &[SatelliteRequest],
        scheduler: Scheduler,
    ) -> ContactPlan {
        plan_contacts(&survey(windows), &[], stations, requests, scheduler).unwrap()
    }

    fn entry_for<'a>(plan: &'a ContactPlan, window: &ContactWindow) -> &'a PlanEntry {
        plan.entries
            .iter()
            .find(|entry| entry.window.as_ref() == Some(window))
            .expect("every window has an entry")
    }

    #[test]
    fn t0_is_the_date_the_tests_assume() {
        assert_eq!(at(0), Utc.with_ymd_and_hms(2026, 8, 1, 12, 0, 0).unwrap());
    }

    // --- Options and the time limit ------------------------------------------------------------

    #[test]
    fn options_default_to_a_one_minute_limit() {
        assert_eq!(DEFAULT_OPTIMAL_TIME_LIMIT, Duration::from_secs(60));
        let options = PlanOptions::new(Scheduler::Optimal);
        assert_eq!(options.scheduler, Scheduler::Optimal);
        assert_eq!(options.optimal_time_limit, DEFAULT_OPTIMAL_TIME_LIMIT);
        assert!(!options.satellite_floor);
        let short = options.with_optimal_time_limit(Duration::from_millis(250));
        assert_eq!(short.optimal_time_limit, Duration::from_millis(250));
        assert_eq!(short.scheduler, Scheduler::Optimal);
    }

    #[test]
    fn plan_contacts_is_plan_contacts_with_the_default_options() {
        let windows = vec![w(1, 1, 0, 600), w(2, 1, 300, 900), w(2, 2, 2_000, 2_450)];
        let stations = [station(1, "Seattle", 30), station(2, "Quito", 0)];
        let requests = [request(1, 9), request(2, 1)];
        for scheduler in [Scheduler::Greedy, Scheduler::Optimal] {
            let plain = plan_contacts(
                &survey(windows.clone()),
                &[],
                &stations,
                &requests,
                scheduler,
            );
            let with = plan_contacts_with(
                &survey(windows.clone()),
                &[],
                &stations,
                &requests,
                &PlanOptions::new(scheduler),
            );
            assert_eq!(plain.unwrap(), with.unwrap());
        }
    }

    #[test]
    fn a_greedy_plan_has_no_optimality_and_an_optimal_plan_says_it_is_proven() {
        let windows = vec![w(1, 1, 0, 600), w(2, 1, 300, 900)];
        let stations = [station(1, "Seattle", 0)];
        let requests = [request(1, 9), request(2, 1)];
        let greedy = plan(windows.clone(), &stations, &requests, Scheduler::Greedy);
        assert_eq!(greedy.optimality, None);

        let optimal = plan(windows, &stations, &requests, Scheduler::Optimal);
        assert_eq!(
            optimal.optimality,
            Some(Optimality {
                proven: true,
                time_limit: DEFAULT_OPTIMAL_TIME_LIMIT,
                satellite_floor: false,
            })
        );
        assert!(!optimal.to_text().contains("NOT PROVEN"));
        let json: Value = serde_json::from_str(&optimal.to_json()).unwrap();
        assert_eq!(
            json["optimality"],
            json!({ "proven": true, "time_limit_s": 60.0, "satellite_floor": false })
        );
        let json: Value = serde_json::from_str(&greedy.to_json()).unwrap();
        assert_eq!(json["optimality"], Value::Null);
    }

    #[test]
    fn a_plan_cut_short_by_the_time_limit_says_so_and_is_still_a_whole_plan() {
        // No time at all: the solver cannot prove anything, so the plan is the greedy one,
        // explained in the optimal scheduler's terms and flagged wherever it is printed.
        let mut rng = Lcg(0x7469_6d65);
        let instance = random_instance(&mut rng, 60);
        let stations: Vec<GroundStation> = (1..=3)
            .map(|id| station(id, &format!("S{id}"), 0))
            .collect();
        let build = |options: &PlanOptions| {
            plan_contacts_with(
                &survey(instance.windows.clone()),
                &[],
                &stations,
                &instance.requests,
                options,
            )
            .unwrap()
        };
        let cut_short =
            build(&PlanOptions::new(Scheduler::Optimal).with_optimal_time_limit(Duration::ZERO));
        let greedy = build(&PlanOptions::new(Scheduler::Greedy));

        assert_eq!(
            cut_short.optimality,
            Some(Optimality {
                proven: false,
                time_limit: Duration::ZERO,
                satellite_floor: false,
            })
        );
        assert_eq!(cut_short.entries.len(), instance.windows.len());
        assert!(cut_short.value >= greedy.value);
        for entry in &cut_short.entries {
            assert!(!entry.detail.is_empty());
        }

        let text = cut_short.to_text();
        assert!(
            text.lines().nth(3).unwrap().starts_with(
                "  NOT PROVEN OPTIMAL: the search reached its time limit of 0 s; this is the best"
            ),
            "{text}"
        );
        let json: Value = serde_json::from_str(&cut_short.to_json()).unwrap();
        assert_eq!(
            json["optimality"],
            json!({ "proven": false, "time_limit_s": 0.0, "satellite_floor": false })
        );
    }

    #[test]
    fn time_until_first_pass_is_measured_to_the_earliest_window() {
        // Windows out of order: the first to begin is 300 s after T0.
        let survey = survey(vec![
            w(1, 1, 900, 1_200),
            w(2, 1, 300, 600),
            w(3, 2, 5_000, 5_100),
        ]);
        assert_eq!(
            time_until_first_pass(&survey, at(0)),
            Some(Duration::from_secs(300))
        );
        assert_eq!(
            time_until_first_pass(&survey, at(-3_600)),
            Some(Duration::from_secs(3_900))
        );
        // Sub-second times are kept.
        assert_eq!(
            time_until_first_pass(&survey, at(299) + chrono::Duration::milliseconds(250)),
            Some(Duration::from_millis(750))
        );
    }

    #[test]
    fn there_is_no_time_until_a_pass_that_has_begun_or_when_there_are_no_passes() {
        let survey_with = survey(vec![w(1, 1, 300, 600)]);
        assert_eq!(time_until_first_pass(&survey_with, at(300)), None);
        assert_eq!(time_until_first_pass(&survey_with, at(450)), None);
        assert_eq!(time_until_first_pass(&survey(Vec::new()), at(0)), None);
    }

    // --- A floor for each satellite ------------------------------------------------------------

    /// One station. Satellite 1 has a 100 s pass; satellite 2 has two 60 s passes that overlap
    /// it. Greedy gives satellite 1 its pass; the best total gives satellite 2 both of its.
    fn contested(options: &PlanOptions) -> ContactPlan {
        plan_contacts_with(
            &survey(vec![w(1, 1, 0, 100), w(2, 1, 0, 60), w(2, 1, 60, 120)]),
            &[],
            &[station(1, "Seattle", 0)],
            &[request(1, 5), request(2, 5)],
            options,
        )
        .unwrap()
    }

    fn contact_of(plan: &ContactPlan, satellite: u32) -> u32 {
        plan.satellites
            .iter()
            .find(|s| s.satellite.get() == satellite)
            .unwrap()
            .contact_scheduled
            .get()
    }

    #[test]
    fn a_floor_keeps_every_satellite_at_its_greedy_share() {
        let free = contested(&PlanOptions::new(Scheduler::Optimal));
        assert_eq!((contact_of(&free, 1), contact_of(&free, 2)), (0, 120));
        assert!(!free.optimality.unwrap().satellite_floor);
        assert!(free.satellites.iter().all(|s| s.floor.is_none()));

        let floored = contested(&PlanOptions::new(Scheduler::Optimal).with_satellite_floor(true));
        assert_eq!((contact_of(&floored, 1), contact_of(&floored, 2)), (100, 0));
        let optimality = floored.optimality.unwrap();
        assert!(optimality.satellite_floor && optimality.proven);
        assert_eq!(floored.satellites[0].floor, Some(Seconds::new(100)));
        assert_eq!(floored.satellites[1].floor, Some(Seconds::ZERO));
        // The passes that lost say that the floor is part of the reason.
        assert_eq!(floored.entries[1].reason, ReasonCode::OutweighedByPlan);
        assert!(
            floored.entries[1].detail.ends_with(
                "using this pass instead would not raise the plan's total value, or would \
                 leave a satellite with less contact than the greedy plan gives it"
            ),
            "{}",
            floored.entries[1].detail
        );
    }

    #[test]
    fn the_greedy_scheduler_ignores_the_floor_option() {
        let plain = contested(&PlanOptions::new(Scheduler::Greedy));
        let asked = contested(&PlanOptions::new(Scheduler::Greedy).with_satellite_floor(true));
        assert_eq!(plain, asked);
        assert!(asked.satellites.iter().all(|s| s.floor.is_none()));
        assert!(!asked.to_text().contains("FLOOR"));
    }

    #[test]
    fn a_plan_with_a_floor_shows_it_in_text_and_json() {
        let floored = contested(&PlanOptions::new(Scheduler::Optimal).with_satellite_floor(true));
        let text = floored.to_text();
        assert_eq!(
            text.lines().nth(3).unwrap(),
            "  Floor: no satellite has less useful contact than the greedy plan gives it"
        );
        assert!(
            text.contains(
                "SATELLITES\n  \
                   NAME         PRI       NEED        FLOOR  SCHEDULED  PASSES  NEED MET\n  \
                   satellite 1    5        all        100 s      100 s   1 of 1   -\n  \
                   satellite 2    5        all          0 s        0 s   0 of 2   -\n"
            ),
            "{text}"
        );
        for line in text.lines() {
            assert_eq!(line, line.trim_end(), "trailing space in {line:?}");
        }

        let json: Value = serde_json::from_str(&floored.to_json()).unwrap();
        assert_eq!(json["optimality"]["satellite_floor"], true);
        assert_eq!(json["satellites"][0]["floor_s"], 100);
        assert_eq!(json["satellites"][1]["floor_s"], 0);
    }

    #[test]
    fn a_satellite_without_a_request_has_no_floor() {
        let plan = plan_contacts_with(
            &survey(vec![w(1, 1, 0, 100), w(9, 1, 200, 300)]),
            &[],
            &[station(1, "Seattle", 0)],
            &[request(1, 5)],
            &PlanOptions::new(Scheduler::Optimal).with_satellite_floor(true),
        )
        .unwrap();
        assert_eq!(plan.satellites[0].floor, Some(Seconds::new(100)));
        assert_eq!(plan.satellites[1].floor, None);
        assert!(
            plan.to_text()
                .contains("satellite 9    -          -            -        0 s")
        );
    }

    // --- Structure -----------------------------------------------------------------------------

    #[test]
    fn an_empty_survey_gives_an_empty_complete_plan() {
        let plan = plan(Vec::new(), &[], &[], Scheduler::Greedy);
        assert!(plan.entries.is_empty() && plan.satellites.is_empty() && plan.stations.is_empty());
        assert!(plan.is_complete());
        assert_eq!(plan.value, 0.0);
        assert_eq!(plan.scheduled_contact(), Seconds::ZERO);
    }

    #[test]
    fn every_window_and_every_survey_rejection_gets_exactly_one_entry() {
        let windows = vec![w(1, 1, 0, 600), w(2, 1, 300, 900)];
        let mut survey = survey(windows);
        survey.rejected.push(RejectedPass {
            satellite: SatelliteId::new(1),
            station: StationId::new(1),
            aos: at(2_000) + chrono::Duration::milliseconds(250),
            los: at(2_400),
            max_elevation_deg: 88.5,
            reason: ReasonCode::SlewRateExceeded,
            detail: "tracking needs 9.00 deg/s in azimuth (the station allows 6.00)".to_owned(),
        });
        let requests = [request(1, 9), request(2, 1)];
        let plan = plan_contacts(&survey, &[], &[], &requests, Scheduler::Greedy).unwrap();

        assert_eq!(plan.entries.len(), 3);
        let from_survey = &plan.entries[2];
        assert_eq!(from_survey.decided_by, DecidedBy::Survey);
        assert_eq!(from_survey.reason, ReasonCode::SlewRateExceeded);
        assert_eq!(
            from_survey.detail,
            "tracking needs 9.00 deg/s in azimuth (the station allows 6.00)"
        );
        assert_eq!(from_survey.window, None);
        assert_eq!(from_survey.priority, Some(Priority::new(9)));
        assert!((from_survey.duration_seconds() - 399.75).abs() < 1e-9);
        assert!(
            plan.entries[..2]
                .iter()
                .all(|e| e.decided_by == DecidedBy::Scheduler)
        );
    }

    #[test]
    fn entries_are_in_order_of_aos_whatever_the_order_of_the_windows() {
        let windows = vec![
            w(1, 1, 5_000, 5_300),
            w(2, 2, 0, 300),
            w(3, 1, 2_000, 2_300),
        ];
        let requests = [request(1, 1), request(2, 1), request(3, 1)];
        let plan = plan(windows, &[], &requests, Scheduler::Greedy);
        let starts: Vec<_> = plan.entries.iter().map(|e| e.aos).collect();
        assert_eq!(starts, [at(0), at(2_000), at(5_000)]);
    }

    #[test]
    fn blocked_by_points_at_the_blocking_entry_after_sorting() {
        // The loser comes first in the input and the winner second, but the winner starts
        // earlier, so sorting swaps them. `blocked_by` must follow the winner to its new place.
        let loser = w(1, 1, 300, 900);
        let winner = w(2, 1, 0, 600);
        let plan = plan(
            vec![loser, winner],
            &[],
            &[request(1, 1), request(2, 9)],
            Scheduler::Greedy,
        );
        assert_eq!(plan.entries[0].window, Some(winner));
        assert_eq!(plan.entries[1].window, Some(loser));
        assert_eq!(plan.entries[1].reason, ReasonCode::StationBusy);
        assert_eq!(plan.entries[1].blocked_by, Some(0));
        assert_eq!(plan.entries[0].blocked_by, None);
    }

    #[test]
    fn blocked_by_is_always_a_scheduled_entry_that_really_conflicts() {
        let mut rng = Lcg(0x706c_616e);
        for round in 0..80 {
            let instance = random_instance(&mut rng, 30);
            for scheduler in [Scheduler::Greedy, Scheduler::Optimal] {
                // Stations with the instance's turnaround times, and a slow mount on each so
                // that the time to turn between passes is in play.
                let stations: Vec<GroundStation> = (1..=3)
                    .map(|id| {
                        let turnaround = rule_for(&instance.rules, StationId::new(id)).turnaround();
                        station(id, &format!("S{id}"), turnaround.get())
                            .with_slew_limits(SlewLimits {
                                azimuth_deg_per_s: 2.0,
                                elevation_deg_per_s: 2.0,
                            })
                            .unwrap()
                    })
                    .collect();
                let rules: Vec<StationRule> = stations.iter().map(GroundStation::rule).collect();
                let plan = plan(
                    instance.windows.clone(),
                    &stations,
                    &instance.requests,
                    scheduler,
                );
                assert_eq!(plan.entries.len(), instance.windows.len());
                for (index, entry) in plan.entries.iter().enumerate() {
                    let Some(blocker) = entry.blocked_by else {
                        continue;
                    };
                    assert_ne!(blocker, index, "round {round}");
                    let other = &plan.entries[blocker];
                    assert!(
                        other.is_scheduled(),
                        "round {round}: blocker is not scheduled"
                    );
                    let rule = rule_for(&rules, entry.station);
                    assert!(
                        conflict(&entry.window.unwrap(), &other.window.unwrap(), &rule).is_some(),
                        "round {round}: entry {index} does not conflict with its blocker"
                    );
                }
            }
        }
    }

    // --- Names ---------------------------------------------------------------------------------

    #[test]
    fn stations_are_named_and_unknown_ones_fall_back_to_their_number() {
        let windows = vec![w(1, 1, 0, 600), w(1, 2, 1_000, 1_600)];
        let plan = plan(
            windows,
            &[station(1, "Seattle", 0)],
            &[request(1, 1)],
            Scheduler::Greedy,
        );
        assert_eq!(plan.entries[0].station_name, "Seattle");
        assert_eq!(plan.entries[1].station_name, "station 2");
        // No `Satellite` list was given, so satellites fall back too.
        assert_eq!(plan.entries[0].satellite_name, "satellite 1");
    }

    // --- Detail sentences ----------------------------------------------------------------------

    #[test]
    fn a_scheduled_pass_states_its_priority_and_length() {
        let plan = plan(
            vec![w(1, 1, 0, 600)],
            &[],
            &[request(1, 7)],
            Scheduler::Greedy,
        );
        assert_eq!(plan.entries[0].detail, "priority 7, 600 s of contact");
    }

    #[test]
    fn a_scheduled_pass_toward_a_need_reports_progress() {
        let windows = vec![w(1, 1, 0, 600), w(1, 1, 1_000, 1_300)];
        let requests = [request(1, 7).with_need(Seconds::new(800))];
        let plan = plan(windows, &[], &requests, Scheduler::Greedy);
        assert_eq!(
            plan.entries[0].detail,
            "priority 7, 600 s of contact; satellite 1 gets 900 s in all of the 800 s it asked for"
        );
    }

    #[test]
    fn a_station_conflict_names_the_winner_its_priority_and_its_times() {
        let windows = vec![w(1, 1, 0, 600), w(2, 1, 300, 900)];
        let plan = plan(
            windows,
            &[station(1, "Seattle", 0)],
            &[request(1, 9), request(2, 1)],
            Scheduler::Greedy,
        );
        assert_eq!(plan.entries[1].reason, ReasonCode::StationBusy);
        assert_eq!(
            plan.entries[1].detail,
            "satellite 1 (priority 9) has Seattle from 2026-08-01 12:00:00 to 2026-08-01 \
             12:10:00; this pass has priority 1"
        );
    }

    #[test]
    fn a_lost_tie_says_which_step_decided_it() {
        let detail = |a: ContactWindow, b: ContactWindow, requests: &[SatelliteRequest]| {
            let plan = plan(
                vec![a, b],
                &[station(1, "Seattle", 0)],
                requests,
                Scheduler::Greedy,
            );
            let loser = plan
                .entries
                .iter()
                .find(|e| e.reason == ReasonCode::StationTieLost)
                .expect("one pass loses the tie");
            loser.detail.clone()
        };
        let equal = [request(1, 5), request(2, 5)];

        let longer = detail(w(1, 1, 0, 300), w(2, 1, 200, 600), &equal);
        assert_eq!(
            longer,
            "satellite 2 has the same priority (5) and has Seattle from 2026-08-01 12:03:20 to \
             2026-08-01 12:10:00; it wins the tie because its pass is longer (400 s against 300 s)"
        );
        let earlier = detail(w(1, 1, 100, 400), w(2, 1, 0, 300), &equal);
        assert!(earlier.ends_with("the passes are equally long and its pass begins earlier"));
        let catalog = detail(w(2, 1, 0, 300), w(1, 1, 0, 300), &equal);
        assert!(
            catalog
                .ends_with("the passes are identical in time and it has the lower catalog number")
        );
        let duplicate = detail(w(1, 1, 0, 300), w(1, 1, 0, 300), &[request(1, 5)]);
        assert!(duplicate.ends_with("the two passes are identical and it came first in the input"));
    }

    #[test]
    fn a_turnaround_conflict_gives_the_gap_and_the_time_needed() {
        let stations = [station(1, "Seattle", 60)];
        let requests = [request(1, 9), request(2, 1)];

        let after = plan(
            vec![w(1, 1, 0, 600), w(2, 1, 630, 900)],
            &stations,
            &requests,
            Scheduler::Greedy,
        );
        assert_eq!(after.entries[1].reason, ReasonCode::StationTurnaround);
        assert_eq!(
            after.entries[1].detail,
            "Seattle needs 60 s between passes and this pass begins 30 s after the pass of \
             satellite 1 (from 2026-08-01 12:00:00 to 2026-08-01 12:10:00)"
        );

        let before = plan(
            vec![w(2, 1, 0, 570), w(1, 1, 600, 1_200)],
            &stations,
            &requests,
            Scheduler::Greedy,
        );
        assert_eq!(before.entries[0].reason, ReasonCode::StationTurnaround);
        assert!(
            before.entries[0]
                .detail
                .contains("this pass ends 30 s before the pass of")
        );
    }

    /// A station that turns at 6 degrees per second in azimuth and 2.5 in elevation.
    fn rotating(id: u16, name: &str, turnaround: u32) -> GroundStation {
        station(id, name, turnaround)
            .with_slew_limits(SlewLimits {
                azimuth_deg_per_s: 6.0,
                elevation_deg_per_s: 2.5,
            })
            .unwrap()
    }

    /// A window with pointing, counted from `T0`.
    fn p(sat: u32, station: u16, aos: i64, los: i64, rise: u16, set: u16) -> ContactWindow {
        pointed(sat, station, T0 + aos, T0 + los, rise, set)
    }

    #[test]
    fn a_turnaround_conflict_at_a_slewing_station_gives_both_parts_of_the_time_needed() {
        // The first pass sets due east; the second rises due west 60 s later. Seattle needs
        // 45 s fixed plus half a turn at 6 deg/s (30 s): 75 s.
        let windows = vec![p(1, 1, 0, 600, 0, 9_000), p(2, 1, 660, 900, 27_000, 0)];
        let requests = [request(1, 9), request(2, 1)];

        let slewing = plan(
            windows.clone(),
            &[rotating(1, "Seattle", 45)],
            &requests,
            Scheduler::Greedy,
        );
        assert_eq!(slewing.entries[1].reason, ReasonCode::StationTurnaround);
        assert_eq!(slewing.entries[1].blocked_by, Some(0));
        assert_eq!(
            slewing.entries[1].detail,
            "Seattle needs 75 s between these two passes (45 s fixed plus 30 s to turn its \
             antenna from the end of one to the start of the other) and this pass begins 60 s \
             after the pass of satellite 1 (from 2026-08-01 12:00:00 to 2026-08-01 12:10:00)"
        );

        // The same passes at a station with no slew limits fit: 60 s is more than 45.
        let fixed = plan(
            windows,
            &[station(1, "Seattle", 45)],
            &requests,
            Scheduler::Greedy,
        );
        assert!(fixed.entries[1].is_scheduled());
    }

    #[test]
    fn the_optimal_scheduler_explains_a_slew_conflict_with_the_time_needed() {
        let windows = vec![p(1, 1, 0, 600, 0, 9_000), p(2, 1, 660, 900, 27_000, 0)];
        let plan = plan(
            windows,
            &[rotating(1, "Seattle", 45)],
            &[request(1, 5), request(2, 5)],
            Scheduler::Optimal,
        );
        assert_eq!(plan.entries[1].reason, ReasonCode::OutweighedByPlan);
        assert!(
            plan.entries[1]
                .detail
                .contains("and Seattle needs 75 s between the two passes"),
            "{}",
            plan.entries[1].detail
        );
    }

    #[test]
    fn a_satellite_conflict_names_the_other_station() {
        let windows = vec![w(7, 1, 0, 300), w(7, 2, 100, 600)];
        let stations = [station(1, "Seattle", 0), station(2, "Quito", 0)];
        let plan = plan(windows, &stations, &[request(7, 5)], Scheduler::Greedy);
        assert_eq!(plan.entries[0].reason, ReasonCode::SatelliteBusy);
        assert_eq!(
            plan.entries[0].detail,
            "satellite 7 is scheduled with Quito from 2026-08-01 12:01:40 to 2026-08-01 12:10:00 \
             and can use one station at a time"
        );
    }

    #[test]
    fn screening_reasons_give_the_numbers() {
        let windows = vec![w(9, 1, 0, 300), w(1, 1, 1_000, 1_090)];
        let requests = [request(1, 5).with_min_contact(Seconds::new(120))];
        let plan = plan(windows, &[], &requests, Scheduler::Greedy);
        assert_eq!(
            plan.entries[0].detail,
            "no contact request names satellite 9 (catalog number 9)"
        );
        assert_eq!(
            plan.entries[1].detail,
            "the pass lasts 90 s and satellite 1 asks for passes of at least 120 s"
        );
    }

    #[test]
    fn a_met_need_gives_what_was_asked_and_what_was_granted() {
        let windows = vec![w(1, 1, 0, 600), w(1, 1, 1_000, 1_200)];
        let requests = [request(1, 5).with_need(Seconds::new(500))];
        let plan = plan(windows, &[], &requests, Scheduler::Greedy);
        assert_eq!(plan.entries[1].reason, ReasonCode::NeedAlreadyMet);
        assert_eq!(
            plan.entries[1].detail,
            "satellite 1 asked for 500 s of contact and its scheduled passes give it 600 s"
        );
    }

    #[test]
    fn an_optimizer_rejection_names_the_kept_pass_and_the_kind_of_conflict() {
        let stations = [station(1, "Seattle", 0), station(2, "Quito", 0)];

        // Station overlap.
        let station_clash = plan(
            vec![w(1, 1, 0, 900), w(2, 1, 400, 410)],
            &stations,
            &[request(1, 0), request(2, 9)],
            Scheduler::Optimal,
        );
        assert_eq!(
            station_clash.entries[1].reason,
            ReasonCode::OutweighedByPlan
        );
        assert_eq!(
            station_clash.entries[1].detail,
            "satellite 1 has Seattle from 2026-08-01 12:00:00 to 2026-08-01 12:15:00; using this \
             pass instead would not raise the plan's total value"
        );

        // Same satellite at two stations.
        let satellite_clash = plan(
            vec![w(7, 1, 0, 300), w(7, 2, 100, 600)],
            &stations,
            &[request(7, 5)],
            Scheduler::Optimal,
        );
        assert!(
            satellite_clash.entries[0]
                .detail
                .starts_with("satellite 7 is scheduled with Quito from")
        );

        // Turnaround.
        let slow = [station(1, "Seattle", 60)];
        let turnaround_clash = plan(
            vec![w(1, 1, 0, 600), w(2, 1, 630, 900)],
            &slow,
            &[request(1, 5), request(2, 5)],
            Scheduler::Optimal,
        );
        assert!(
            turnaround_clash.entries[1]
                .detail
                .contains("and Seattle needs 60 s between the two passes"),
            "{}",
            turnaround_clash.entries[1].detail
        );
    }

    #[test]
    fn explanation_joins_code_summary_and_detail() {
        let plan = plan(
            vec![w(1, 1, 0, 600)],
            &[],
            &[request(1, 7)],
            Scheduler::Greedy,
        );
        assert_eq!(
            plan.entries[0].explanation(),
            "MAAT-001: scheduled: the pass is in the contact plan (priority 7, 600 s of contact)"
        );
    }

    #[test]
    fn every_entry_of_random_plans_has_a_specific_detail() {
        // The fallback for an unexpected reason is the bare summary. No reason either scheduler
        // can produce should fall through to it.
        let mut rng = Lcg(0x6465_7461);
        for _ in 0..60 {
            let instance = random_instance(&mut rng, 40);
            for scheduler in [Scheduler::Greedy, Scheduler::Optimal] {
                let plan = plan(instance.windows.clone(), &[], &instance.requests, scheduler);
                for entry in &plan.entries {
                    assert!(!entry.detail.is_empty());
                    assert_ne!(entry.detail, entry.reason.summary(), "{:?}", entry.reason);
                }
            }
        }
    }

    // --- Summaries and value -------------------------------------------------------------------

    #[test]
    fn satellite_summaries_report_what_was_asked_and_what_was_given() {
        let windows = vec![
            w(1, 1, 0, 600),
            w(1, 1, 1_000, 1_200),
            w(2, 1, 300, 900),
            w(9, 2, 0, 100),
        ];
        let requests = [
            request(1, 9).with_need(Seconds::new(500)),
            request(2, 1),
            request(3, 1).with_need(Seconds::new(60)), // asked, but never visible
        ];
        let plan = plan(windows, &[], &requests, Scheduler::Greedy);
        let summary = |id: u32| {
            plan.satellites
                .iter()
                .find(|s| s.satellite.get() == id)
                .unwrap()
        };

        let ids: Vec<u32> = plan.satellites.iter().map(|s| s.satellite.get()).collect();
        assert_eq!(ids, [1, 2, 3, 9], "requested and seen satellites, in order");

        assert_eq!(
            (summary(1).passes_found, summary(1).passes_scheduled),
            (2, 1)
        );
        assert_eq!(summary(1).contact_scheduled, Seconds::new(600));
        assert_eq!(summary(1).need_met(), Some(true));

        assert_eq!(
            (summary(2).passes_found, summary(2).passes_scheduled),
            (1, 0)
        );
        assert_eq!(summary(2).need_met(), None, "no limit, so no yes or no");

        assert_eq!(
            (summary(3).passes_found, summary(3).passes_scheduled),
            (0, 0)
        );
        assert_eq!(
            summary(3).need_met(),
            Some(false),
            "asked for contact and got none"
        );

        assert!(summary(9).request.is_none());
        assert_eq!(summary(9).need_met(), None);
        assert_eq!(plan.scheduled_contact(), Seconds::new(600));
    }

    #[test]
    fn station_summaries_cover_idle_stations_too() {
        let windows = vec![w(1, 1, 0, 600), w(2, 1, 300, 900)];
        let stations = [station(1, "Seattle", 45), station(2, "Quito", 0)];
        let plan = plan(
            windows,
            &stations,
            &[request(1, 9), request(2, 1)],
            Scheduler::Greedy,
        );
        assert_eq!(plan.stations.len(), 2);
        assert_eq!(plan.stations[0].name, "Seattle");
        assert_eq!(plan.stations[0].turnaround, Seconds::new(45));
        assert_eq!(
            (
                plan.stations[0].passes_found,
                plan.stations[0].passes_scheduled
            ),
            (2, 1)
        );
        assert_eq!(plan.stations[0].contact_scheduled, Seconds::new(600));
        assert_eq!(
            (
                plan.stations[1].passes_found,
                plan.stations[1].passes_scheduled
            ),
            (0, 0)
        );
    }

    #[test]
    fn value_is_priority_plus_one_times_useful_seconds() {
        // 600 s at priority 9 (weight 10) capped at a need of 500 s, plus nothing for the loser.
        let windows = vec![w(1, 1, 0, 600), w(2, 1, 300, 900)];
        let requests = [request(1, 9).with_need(Seconds::new(500)), request(2, 1)];
        let plan = plan(windows, &[], &requests, Scheduler::Greedy);
        assert_eq!(plan.value, 5_000.0);
    }

    #[test]
    fn the_optimal_plan_is_worth_at_least_the_greedy_plan() {
        let mut rng = Lcg(0x7661_6c75);
        let mut better = 0;
        for _ in 0..60 {
            let instance = random_instance(&mut rng, 40);
            let greedy = plan(
                instance.windows.clone(),
                &[],
                &instance.requests,
                Scheduler::Greedy,
            );
            let optimal = plan(
                instance.windows.clone(),
                &[],
                &instance.requests,
                Scheduler::Optimal,
            );
            assert!(optimal.value >= greedy.value - 1e-6);
            assert_eq!(optimal.entries.len(), greedy.entries.len());
            if optimal.value > greedy.value + 1e-6 {
                better += 1;
            }
        }
        assert!(
            better >= 10,
            "the optimizer beat greedy in only {better} of 60 problems"
        );
    }

    #[test]
    fn a_duplicate_request_is_refused_with_its_code() {
        let error = plan_contacts(
            &survey(vec![w(1, 1, 0, 600)]),
            &[],
            &[],
            &[request(1, 1), request(1, 2)],
            Scheduler::Greedy,
        )
        .unwrap_err();
        assert_eq!(error.reason(), Some(ReasonCode::DuplicateRequest));
        assert!(error.to_string().starts_with("MAAT-204: "), "{error}");
    }

    // --- Incomplete surveys --------------------------------------------------------------------

    fn incomplete_plan() -> ContactPlan {
        let mut survey = survey(vec![w(1, 1, 0, 600)]);
        survey.failures.push(SurveyFailure {
            satellite: SatelliteId::new(70_000),
            station: StationId::new(1),
            error: GroundError::InvalidElevation { degrees: f64::NAN },
        });
        plan_contacts(&survey, &[], &[], &[request(1, 1)], Scheduler::Greedy).unwrap()
    }

    #[test]
    fn survey_failures_make_the_plan_incomplete_and_are_carried_in_full() {
        let plan = incomplete_plan();
        assert!(!plan.is_complete());
        assert_eq!(plan.failures.len(), 1);
        assert!(
            plan.failures[0].starts_with("MAAT-203: "),
            "{}",
            plan.failures[0]
        );
    }

    // --- Text ----------------------------------------------------------------------------------

    fn small_plan() -> ContactPlan {
        let windows = vec![w(1, 1, 0, 600), w(2, 1, 300, 900), w(2, 2, 2_000, 2_450)];
        let stations = [station(1, "Seattle", 30), station(2, "Quito", 0)];
        let requests = [request(1, 9).with_need(Seconds::new(500)), request(2, 1)];
        plan(windows, &stations, &requests, Scheduler::Greedy)
    }

    #[test]
    fn the_text_report_is_stable() {
        // A golden test: the report is read by people and pasted into documents, so a change
        // to its layout should be a deliberate edit of this expected text.
        let expected = "\
Contact plan (greedy scheduler)
  3 passes found: 2 scheduled, 1 not scheduled
  1050 s of scheduled contact, plan value 5900

SCHEDULED
  AOS (UTC)              LENGTH  SATELLITE    STATION       PEAK  PRI
  2026-08-01 12:00:00     600 s  satellite 1  Seattle  90.00 deg    9
  2026-08-01 12:33:20     450 s  satellite 2  Quito    90.00 deg    1

NOT SCHEDULED
  AOS (UTC)              LENGTH  SATELLITE    STATION       PEAK  PRI
  2026-08-01 12:05:00     600 s  satellite 2  Seattle  90.00 deg    1
      MAAT-101: dropped: overlaps a higher-priority pass at the same station
      satellite 1 (priority 9) has Seattle from 2026-08-01 12:00:00 to 2026-08-01 12:10:00; this pass has priority 1

SATELLITES
  NAME         PRI       NEED  SCHEDULED  PASSES  NEED MET
  satellite 1    9      500 s      600 s   1 of 1   yes
  satellite 2    1        all      450 s   1 of 2   -

STATIONS
  NAME     TURNAROUND  SLEW AZ/EL  SCHEDULED  PASSES
  Seattle        30 s           -      600 s   1 of 2
  Quito           0 s           -      450 s   1 of 1
";
        assert_eq!(small_plan().to_text(), expected);
    }

    #[test]
    fn the_text_report_has_no_trailing_spaces() {
        for plan in [small_plan(), incomplete_plan()] {
            for line in plan.to_text().lines() {
                assert_eq!(line, line.trim_end(), "trailing space in {line:?}");
            }
        }
    }

    #[test]
    fn the_text_report_says_none_when_a_section_is_empty() {
        let all_scheduled = plan(
            vec![w(1, 1, 0, 600)],
            &[],
            &[request(1, 1)],
            Scheduler::Optimal,
        );
        let text = all_scheduled.to_text();
        assert!(
            text.starts_with("Contact plan (optimal scheduler)\n"),
            "{text}"
        );
        assert!(text.contains("\nNOT SCHEDULED\n  (none)\n"), "{text}");

        let none_scheduled = plan(vec![w(1, 1, 0, 600)], &[], &[], Scheduler::Greedy);
        assert!(none_scheduled.to_text().contains("\nSCHEDULED\n  (none)\n"));
    }

    #[test]
    fn the_text_report_flags_an_incomplete_plan_at_the_top_and_lists_the_failures() {
        let text = incomplete_plan().to_text();
        let third_line = text.lines().nth(3).unwrap();
        assert_eq!(
            third_line,
            "  INCOMPLETE: 1 satellite/station pairs could not be searched (listed below)"
        );
        assert!(text.contains("\nNOT SEARCHED\n  MAAT-203: "), "{text}");
    }

    // --- JSON ----------------------------------------------------------------------------------

    #[test]
    fn json_parses_and_carries_the_schema_scheduler_and_totals() {
        let plan = small_plan();
        let json: Value = serde_json::from_str(&plan.to_json()).expect("the output is valid JSON");
        assert_eq!(json["schema"], "maat.contact-plan/1");
        assert_eq!(json["scheduler"], "greedy");
        assert_eq!(json["complete"], true);
        assert_eq!(json["totals"]["passes"], 3);
        assert_eq!(json["totals"]["scheduled"], 2);
        assert_eq!(json["totals"]["not_scheduled"], 1);
        assert_eq!(json["totals"]["contact_scheduled_s"], 1_050);
        assert_eq!(json["totals"]["value"], 5_900.0);
        assert_eq!(json["failures"], json!([]));
    }

    #[test]
    fn json_entries_carry_the_reason_as_code_number_class_and_summary() {
        let json = small_plan().to_json_value();
        let dropped = &json["entries"][1];
        assert_eq!(
            *dropped,
            json!({
                "index": 1,
                "satellite": { "id": 2, "name": "satellite 2" },
                "station": { "id": 1, "name": "Seattle" },
                "aos": "2026-08-01T12:05:00.000Z",
                "los": "2026-08-01T12:15:00.000Z",
                "duration_s": 600.0,
                "max_elevation_deg": 90.0,
                "pointing": null,
                "priority": 1,
                "scheduled": false,
                "decided_by": "scheduler",
                "reason": {
                    "code": "MAAT-101",
                    "number": 101,
                    "class": "rejected",
                    "summary": "dropped: overlaps a higher-priority pass at the same station",
                },
                "detail": "satellite 1 (priority 9) has Seattle from 2026-08-01 12:00:00 to \
                           2026-08-01 12:10:00; this pass has priority 1",
                "blocked_by": 0,
            })
        );
        assert_eq!(json["entries"][0]["scheduled"], true);
        assert_eq!(json["entries"][0]["blocked_by"], Value::Null);
        assert_eq!(json["entries"][0]["reason"]["code"], "MAAT-001");
    }

    #[test]
    fn json_summaries_use_null_for_no_limit_and_no_request() {
        let windows = vec![
            w(1, 1, 0, 600),
            w(2, 1, 1_000, 1_300),
            w(9, 1, 2_000, 2_100),
        ];
        let requests = [request(1, 9).with_need(Seconds::new(500)), request(2, 1)];
        let json = plan(
            windows,
            &[station(1, "Seattle", 30)],
            &requests,
            Scheduler::Greedy,
        )
        .to_json_value();

        assert_eq!(
            json["satellites"][0],
            json!({
                "id": 1, "name": "satellite 1",
                "request": { "priority": 9, "min_contact_s": 0, "need_s": 500 },
                "passes_found": 1, "passes_scheduled": 1,
                "contact_scheduled_s": 600, "floor_s": null, "need_met": true,
            })
        );
        assert_eq!(json["satellites"][1]["request"]["need_s"], Value::Null);
        assert_eq!(json["satellites"][1]["need_met"], Value::Null);
        assert_eq!(json["satellites"][2]["request"], Value::Null);
        assert_eq!(
            json["stations"][0],
            json!({
                "id": 1, "name": "Seattle", "turnaround_s": 30, "slew_rates_deg_per_s": null,
                "passes_found": 3, "passes_scheduled": 2, "contact_scheduled_s": 900,
            })
        );
    }

    #[test]
    fn json_carries_pointing_and_slew_rates_when_they_are_known() {
        let plan = plan(
            vec![p(1, 1, 0, 600, 4_550, 31_025), w(2, 1, 2_000, 2_300)],
            &[rotating(1, "Seattle", 45)],
            &[request(1, 9), request(2, 1)],
            Scheduler::Greedy,
        );
        let json: Value = serde_json::from_str(&plan.to_json()).unwrap();
        assert_eq!(
            json["entries"][0]["pointing"],
            json!({
                "aos": { "azimuth_deg": 45.5, "elevation_deg": 10.0 },
                "los": { "azimuth_deg": 310.25, "elevation_deg": 10.0 },
            })
        );
        assert_eq!(json["entries"][1]["pointing"], Value::Null);
        assert_eq!(
            json["stations"][0]["slew_rates_deg_per_s"],
            json!({ "azimuth": 6.0, "elevation": 2.5 })
        );
        assert_eq!(plan.stations[0].slew_rates, plan_rule(&plan).slew_rates());
        assert!(plan.to_text().contains(
            "  NAME     TURNAROUND       SLEW AZ/EL  SCHEDULED  PASSES\n  \
               Seattle        45 s  6.00/2.50 deg/s"
        ));
    }

    /// The rule of the one station in a one-station plan built with [`rotating`].
    fn plan_rule(plan: &ContactPlan) -> StationRule {
        assert_eq!(plan.stations.len(), 1);
        rotating(1, "Seattle", 45).rule()
    }

    #[test]
    fn json_blocked_by_indexes_into_entries() {
        let mut rng = Lcg(0x6a73_6f6e);
        for _ in 0..30 {
            let instance = random_instance(&mut rng, 30);
            let json =
                plan(instance.windows, &[], &instance.requests, Scheduler::Greedy).to_json_value();
            let entries = json["entries"].as_array().unwrap();
            for (position, entry) in entries.iter().enumerate() {
                assert_eq!(entry["index"], position);
                if let Some(blocker) = entry["blocked_by"].as_u64() {
                    let blocker = &entries[usize::try_from(blocker).unwrap()];
                    assert_eq!(blocker["scheduled"], true);
                }
            }
        }
    }

    #[test]
    fn json_reports_an_incomplete_plan() {
        let json = incomplete_plan().to_json_value();
        assert_eq!(json["complete"], false);
        assert_eq!(json["failures"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn json_is_deterministic() {
        assert_eq!(small_plan().to_json(), small_plan().to_json());
    }

    #[test]
    fn json_escapes_awkward_names() {
        let windows = vec![w(1, 1, 0, 600)];
        let awkward = station(1, "Dish \"A\" \\ north\nfield", 0);
        let text = plan(windows, &[awkward], &[request(1, 1)], Scheduler::Greedy).to_json();
        let json: Value = serde_json::from_str(&text).expect("still valid JSON");
        assert_eq!(
            json["entries"][0]["station"]["name"],
            "Dish \"A\" \\ north\nfield"
        );
    }

    #[test]
    fn entry_for_finds_each_window() {
        let a = w(1, 1, 0, 600);
        let plan = plan(vec![a], &[], &[request(1, 1)], Scheduler::Greedy);
        assert!(entry_for(&plan, &a).is_scheduled());
    }
}
