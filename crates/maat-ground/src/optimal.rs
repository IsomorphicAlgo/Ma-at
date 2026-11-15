//! The optimal contact scheduler: an integer linear program, solved exactly.
//!
//! The greedy scheduler in `maat-flight` takes the best window first and never looks back. This
//! one considers every window at once and returns the plan with the greatest total **value**.
//! Comparing the two shows what greedy leaves on the table.
//!
//! ## What is maximized
//!
//! ```text
//! value  =  sum over satellites of   weight(satellite) x useful contact seconds
//!
//! weight(satellite)        =  priority + 1
//! useful contact seconds   =  min(scheduled contact, need)
//! ```
//!
//! Contact beyond a satellite's need is worth nothing. The weight is `priority + 1` so that
//! priority 0 still counts.
//!
//! **This is a different meaning of priority from the greedy scheduler's.** Greedy treats
//! priority as a strict rank: a higher priority always wins a contested station. Here priority is
//! a weight: one second for a priority-9 satellite is worth as much as ten seconds for a
//! priority-0 satellite, and no more, so a long low-priority pass can outweigh a short
//! high-priority one. When every satellite has the same priority the two meanings agree, and the
//! value is simply useful contact time.
//!
//! ## The model
//!
//! One yes/no variable per candidate window. The constraints are the same rules the greedy
//! scheduler enforces, stated through the same [`conflict`] function:
//!
//! - a station serves one pass at a time, with the time it needs between passes;
//! - a satellite uses one station at a time.
//!
//! Most of this is written as *clique* constraints: for every instant a pass begins, at most one
//! of the passes in progress at that station (or within its fixed turnaround time, or for that
//! satellite) may be chosen. This is much tighter than one constraint per conflicting pair, and
//! it is why the solver usually finds a whole-number answer without having to branch.
//!
//! The time an antenna needs to turn from one pass to the next depends on the two passes, so it
//! does not fit that pattern. Pairs that are too close only because of that slew time each get
//! a constraint of their own.
//!
//! ## A floor for each satellite
//!
//! Left to itself the solver looks only at the total. It can take contact from one satellite to
//! give more to others, and between two satellites of equal weight it has no preference at all.
//! With a floor, one more constraint per satellite requires its useful contact to be at least
//! what the greedy plan gives it. The plan can then improve on the greedy one only where no
//! satellite pays for it. The greedy plan always satisfies the floor, so there is always a plan
//! to return. "Proven optimal" then means: no plan *that respects the floor* is worth more.
//!
//! ## Starting point and time limit
//!
//! The solver starts from the greedy plan, and has a time limit. Two things follow:
//!
//! - **The result is never worth less than the greedy plan.** If the solver has found nothing
//!   better when it stops, the greedy plan is what it returns.
//! - **The result is not always proven best.** If the limit is reached, the plan is the best
//!   the solver had found, and it is marked as not proven optimal.
//!
//! ## After the solver
//!
//! The solver's answer is checked and tidied before it is trusted:
//!
//! 1. it must contain no conflict, or an error is returned;
//! 2. passes that add nothing (the satellite's need is met without them) are removed;
//! 3. any usable pass still left out is added.
//!
//! Steps 2 and 3 never lower the value. They make sure every pass that is not in the plan has a
//! concrete reason: its satellite's need is met (`MAAT-104`), or it conflicts with a pass that
//! is in the plan (`MAAT-109`).
//!
//! ## Limits
//!
//! - **Run time is bounded only by the time limit.** This is an NP-hard problem in general. The
//!   solver is a pure-Rust branch-and-bound (`microlp`, through `good_lp`). Problems of a few
//!   hundred to a few thousand windows solve in well under a second in practice; an adversarial
//!   problem could use the whole limit. Use the greedy scheduler when a hard bound matters.
//! - **A plan cut short by the time limit depends on the machine.** A faster computer gets
//!   further in the same time, so two runs can differ. A plan that is proven optimal does not
//!   have this problem.
//! - **Ground only.** It allocates, uses floating point and depends on a solver.

use std::cmp::Ordering;
use std::time::Duration;

use good_lp::{
    Expression, ProblemVariables, ResolutionError, Solution, SolutionStatus, SolverModel, Variable,
    WithTimeLimit, constraint, microlp, variable,
};
#[cfg(test)]
use maat_flight::Seconds;
use maat_flight::schedule::{conflict, rank, rule_for, schedule_greedy, screen, validate};
use maat_flight::{ContactWindow, Decision, ReasonCode, SatelliteRequest, StationRule};

use crate::error::GroundError;

/// A scheduler's verdict on one window: the reason, and for a window that lost to another, the
/// position of that other window in the input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Verdict {
    pub(crate) reason: ReasonCode,
    pub(crate) blocked_by: Option<usize>,
}

impl Verdict {
    pub(crate) fn is_scheduled(&self) -> bool {
        self.reason == ReasonCode::Scheduled
    }
}

/// What a second of contact is worth for a satellite of the given request.
pub(crate) fn weight(request: &SatelliteRequest) -> f64 {
    f64::from(request.priority().get()) + 1.0
}

/// The value of a plan, as defined in the [module docs](self).
///
/// `scheduled` says, for each window, whether it is in the plan.
pub(crate) fn plan_value(
    windows: &[ContactWindow],
    scheduled: &[bool],
    requests: &[SatelliteRequest],
) -> f64 {
    requests
        .iter()
        .map(|request| {
            let useful = useful_seconds(windows, scheduled, request);
            // Contact totals are far below 2^53, so the conversion is exact.
            weight(request) * useful as f64
        })
        .sum()
}

/// Total scheduled contact for one request's satellite, in seconds.
fn granted_seconds(
    windows: &[ContactWindow],
    scheduled: &[bool],
    request: &SatelliteRequest,
) -> u64 {
    windows
        .iter()
        .zip(scheduled)
        .filter(|(window, chosen)| **chosen && window.satellite() == request.satellite())
        .map(|(window, _)| u64::from(window.duration().get()))
        .sum()
}

/// Whether `granted` seconds satisfy the request. Mirrors `SatelliteRequest::is_met_by` for
/// totals that are too large for a `Seconds`.
fn need_met(request: &SatelliteRequest, granted: u64) -> bool {
    !request.is_unlimited() && granted >= u64::from(request.need().get())
}

/// The useful contact a plan gives one request's satellite, in seconds: its scheduled contact,
/// counted only up to its need.
pub(crate) fn useful_seconds(
    windows: &[ContactWindow],
    scheduled: &[bool],
    request: &SatelliteRequest,
) -> u64 {
    let granted = granted_seconds(windows, scheduled, request);
    if request.is_unlimited() {
        granted
    } else {
        granted.min(u64::from(request.need().get()))
    }
}

/// The longest time limit passed to the solver. A longer one is treated as this.
const LONGEST_TIME_LIMIT: Duration = Duration::from_secs(366 * 24 * 60 * 60);

/// What the optimal scheduler returns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OptimalOutcome {
    /// One verdict per window, in the order of the windows.
    pub(crate) verdicts: Vec<Verdict>,
    /// `true` if the solver proved that no plan is worth more. `false` if it stopped at its
    /// time limit: the plan is then the best it had found, and at least as good as greedy.
    pub(crate) proven_optimal: bool,
    /// With a per-satellite floor: for each request, in order, the useful contact in seconds
    /// that the plan was required to give its satellite. `None` without a floor.
    pub(crate) floors: Option<Vec<u64>>,
}

/// Schedules contact windows for the greatest total value. See the [module docs](self).
///
/// The solver stops after `time_limit`. Whatever happens, the plan returned is free of
/// conflicts and worth at least as much as the greedy plan.
///
/// With `satellite_floor`, the plan must also give every satellite at least the useful contact
/// the greedy plan gives it. See the [module docs](self).
pub(crate) fn schedule_optimal(
    windows: &[ContactWindow],
    requests: &[SatelliteRequest],
    rules: &[StationRule],
    time_limit: Duration,
    satellite_floor: bool,
) -> Result<OptimalOutcome, GroundError> {
    validate(requests, rules)?;

    // Screen exactly as the greedy scheduler does. `request_of[i]` is the request for a window
    // that is a candidate; a window that is not one already has its verdict.
    let mut verdicts: Vec<Option<Verdict>> = Vec::with_capacity(windows.len());
    let mut request_of: Vec<Option<&SatelliteRequest>> = Vec::with_capacity(windows.len());
    for window in windows {
        match screen(window, requests) {
            Ok(request) => {
                verdicts.push(None);
                request_of.push(Some(request));
            }
            Err(reason) => {
                verdicts.push(Some(Verdict {
                    reason,
                    blocked_by: None,
                }));
                request_of.push(None);
            }
        }
    }
    let candidates: Vec<usize> = (0..windows.len())
        .filter(|&index| request_of[index].is_some())
        .collect();

    // The greedy plan: where the solver starts, and what it falls back to.
    let greedy = greedy_choice(windows, requests, rules)?;
    let greedy_value = plan_value(windows, &greedy, requests);
    // What the greedy plan gives each satellite, which a floor makes the least it may get.
    let floors: Option<Vec<u64>> = satellite_floor.then(|| {
        requests
            .iter()
            .map(|request| useful_seconds(windows, &greedy, request))
            .collect()
    });

    let solved = solve(
        windows,
        &candidates,
        requests,
        rules,
        &greedy,
        floors.as_deref(),
        time_limit,
    )?;
    let (mut chosen, mut proven_optimal) = match solved {
        Some((chosen, proven)) => {
            check_no_conflicts(windows, &chosen, rules)?;
            (chosen, proven)
        }
        // The time limit passed before the solver had any plan of its own.
        None => (greedy.clone(), false),
    };
    // An unfinished search must never cost anything: keep the greedy plan if it is worth more.
    if plan_value(windows, &chosen, requests) < greedy_value {
        chosen = greedy.clone();
        proven_optimal = false;
    }
    tidy(
        windows,
        &candidates,
        &request_of,
        requests,
        rules,
        &mut chosen,
    );
    // The solver was told to respect the floor, and tidying never lowers a satellite's useful
    // contact. This is the check that it held; if it ever did not, the greedy plan, which meets
    // its own floor by definition, is used.
    if let Some(floors) = &floors {
        let held = requests
            .iter()
            .zip(floors)
            .all(|(request, &floor)| useful_seconds(windows, &chosen, request) >= floor);
        if !held {
            chosen = greedy;
            proven_optimal = false;
        }
    }

    // Explain every candidate that is not in the plan.
    for &index in &candidates {
        let request = request_of[index].expect("a candidate has a request");
        let verdict = if chosen[index] {
            Verdict {
                reason: ReasonCode::Scheduled,
                blocked_by: None,
            }
        } else if need_met(request, granted_seconds(windows, &chosen, request)) {
            Verdict {
                reason: ReasonCode::NeedAlreadyMet,
                blocked_by: None,
            }
        } else {
            match first_blocker(windows, &chosen, rules, index) {
                Some(blocker) => Verdict {
                    reason: ReasonCode::OutweighedByPlan,
                    blocked_by: Some(blocker),
                },
                // `tidy` adds every usable pass, so this cannot happen. If it ever did, saying
                // so is better than inventing a reason.
                None => {
                    return Err(GroundError::Solver {
                        message: format!(
                            "window {index} was left out of the plan although nothing rules it out"
                        ),
                    });
                }
            }
        };
        verdicts[index] = Some(verdict);
    }

    Ok(OptimalOutcome {
        verdicts: verdicts
            .into_iter()
            .map(|verdict| verdict.expect("every window has a verdict"))
            .collect(),
        proven_optimal,
        floors,
    })
}

/// Which windows the greedy scheduler would put in the plan.
fn greedy_choice(
    windows: &[ContactWindow],
    requests: &[SatelliteRequest],
    rules: &[StationRule],
) -> Result<Vec<bool>, GroundError> {
    let mut decisions = vec![Decision::PENDING; windows.len()];
    schedule_greedy(windows, requests, rules, &mut decisions)?;
    Ok(decisions.iter().map(Decision::is_scheduled).collect())
}

/// Builds and solves the integer program, starting from the plan in `start`.
///
/// Returns, for each window, whether it was chosen, and whether the choice is proven optimal.
/// Returns `None` if the time limit passed before the solver had a plan.
fn solve(
    windows: &[ContactWindow],
    candidates: &[usize],
    requests: &[SatelliteRequest],
    rules: &[StationRule],
    start: &[bool],
    floors: Option<&[u64]>,
    time_limit: Duration,
) -> Result<Option<(Vec<bool>, bool)>, GroundError> {
    let mut chosen = vec![false; windows.len()];
    if candidates.is_empty() {
        return Ok(Some((chosen, true)));
    }
    let as_number = |flag: bool| if flag { 1.0 } else { 0.0 };

    let mut variables = ProblemVariables::new();
    // `pick[i]` is the yes/no variable for window `i`, for candidates only.
    let mut pick: Vec<Option<Variable>> = vec![None; windows.len()];
    for &index in candidates {
        pick[index] = Some(variables.add(variable().binary().initial(as_number(start[index]))));
    }

    // Objective, and one "useful contact" variable for each satellite whose need can bind.
    let mut objective = Expression::from(0.0);
    let mut caps: Vec<(Variable, Expression)> = Vec::new();
    // With a floor: an expression for each satellite's useful contact, and the least it may be.
    let mut floored: Vec<(Expression, f64)> = Vec::new();
    for (position, request) in requests.iter().enumerate() {
        let floor = floors
            .and_then(|floors| floors.get(position))
            .copied()
            .filter(|&floor| floor > 0);
        let mine: Vec<usize> = candidates
            .iter()
            .copied()
            .filter(|&index| windows[index].satellite() == request.satellite())
            .collect();
        if mine.is_empty() {
            continue;
        }
        let mut contact = Expression::from(0.0);
        let mut available: u64 = 0;
        for &index in &mine {
            let seconds = windows[index].duration().get();
            available += u64::from(seconds);
            contact += f64::from(seconds) * pick[index].expect("a candidate has a variable");
        }
        if need_met(request, available) {
            // The need can be reached, so contact beyond it must earn nothing:
            // useful <= need, useful <= contact, and the objective counts `useful`.
            let need = f64::from(request.need().get());
            let started_with = (granted_seconds(windows, start, request) as f64).min(need);
            let useful = variables.add(variable().min(0.0).max(need).initial(started_with));
            objective += weight(request) * useful;
            if let Some(floor) = floor {
                floored.push((Expression::from(useful), floor as f64));
            }
            caps.push((useful, contact));
        } else {
            // Every second counts: the need is unlimited or out of reach.
            if let Some(floor) = floor {
                floored.push((contact.clone(), floor as f64));
            }
            objective += weight(request) * contact;
        }
    }

    let limit = time_limit.min(LONGEST_TIME_LIMIT).as_secs_f64();
    let mut model = variables
        .maximise(objective)
        .using(microlp)
        .with_time_limit(limit);
    for (useful, contact) in caps {
        model = model.with(constraint!(useful <= contact));
    }
    for (useful, floor) in floored {
        model = model.with(constraint!(useful >= floor));
    }
    for clique in cliques(windows, candidates, rules) {
        let mut together = Expression::from(0.0);
        for index in clique {
            together += pick[index].expect("a candidate has a variable");
        }
        model = model.with(constraint!(together <= 1.0));
    }

    let solution = match model.solve() {
        Ok(solution) => solution,
        // The one error that is not a failure: the limit passed with no plan in hand.
        Err(ResolutionError::Other(message)) if message.contains("Time limit") => {
            return Ok(None);
        }
        Err(error) => {
            return Err(GroundError::Solver {
                message: error.to_string(),
            });
        }
    };
    let proven = is_proven(solution.status());
    for &index in candidates {
        chosen[index] = solution.value(pick[index].expect("a candidate has a variable")) > 0.5;
    }
    Ok(Some((chosen, proven)))
}

/// Whether a solver status means the plan is proven to be the best.
///
/// Only [`SolutionStatus::Optimal`] does. A plan returned because a limit was reached is the
/// best found so far, not the best there is.
fn is_proven(status: SolutionStatus) -> bool {
    match status {
        SolutionStatus::Optimal => true,
        SolutionStatus::TimeLimit | SolutionStatus::GapLimit => false,
    }
}

/// The sets of windows of which at most one may be scheduled.
///
/// For each candidate window `w`, taken as "the instant `w` begins":
///
/// - **station clique**: every candidate at `w`'s station that is in progress at that instant,
///   or ended less than the station's fixed turnaround time before it;
/// - **satellite clique**: every candidate of `w`'s satellite, at any station, in progress at
///   that instant.
///
/// Any two windows that overlap, or are closer than the fixed turnaround, appear together in the
/// clique of whichever begins later. What is left is pairs at one station that are too close
/// only because of the time the antenna needs to turn from one to the other. That time differs
/// from pair to pair, so each such pair is added as a set of two.
///
/// Together the sets rule out exactly the pairs for which [`conflict`] returns a conflict.
/// Sets of one are dropped.
fn cliques(
    windows: &[ContactWindow],
    candidates: &[usize],
    rules: &[StationRule],
) -> Vec<Vec<usize>> {
    let mut cliques = Vec::new();
    for &anchor in candidates {
        let start = windows[anchor].aos();
        let turnaround = rule_for(rules, windows[anchor].station()).turnaround();

        let station: Vec<usize> = candidates
            .iter()
            .copied()
            .filter(|&other| {
                let w = &windows[other];
                w.station() == windows[anchor].station()
                    && w.aos() <= start
                    // In progress at `start`, or ended within the turnaround before it. An end
                    // too far in the future to add the turnaround to certainly covers `start`.
                    && w.los().checked_add(turnaround).is_none_or(|free| start < free)
            })
            .collect();
        if station.len() > 1 {
            cliques.push(station);
        }

        let satellite: Vec<usize> = candidates
            .iter()
            .copied()
            .filter(|&other| {
                let w = &windows[other];
                w.satellite() == windows[anchor].satellite() && w.contains(start)
            })
            .collect();
        if satellite.len() > 1 {
            cliques.push(satellite);
        }
    }

    // Pairs that conflict only once the slew between them is counted.
    for (position, &a) in candidates.iter().enumerate() {
        for &b in &candidates[position + 1..] {
            if windows[a].station() != windows[b].station() {
                continue;
            }
            let rule = rule_for(rules, windows[a].station());
            let fixed_only = StationRule::new(rule.station(), rule.turnaround());
            if conflict(&windows[a], &windows[b], &rule).is_some()
                && conflict(&windows[a], &windows[b], &fixed_only).is_none()
            {
                cliques.push(vec![a, b]);
            }
        }
    }

    cliques.sort();
    cliques.dedup();
    cliques
}

/// The first scheduled window that conflicts with `index`, if any.
fn first_blocker(
    windows: &[ContactWindow],
    chosen: &[bool],
    rules: &[StationRule],
    index: usize,
) -> Option<usize> {
    let window = &windows[index];
    let rule = rule_for(rules, window.station());
    (0..windows.len()).find(|&other| {
        other != index && chosen[other] && conflict(window, &windows[other], &rule).is_some()
    })
}

/// Fails if the solver's answer schedules two windows that conflict.
fn check_no_conflicts(
    windows: &[ContactWindow],
    chosen: &[bool],
    rules: &[StationRule],
) -> Result<(), GroundError> {
    for index in (0..windows.len()).filter(|&index| chosen[index]) {
        if let Some(other) = first_blocker(windows, chosen, rules, index) {
            return Err(GroundError::Solver {
                message: format!(
                    "the solver scheduled windows {index} and {other}, which conflict"
                ),
            });
        }
    }
    Ok(())
}

/// Removes passes that add no value and adds usable passes that were left out.
///
/// Neither step can lower the plan's value. They repeat until nothing changes; each round that
/// changes something either raises the value or shortens the plan, so the loop ends.
fn tidy(
    windows: &[ContactWindow],
    candidates: &[usize],
    request_of: &[Option<&SatelliteRequest>],
    requests: &[SatelliteRequest],
    rules: &[StationRule],
    chosen: &mut [bool],
) {
    // Candidates from best to worst, by the same ranking the greedy scheduler uses.
    let mut ranked = candidates.to_vec();
    ranked.sort_by(|&a, &b| compare(windows, request_of, a, b));

    for _ in 0..=candidates.len() {
        let mut changed = false;

        // Remove surplus passes, worst first: a pass is surplus if its satellite's need is
        // still met without it.
        for request in requests.iter().filter(|r| !r.is_unlimited()) {
            for &index in ranked.iter().rev() {
                if !chosen[index] || windows[index].satellite() != request.satellite() {
                    continue;
                }
                let granted = granted_seconds(windows, chosen, request);
                let without = granted - u64::from(windows[index].duration().get());
                if need_met(request, without) {
                    chosen[index] = false;
                    changed = true;
                }
            }
        }

        // Add usable passes, best first: wanted, and in conflict with nothing in the plan.
        for &index in &ranked {
            if chosen[index] {
                continue;
            }
            let request = request_of[index].expect("a candidate has a request");
            let wanted = !need_met(request, granted_seconds(windows, chosen, request));
            if wanted && first_blocker(windows, chosen, rules, index).is_none() {
                chosen[index] = true;
                changed = true;
            }
        }

        if !changed {
            break;
        }
    }
}

/// The greedy ranking, applied to two candidate windows by position.
fn compare(
    windows: &[ContactWindow],
    request_of: &[Option<&SatelliteRequest>],
    a: usize,
    b: usize,
) -> Ordering {
    let priority = |index: usize| {
        request_of[index]
            .expect("a candidate has a request")
            .priority()
    };
    rank(priority(a), &windows[a], priority(b), &windows[b]).then(a.cmp(&b))
}

/// The greatest value any conflict-free plan can have, found by trying every subset.
///
/// Exponential, so for tests on small problems only. It shares the definitions of conflict and
/// value with the solver but none of its method.
#[cfg(test)]
pub(crate) fn brute_force_value(
    windows: &[ContactWindow],
    requests: &[SatelliteRequest],
    rules: &[StationRule],
) -> f64 {
    brute_force_value_above(windows, requests, rules, None)
}

/// As [`brute_force_value`], counting only plans that give each request's satellite at least
/// the useful contact in `floors` (in the order of `requests`).
#[cfg(test)]
pub(crate) fn brute_force_value_above(
    windows: &[ContactWindow],
    requests: &[SatelliteRequest],
    rules: &[StationRule],
    floors: Option<&[u64]>,
) -> f64 {
    let candidates: Vec<usize> = (0..windows.len())
        .filter(|&index| screen(&windows[index], requests).is_ok())
        .collect();
    assert!(candidates.len() <= 20, "too many windows to enumerate");
    let mut best = 0.0_f64;
    for mask in 0_u32..(1 << candidates.len()) {
        let mut chosen = vec![false; windows.len()];
        for (bit, &index) in candidates.iter().enumerate() {
            chosen[index] = mask & (1 << bit) != 0;
        }
        let above_floor = floors.is_none_or(|floors| {
            requests
                .iter()
                .zip(floors)
                .all(|(request, &floor)| useful_seconds(windows, &chosen, request) >= floor)
        });
        if above_floor && check_no_conflicts(windows, &chosen, rules).is_ok() {
            best = best.max(plan_value(windows, &chosen, requests));
        }
    }
    best
}

/// The contact time a `Seconds` total would report, for tests that compare with the flight
/// scheduler's summary.
#[cfg(test)]
pub(crate) fn total_contact(windows: &[ContactWindow], verdicts: &[Verdict]) -> Seconds {
    windows
        .iter()
        .zip(verdicts)
        .filter(|(_, verdict)| verdict.is_scheduled())
        .fold(Seconds::ZERO, |total, (window, _)| {
            total.saturating_add(window.duration())
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::{Instance, Lcg, pointed, random_instance, request, rule, window};
    use maat_flight::SlewRates;

    // --- Helpers -------------------------------------------------------------------------------

    /// Long enough that no test problem comes near it.
    const AMPLE: Duration = Duration::from_secs(600);

    fn optimal(
        windows: &[ContactWindow],
        requests: &[SatelliteRequest],
        rules: &[StationRule],
    ) -> Vec<Verdict> {
        let outcome =
            schedule_optimal(windows, requests, rules, AMPLE, false).expect("the solver succeeds");
        assert!(
            outcome.proven_optimal,
            "with ample time the plan is proven optimal"
        );
        outcome.verdicts
    }

    fn reasons(verdicts: &[Verdict]) -> Vec<ReasonCode> {
        verdicts.iter().map(|v| v.reason).collect()
    }

    fn value_of(instance: &Instance, verdicts: &[Verdict]) -> f64 {
        let chosen: Vec<bool> = verdicts.iter().map(Verdict::is_scheduled).collect();
        plan_value(&instance.windows, &chosen, &instance.requests)
    }

    fn greedy(instance: &Instance) -> Vec<Verdict> {
        let mut decisions = vec![Decision::PENDING; instance.windows.len()];
        schedule_greedy(
            &instance.windows,
            &instance.requests,
            &instance.rules,
            &mut decisions,
        )
        .unwrap();
        decisions
            .iter()
            .map(|d| Verdict {
                reason: d.reason().unwrap(),
                blocked_by: d.blocked_by(),
            })
            .collect()
    }

    /// Checks an optimal plan against the rules, independently of how it was found: no
    /// conflicts, and every stated reason true.
    fn assert_plan_is_sound(instance: &Instance, verdicts: &[Verdict]) {
        let Instance {
            windows,
            requests,
            rules,
        } = instance;
        let chosen: Vec<bool> = verdicts.iter().map(Verdict::is_scheduled).collect();
        assert_eq!(verdicts.len(), windows.len());
        check_no_conflicts(windows, &chosen, rules).expect("no two scheduled windows conflict");

        for (index, (window, verdict)) in windows.iter().zip(verdicts).enumerate() {
            let screened = screen(window, requests);
            match verdict.reason {
                ReasonCode::NoRequest | ReasonCode::BelowMinimumDuration => {
                    assert_eq!(screened.err(), Some(verdict.reason), "window {index}");
                    assert_eq!(verdict.blocked_by, None);
                }
                ReasonCode::Scheduled => {
                    let request = screened.expect("a scheduled window passed screening");
                    assert_eq!(verdict.blocked_by, None);
                    // Not surplus: without this pass the need would not be met.
                    let granted = granted_seconds(windows, &chosen, request);
                    let without = granted - u64::from(window.duration().get());
                    assert!(!need_met(request, without), "window {index} is surplus");
                }
                ReasonCode::NeedAlreadyMet => {
                    let request = screened.expect("a candidate");
                    assert!(
                        need_met(request, granted_seconds(windows, &chosen, request)),
                        "window {index}: the need is not met"
                    );
                }
                ReasonCode::OutweighedByPlan => {
                    let blocker = verdict.blocked_by.expect("MAAT-109 names its blocker");
                    assert!(
                        chosen[blocker],
                        "window {index} is blocked by an unscheduled pass"
                    );
                    let rule = rule_for(rules, window.station());
                    assert!(
                        conflict(window, &windows[blocker], &rule).is_some(),
                        "window {index} does not conflict with its blocker {blocker}"
                    );
                }
                other => panic!("window {index} got {other:?}, which this scheduler never emits"),
            }
        }
    }

    // --- Small cases with known answers --------------------------------------------------------

    #[test]
    fn nothing_to_schedule_is_an_empty_plan() {
        assert!(optimal(&[], &[], &[]).is_empty());
        assert!(optimal(&[], &[request(1, 1)], &[rule(1, 60)]).is_empty());
    }

    #[test]
    fn windows_that_do_not_compete_are_all_scheduled() {
        let windows = [
            window(1, 1, 0, 600),
            window(2, 2, 0, 600),
            window(1, 1, 600, 900),
        ];
        let verdicts = optimal(&windows, &[request(1, 1), request(2, 1)], &[]);
        assert_eq!(reasons(&verdicts), [ReasonCode::Scheduled; 3]);
    }

    #[test]
    fn two_short_passes_beat_one_long_pass_that_blocks_both() {
        // The case greedy gets wrong (see the flight scheduler's tests): 300 + 300 > 400.
        let windows = [
            window(1, 1, 0, 300),
            window(2, 1, 250, 650),
            window(3, 1, 600, 900),
        ];
        let requests = [request(1, 5), request(2, 5), request(3, 5)];
        let verdicts = optimal(&windows, &requests, &[]);
        assert_eq!(
            reasons(&verdicts),
            [
                ReasonCode::Scheduled,
                ReasonCode::OutweighedByPlan,
                ReasonCode::Scheduled
            ]
        );
        // The loser is told which pass it lost to.
        assert!(matches!(verdicts[1].blocked_by, Some(0 | 2)));
    }

    #[test]
    fn priority_is_a_weight_not_a_strict_rank() {
        // 10 s at priority 9 is worth 10 x 10 = 100. 900 s at priority 0 is worth 900 x 1.
        // The optimizer keeps the long low-priority pass; greedy would keep the short one.
        let windows = [window(1, 1, 0, 900), window(2, 1, 400, 410)];
        let requests = [request(1, 0), request(2, 9)];
        let verdicts = optimal(&windows, &requests, &[]);
        assert_eq!(
            reasons(&verdicts),
            [ReasonCode::Scheduled, ReasonCode::OutweighedByPlan]
        );

        // Make the short pass long enough to be worth more (100 s x 10 = 1000 > 900) and the
        // decision flips.
        let windows = [window(1, 1, 0, 900), window(2, 1, 400, 500)];
        let verdicts = optimal(&windows, &requests, &[]);
        assert_eq!(
            reasons(&verdicts),
            [ReasonCode::OutweighedByPlan, ReasonCode::Scheduled]
        );
    }

    #[test]
    fn contact_beyond_a_need_is_worth_nothing() {
        // Satellite 1 needs 300 s and has two passes; satellite 2 competes for the second.
        // Giving satellite 1 both would add nothing, so satellite 2 gets the station.
        let windows = [
            window(1, 1, 0, 300),
            window(1, 1, 1_000, 1_400),
            window(2, 1, 1_100, 1_300),
        ];
        let requests = [request(1, 9).with_need(Seconds::new(300)), request(2, 0)];
        let verdicts = optimal(&windows, &requests, &[]);
        assert_eq!(
            reasons(&verdicts),
            [
                ReasonCode::Scheduled,
                ReasonCode::NeedAlreadyMet,
                ReasonCode::Scheduled
            ]
        );
    }

    #[test]
    fn a_surplus_pass_is_removed_even_when_nothing_competes_for_it() {
        // Nothing else wants the station, but the plan should still not contain a pass that
        // adds no value. The shorter pass is the one dropped: the longer one meets the need.
        let windows = [window(1, 1, 0, 200), window(1, 1, 1_000, 1_600)];
        let requests = [request(1, 5).with_need(Seconds::new(500))];
        let verdicts = optimal(&windows, &requests, &[]);
        assert_eq!(
            reasons(&verdicts),
            [ReasonCode::NeedAlreadyMet, ReasonCode::Scheduled]
        );
    }

    #[test]
    fn a_need_of_zero_schedules_nothing() {
        let windows = [window(1, 1, 0, 300)];
        let requests = [request(1, 5).with_need(Seconds::ZERO)];
        assert_eq!(
            reasons(&optimal(&windows, &requests, &[])),
            [ReasonCode::NeedAlreadyMet]
        );
    }

    #[test]
    fn turnaround_time_is_respected() {
        let windows = [window(1, 1, 0, 600), window(2, 1, 630, 900)];
        let requests = [request(1, 5), request(2, 5)];
        assert_eq!(
            reasons(&optimal(&windows, &requests, &[rule(1, 30)])),
            [ReasonCode::Scheduled; 2]
        );
        let tight = optimal(&windows, &requests, &[rule(1, 31)]);
        assert_eq!(
            reasons(&tight),
            [ReasonCode::Scheduled, ReasonCode::OutweighedByPlan]
        );
        assert_eq!(tight[1].blocked_by, Some(0));
    }

    #[test]
    fn a_satellite_uses_one_station_at_a_time() {
        let windows = [window(7, 1, 0, 300), window(7, 2, 100, 600)];
        let verdicts = optimal(&windows, &[request(7, 5)], &[]);
        assert_eq!(
            reasons(&verdicts),
            [ReasonCode::OutweighedByPlan, ReasonCode::Scheduled]
        );
    }

    #[test]
    fn screening_matches_the_greedy_scheduler() {
        let windows = [
            window(9, 1, 0, 900),
            window(1, 1, 0, 90),
            window(1, 2, 0, 300),
        ];
        let requests = [request(1, 5).with_min_contact(Seconds::new(120))];
        assert_eq!(
            reasons(&optimal(&windows, &requests, &[])),
            [
                ReasonCode::NoRequest,
                ReasonCode::BelowMinimumDuration,
                ReasonCode::Scheduled
            ]
        );
    }

    #[test]
    fn duplicate_requests_are_refused_with_the_flight_code() {
        let error = schedule_optimal(
            &[window(1, 1, 0, 100)],
            &[request(1, 1), request(1, 2)],
            &[],
            AMPLE,
            false,
        )
        .unwrap_err();
        assert_eq!(error.reason(), Some(ReasonCode::DuplicateRequest));
    }

    // --- Slew time -----------------------------------------------------------------------------

    #[test]
    fn the_antenna_is_given_time_to_turn_between_passes() {
        // The same three passes as the flight scheduler's test: 20 s between the first and
        // each of the others, and a mount that turns 6 degrees per second. The second rises
        // half a turn from where the first sets (30 s away); the third rises 30 degrees from it
        // (5 s away). The second and third overlap each other.
        let windows = [
            pointed(1, 1, 0, 600, 0, 9_000),
            pointed(2, 1, 620, 1_000, 27_000, 0),
            pointed(3, 1, 620, 900, 12_000, 0),
        ];
        let requests = [request(1, 5), request(2, 5), request(3, 5)];
        let slewing = [rule(1, 0).with_slew_rates(SlewRates::new(600, 250).unwrap())];

        // Without slew rates: first (600 s) plus the longer of the other two (380 s).
        assert_eq!(
            reasons(&optimal(&windows, &requests, &[rule(1, 0)])),
            [
                ReasonCode::Scheduled,
                ReasonCode::Scheduled,
                ReasonCode::OutweighedByPlan
            ]
        );
        // With them the second cannot follow the first. First plus third (600 + 280) beats the
        // second alone (380), so the third is scheduled and the second names its blocker.
        let verdicts = optimal(&windows, &requests, &slewing);
        assert_eq!(
            reasons(&verdicts),
            [
                ReasonCode::Scheduled,
                ReasonCode::OutweighedByPlan,
                ReasonCode::Scheduled
            ]
        );
        assert!(matches!(verdicts[1].blocked_by, Some(0 | 2)));
    }

    #[test]
    fn a_pair_that_is_too_close_only_because_of_slew_gets_its_own_constraint() {
        let windows = [
            pointed(1, 1, 0, 600, 0, 9_000),
            pointed(2, 1, 620, 1_000, 27_000, 0),
        ];
        let candidates = [0, 1];
        let slewing = [rule(1, 0).with_slew_rates(SlewRates::new(600, 250).unwrap())];
        assert_eq!(cliques(&windows, &candidates, &slewing), [vec![0, 1]]);
        // Without slew rates the 20 s gap is fine, and nothing constrains the pair.
        assert!(cliques(&windows, &candidates, &[rule(1, 0)]).is_empty());
        // With a fixed turnaround longer than the gap the ordinary clique already covers it,
        // and the pair is not added a second time.
        let both = [rule(1, 30).with_slew_rates(SlewRates::new(600, 250).unwrap())];
        assert_eq!(cliques(&windows, &candidates, &both), [vec![0, 1]]);
    }

    // --- A floor for each satellite ------------------------------------------------------------

    /// The plan built with a floor under every satellite, and the floors used.
    fn floored(instance: &Instance) -> (Vec<Verdict>, Vec<u64>) {
        let outcome = schedule_optimal(
            &instance.windows,
            &instance.requests,
            &instance.rules,
            AMPLE,
            true,
        )
        .expect("the solver succeeds");
        assert!(outcome.proven_optimal);
        (
            outcome.verdicts,
            outcome.floors.expect("a floor was asked for"),
        )
    }

    fn useful_of(instance: &Instance, verdicts: &[Verdict]) -> Vec<u64> {
        let chosen: Vec<bool> = verdicts.iter().map(Verdict::is_scheduled).collect();
        instance
            .requests
            .iter()
            .map(|request| useful_seconds(&instance.windows, &chosen, request))
            .collect()
    }

    fn instance_of(
        windows: Vec<ContactWindow>,
        requests: Vec<SatelliteRequest>,
        rules: Vec<StationRule>,
    ) -> Instance {
        Instance {
            windows,
            requests,
            rules,
        }
    }

    #[test]
    fn without_a_floor_the_solver_can_take_everything_from_one_satellite() {
        // One station. Satellite 1 has one pass of 100 s. Satellite 2 has two passes of 60 s
        // that both overlap it. Equal priority. Greedy takes the longest pass first, so
        // satellite 1 gets 100 s and satellite 2 nothing. The best total is the other way
        // round: 120 s for satellite 2 and nothing for satellite 1.
        let instance = instance_of(
            vec![
                window(1, 1, 0, 100),
                window(2, 1, 0, 60),
                window(2, 1, 60, 120),
            ],
            vec![request(1, 5), request(2, 5)],
            Vec::new(),
        );
        assert_eq!(useful_of(&instance, &greedy(&instance)), [100, 0]);

        let free = optimal(&instance.windows, &instance.requests, &instance.rules);
        assert_eq!(useful_of(&instance, &free), [0, 120]);

        // With a floor satellite 1 keeps its 100 s, and nothing better is left.
        let (kept, floors) = floored(&instance);
        assert_eq!(floors, [100, 0]);
        assert_eq!(useful_of(&instance, &kept), [100, 0]);
        assert_eq!(
            reasons(&kept),
            [
                ReasonCode::Scheduled,
                ReasonCode::OutweighedByPlan,
                ReasonCode::OutweighedByPlan
            ]
        );
    }

    #[test]
    fn a_floor_still_allows_gains_that_cost_no_satellite_anything() {
        // Satellite 1 has a 100 s pass at station 1, and at station 2 two passes of 60 and 65 s
        // that both overlap it in time. Greedy takes the 100 s pass, which rules out the other
        // two. Taking those two gives the same satellite 125 s, and nobody loses.
        let instance = instance_of(
            vec![
                window(1, 1, 0, 100),
                window(1, 2, 0, 60),
                window(1, 2, 60, 125),
            ],
            vec![request(1, 5)],
            Vec::new(),
        );
        assert_eq!(useful_of(&instance, &greedy(&instance)), [100]);
        let (plan, floors) = floored(&instance);
        assert_eq!(floors, [100]);
        assert_eq!(useful_of(&instance, &plan), [125]);
    }

    #[test]
    fn a_floor_counts_contact_only_up_to_a_need() {
        // Satellite 1 needs 150 s. Greedy gives it the 200 s pass, of which 150 is useful, so
        // its floor is 150 and not 200. The plan may give it any passes that reach 150.
        let instance = instance_of(
            vec![
                window(1, 1, 0, 200),
                window(1, 2, 300, 460),
                window(2, 1, 0, 190),
            ],
            vec![request(1, 5).with_need(Seconds::new(150)), request(2, 5)],
            Vec::new(),
        );
        let (plan, floors) = floored(&instance);
        assert_eq!(floors, [150, 0]);
        // Satellite 1 moves to its 160 s pass at station 2, which frees station 1 for
        // satellite 2. Both gain or keep what they had.
        assert_eq!(useful_of(&instance, &plan), [150, 190]);
    }

    #[test]
    fn no_floor_is_recorded_unless_one_is_asked_for() {
        let outcome =
            schedule_optimal(&[window(1, 1, 0, 100)], &[request(1, 5)], &[], AMPLE, false).unwrap();
        assert_eq!(outcome.floors, None);
    }

    #[test]
    fn with_a_floor_no_satellite_gets_less_than_greedy_gives_it() {
        // On random crowded problems: the plan is sound, every satellite has at least its
        // greedy share, and the value lies between the greedy plan's and the unconstrained
        // optimum's. The floor must also bind somewhere, or the test would show nothing.
        let mut rng = Lcg(0x666c_6f6f);
        let (mut floor_cost_something, mut still_beat_greedy) = (0, 0);
        for round in 0..60 {
            let instance = random_instance(&mut rng, 40 + (round % 5) * 40);
            let greedy_plan = greedy(&instance);
            let (plan, floors) = floored(&instance);
            assert_plan_is_sound(&instance, &plan);

            let greedy_share = useful_of(&instance, &greedy_plan);
            assert_eq!(
                floors, greedy_share,
                "round {round}: the floor is the greedy share"
            );
            for (position, (got, floor)) in
                useful_of(&instance, &plan).iter().zip(&floors).enumerate()
            {
                assert!(
                    got >= floor,
                    "round {round}: request {position} has {got} s, below its floor of {floor} s"
                );
            }

            let free = optimal(&instance.windows, &instance.requests, &instance.rules);
            let (low, found, high) = (
                value_of(&instance, &greedy_plan),
                value_of(&instance, &plan),
                value_of(&instance, &free),
            );
            assert!(low - 1e-6 <= found && found <= high + 1e-6, "round {round}");
            floor_cost_something += usize::from(found < high - 1e-6);
            still_beat_greedy += usize::from(found > low + 1e-6);
        }
        assert!(
            floor_cost_something >= 10,
            "the floor bound in {floor_cost_something} of 60"
        );
        assert!(
            still_beat_greedy >= 10,
            "greedy was beaten in {still_beat_greedy} of 60"
        );
    }

    #[test]
    fn with_a_floor_the_plan_is_the_best_of_the_plans_that_respect_it() {
        // Every subset of windows is tried, keeping only those that give each satellite its
        // greedy share. The solver's plan must be worth exactly as much as the best of them.
        let mut rng = Lcg(0x6265_7374);
        let mut differs_from_unconstrained = 0;
        for round in 0..200 {
            let instance = random_instance(&mut rng, 4 + rng_count(round));
            let (plan, floors) = floored(&instance);
            assert_plan_is_sound(&instance, &plan);
            let best = brute_force_value_above(
                &instance.windows,
                &instance.requests,
                &instance.rules,
                Some(&floors),
            );
            let found = value_of(&instance, &plan);
            assert!(
                (found - best).abs() < 1e-6,
                "round {round}: the plan is worth {found}, the best subset above the floor {best}"
            );
            let unconstrained =
                brute_force_value(&instance.windows, &instance.requests, &instance.rules);
            differs_from_unconstrained += usize::from(best < unconstrained - 1e-6);
        }
        assert!(
            differs_from_unconstrained >= 5,
            "the floor changed the optimum in only {differs_from_unconstrained} of 200 problems"
        );
    }

    #[test]
    fn with_a_floor_and_no_time_the_plan_is_the_greedy_plan() {
        let mut rng = Lcg(0x6e6f_7469);
        for _ in 0..20 {
            let instance = random_instance(&mut rng, 60);
            let outcome = schedule_optimal(
                &instance.windows,
                &instance.requests,
                &instance.rules,
                Duration::ZERO,
                true,
            )
            .unwrap();
            assert!(!outcome.proven_optimal);
            assert_plan_is_sound(&instance, &outcome.verdicts);
            assert_eq!(
                useful_of(&instance, &outcome.verdicts),
                useful_of(&instance, &greedy(&instance))
            );
        }
    }

    // --- Time limit ----------------------------------------------------------------------------

    #[test]
    fn with_no_time_at_all_the_result_is_still_sound_and_no_worse_than_greedy() {
        // A zero limit stops the solver before it has done anything. The plan must still obey
        // every rule and be worth at least what greedy gives, because that is where it starts.
        let mut rng = Lcg(0x7a65_726f);
        for round in 0..40 {
            let instance = random_instance(&mut rng, 60);
            let outcome = schedule_optimal(
                &instance.windows,
                &instance.requests,
                &instance.rules,
                Duration::ZERO,
                false,
            )
            .expect("running out of time is not an error");
            assert_plan_is_sound(&instance, &outcome.verdicts);
            let (found, greedy_value) = (
                value_of(&instance, &outcome.verdicts),
                value_of(&instance, &greedy(&instance)),
            );
            assert!(
                found >= greedy_value - 1e-6,
                "round {round}: {found} is below greedy {greedy_value}"
            );
        }
    }

    #[test]
    fn a_limit_shorter_than_the_solve_gives_a_sound_plan_that_may_not_be_proven() {
        // Limits from far too short to plenty, on problems big enough to take a while. What is
        // promised is the same at every limit: a sound plan worth at least the greedy one and
        // no more than the optimum. What varies is whether it is proven optimal, and when it is,
        // its value must be the optimum.
        //
        // Measured in a debug build: these problems take 15 to 100 ms to solve to the end, the
        // solver has no plan of its own before about 30 ms, and between about 40 and 80 ms it
        // can return a plan it has not proven. The limits below straddle that. Nothing asserted
        // here depends on which side of a limit a given machine falls.
        let mut rng = Lcg(0x6c69_6d69);
        let mut with_something_to_gain = 0;
        for _ in 0..6 {
            let instance = random_instance(&mut rng, 400);
            let greedy_value = value_of(&instance, &greedy(&instance));
            let best = value_of(
                &instance,
                &optimal(&instance.windows, &instance.requests, &instance.rules),
            );
            with_something_to_gain += usize::from(best > greedy_value);

            for limit in [
                Duration::from_nanos(1),
                Duration::from_millis(10),
                Duration::from_millis(30),
                Duration::from_millis(50),
                Duration::from_millis(80),
                AMPLE,
            ] {
                let outcome = schedule_optimal(
                    &instance.windows,
                    &instance.requests,
                    &instance.rules,
                    limit,
                    false,
                )
                .expect("running out of time is not an error");
                assert_plan_is_sound(&instance, &outcome.verdicts);
                let found = value_of(&instance, &outcome.verdicts);
                assert!(
                    found >= greedy_value - 1e-6,
                    "{limit:?}: {found} is below greedy {greedy_value}"
                );
                assert!(
                    found <= best + 1e-6,
                    "{limit:?}: {found} exceeds the optimum {best}"
                );
                if outcome.proven_optimal {
                    assert!(
                        (found - best).abs() < 1e-6,
                        "{limit:?}: proven, but {found} is not the optimum {best}"
                    );
                }
                if limit == AMPLE {
                    assert!(outcome.proven_optimal, "ample time proves the plan");
                }
            }
        }
        assert!(
            with_something_to_gain >= 3,
            "most of these problems should leave the solver something to find"
        );
    }

    #[test]
    fn only_a_finished_search_counts_as_proof() {
        assert!(is_proven(SolutionStatus::Optimal));
        assert!(!is_proven(SolutionStatus::TimeLimit));
        assert!(!is_proven(SolutionStatus::GapLimit));
    }

    #[test]
    fn a_plan_the_solver_was_stopped_on_is_sound_and_not_called_proven() {
        // The case in between "no time" and "enough time": the solver is stopped while it holds
        // a plan it has not proven. When that happens depends on the machine, so this test
        // times the full solve first and then stops the solver at steps across that time.
        //
        // What it asserts holds whenever the case arises: such a plan has no conflicts, is not
        // worth more than the optimum, and is not reported as proven unless it is the optimum.
        // It does not assert that the case arose, because on a given run it may not. In three
        // runs on the development machine it arose 12, 14 and 17 times in 96 tries.
        let mut rng = Lcg(0x6c69_6d69);
        for _ in 0..6 {
            let instance = random_instance(&mut rng, 400);
            let Instance {
                windows,
                requests,
                rules,
            } = &instance;
            let candidates: Vec<usize> = (0..windows.len())
                .filter(|&index| screen(&windows[index], requests).is_ok())
                .collect();
            let start = greedy_choice(windows, requests, rules).unwrap();

            let clock = std::time::Instant::now();
            let (best, proven) = solve(windows, &candidates, requests, rules, &start, None, AMPLE)
                .unwrap()
                .expect("ample time gives a plan");
            let full = clock.elapsed();
            assert!(proven);
            let best = plan_value(windows, &best, requests);

            for step in 4..20_u32 {
                let limit = full * step / 20;
                let Some((chosen, proven)) =
                    solve(windows, &candidates, requests, rules, &start, None, limit).unwrap()
                else {
                    continue; // stopped before it had a plan
                };
                check_no_conflicts(windows, &chosen, rules).expect("an unproven plan is sound");
                let found = plan_value(windows, &chosen, requests);
                assert!(found <= best + 1e-6, "{found} exceeds the optimum {best}");
                if proven {
                    assert!(
                        (found - best).abs() < 1e-6,
                        "proven, but {found} is not {best}"
                    );
                }
            }
        }
    }

    #[test]
    fn an_enormous_time_limit_is_accepted() {
        // `Duration::MAX` does not survive a round trip through floating-point seconds, so the
        // limit handed to the solver is capped. This must not panic.
        let windows = [window(1, 1, 0, 300), window(2, 1, 100, 200)];
        let outcome = schedule_optimal(
            &windows,
            &[request(1, 5), request(2, 5)],
            &[],
            Duration::MAX,
            false,
        )
        .unwrap();
        assert!(outcome.proven_optimal);
    }

    #[test]
    fn nothing_to_solve_is_proven_optimal_whatever_the_limit() {
        let outcome = schedule_optimal(&[], &[], &[], Duration::ZERO, false).unwrap();
        assert!(outcome.proven_optimal && outcome.verdicts.is_empty());
        // Windows that are all screened out leave nothing for the solver either.
        let outcome = schedule_optimal(
            &[window(9, 1, 0, 100)],
            &[request(1, 1)],
            &[],
            Duration::ZERO,
            false,
        )
        .unwrap();
        assert!(outcome.proven_optimal);
        assert_eq!(reasons(&outcome.verdicts), [ReasonCode::NoRequest]);
    }

    // --- Value ---------------------------------------------------------------------------------

    #[test]
    fn plan_value_weights_useful_seconds_by_priority_plus_one() {
        let windows = [
            window(1, 1, 0, 100),
            window(2, 2, 0, 300),
            window(2, 2, 400, 700),
        ];
        let requests = [
            request(1, 0),                              // weight 1, unlimited
            request(2, 4).with_need(Seconds::new(500)), // weight 5, capped at 500
        ];
        // All scheduled: 1 x 100 + 5 x min(600, 500) = 2600.
        assert_eq!(
            plan_value(&windows, &[true, true, true], &requests),
            2_600.0
        );
        // One of satellite 2's passes: 1 x 100 + 5 x 300 = 1600.
        assert_eq!(
            plan_value(&windows, &[true, true, false], &requests),
            1_600.0
        );
        assert_eq!(plan_value(&windows, &[false, false, false], &requests), 0.0);
    }

    #[test]
    fn a_satellite_without_a_request_adds_no_value() {
        let windows = [window(9, 1, 0, 100)];
        assert_eq!(plan_value(&windows, &[true], &[request(1, 5)]), 0.0);
    }

    // --- Cliques -------------------------------------------------------------------------------

    #[test]
    fn cliques_rule_out_exactly_the_conflicting_pairs() {
        // For random problems: two candidates share some clique if and only if `conflict` says
        // they cannot both be scheduled. This is what makes the integer program equivalent to
        // the rules the greedy scheduler follows.
        let mut rng = Lcg(0x636c_6971);
        for _ in 0..150 {
            let instance = random_instance(&mut rng, 30);
            let candidates: Vec<usize> = (0..instance.windows.len()).collect();
            let found = cliques(&instance.windows, &candidates, &instance.rules);
            for a in 0..instance.windows.len() {
                for b in (a + 1)..instance.windows.len() {
                    let (wa, wb) = (&instance.windows[a], &instance.windows[b]);
                    let rule = rule_for(&instance.rules, wa.station());
                    let conflicting = conflict(wa, wb, &rule).is_some();
                    let together = found.iter().any(|c| c.contains(&a) && c.contains(&b));
                    assert_eq!(together, conflicting, "windows {a} and {b}: {wa:?} {wb:?}");
                }
            }
        }
    }

    // --- Against brute force and against greedy ------------------------------------------------

    #[test]
    fn the_plan_value_equals_the_best_found_by_trying_every_subset() {
        // The proof of "optimal": for small problems every subset of windows is tried, and the
        // solver's plan must be worth exactly as much as the best of them.
        // Tolerance 1e-6 on values that are whole numbers up to a few tens of thousands.
        let mut rng = Lcg(0x6f70_7469);
        let mut strictly_better_than_greedy = 0;
        for round in 0..300 {
            let instance = random_instance(&mut rng, 4 + rng_count(round));
            let verdicts = optimal(&instance.windows, &instance.requests, &instance.rules);
            assert_plan_is_sound(&instance, &verdicts);

            let best = brute_force_value(&instance.windows, &instance.requests, &instance.rules);
            let found = value_of(&instance, &verdicts);
            assert!(
                (found - best).abs() < 1e-6,
                "round {round}: the solver's plan is worth {found}, the best subset {best}"
            );
            if found > value_of(&instance, &greedy(&instance)) + 1e-6 {
                strictly_better_than_greedy += 1;
            }
        }
        // The comparison would be empty if greedy were always optimal on these problems.
        assert!(
            strictly_better_than_greedy >= 20,
            "greedy was beaten in only {strictly_better_than_greedy} of 300 problems"
        );
    }

    /// 4 to 16 windows, cycling, so that brute force stays within 2^16 subsets.
    fn rng_count(round: u64) -> u64 {
        round % 13
    }

    #[test]
    fn the_optimal_plan_is_never_worth_less_than_the_greedy_plan() {
        // Larger problems than brute force can check. The greedy plan obeys the same rules, so
        // it is one of the plans the optimizer chose among and cannot be worth more.
        let mut rng = Lcg(0x6772_6565);
        for round in 0..60 {
            let instance = random_instance(&mut rng, 40 + (round % 5) * 40);
            let verdicts = optimal(&instance.windows, &instance.requests, &instance.rules);
            assert_plan_is_sound(&instance, &verdicts);
            let (best, greedy_value) = (
                value_of(&instance, &verdicts),
                value_of(&instance, &greedy(&instance)),
            );
            assert!(
                best >= greedy_value - 1e-6,
                "round {round}: optimal {best} is below greedy {greedy_value}"
            );
        }
    }

    #[test]
    fn the_same_problem_gives_the_same_plan() {
        let mut rng = Lcg(11);
        let instance = random_instance(&mut rng, 80);
        let first = optimal(&instance.windows, &instance.requests, &instance.rules);
        let second = optimal(&instance.windows, &instance.requests, &instance.rules);
        assert_eq!(first, second);
    }

    #[test]
    fn total_contact_adds_up_the_scheduled_windows() {
        let windows = [
            window(1, 1, 0, 300),
            window(2, 1, 100, 200),
            window(3, 2, 0, 50),
        ];
        let requests = [request(1, 5), request(2, 5), request(3, 5)];
        let verdicts = optimal(&windows, &requests, &[]);
        assert_eq!(total_contact(&windows, &verdicts), Seconds::new(350));
    }
}
