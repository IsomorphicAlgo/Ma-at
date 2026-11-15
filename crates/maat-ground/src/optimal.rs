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
//! - a station serves one pass at a time, with its turnaround time between passes;
//! - a satellite uses one station at a time.
//!
//! Both are written as *clique* constraints: for every instant a pass begins, at most one of the
//! passes in progress at that station (or for that satellite) may be chosen. This is much
//! tighter than one constraint per conflicting pair, and it is why the solver usually finds a
//! whole-number answer without having to branch.
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
//! - **Run time is not bounded.** This is an NP-hard problem in general. The solver is a
//!   pure-Rust branch-and-bound (`microlp`, through `good_lp`) and has no time limit. Problems of
//!   a few hundred to a few thousand windows solve in well under a second in practice; an
//!   adversarial problem could take very long. Use the greedy scheduler when a bound matters.
//! - **Ground only.** It allocates, uses floating point and depends on a solver.

use std::cmp::Ordering;

use good_lp::{
    Expression, ProblemVariables, Solution, SolverModel, Variable, constraint, microlp, variable,
};
#[cfg(test)]
use maat_flight::Seconds;
use maat_flight::schedule::{conflict, rank, screen, turnaround_for, validate};
use maat_flight::{ContactWindow, ReasonCode, SatelliteRequest, StationRule};

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
            let granted = granted_seconds(windows, scheduled, request);
            let useful = if request.is_unlimited() {
                granted
            } else {
                granted.min(u64::from(request.need().get()))
            };
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

/// Schedules contact windows for the greatest total value. See the [module docs](self).
///
/// Returns one [`Verdict`] per window, in the order of `windows`.
pub(crate) fn schedule_optimal(
    windows: &[ContactWindow],
    requests: &[SatelliteRequest],
    rules: &[StationRule],
) -> Result<Vec<Verdict>, GroundError> {
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

    let mut chosen = solve(windows, &candidates, requests, rules)?;
    check_no_conflicts(windows, &chosen, rules)?;
    tidy(
        windows,
        &candidates,
        &request_of,
        requests,
        rules,
        &mut chosen,
    );

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

    Ok(verdicts
        .into_iter()
        .map(|verdict| verdict.expect("every window has a verdict"))
        .collect())
}

/// Builds and solves the integer program. Returns, for each window, whether it was chosen.
fn solve(
    windows: &[ContactWindow],
    candidates: &[usize],
    requests: &[SatelliteRequest],
    rules: &[StationRule],
) -> Result<Vec<bool>, GroundError> {
    let mut chosen = vec![false; windows.len()];
    if candidates.is_empty() {
        return Ok(chosen);
    }

    let mut variables = ProblemVariables::new();
    // `pick[i]` is the yes/no variable for window `i`, for candidates only.
    let mut pick: Vec<Option<Variable>> = vec![None; windows.len()];
    for &index in candidates {
        pick[index] = Some(variables.add(variable().binary()));
    }

    // Objective, and one "useful contact" variable for each satellite whose need can bind.
    let mut objective = Expression::from(0.0);
    let mut caps: Vec<(Variable, Expression)> = Vec::new();
    for request in requests {
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
            let useful = variables.add(variable().min(0.0).max(need));
            objective += weight(request) * useful;
            caps.push((useful, contact));
        } else {
            // Every second counts: the need is unlimited or out of reach.
            objective += weight(request) * contact;
        }
    }

    let mut model = variables.maximise(objective).using(microlp);
    for (useful, contact) in caps {
        model = model.with(constraint!(useful <= contact));
    }
    for clique in cliques(windows, candidates, rules) {
        let mut together = Expression::from(0.0);
        for index in clique {
            together += pick[index].expect("a candidate has a variable");
        }
        model = model.with(constraint!(together <= 1.0));
    }

    let solution = model.solve().map_err(|error| GroundError::Solver {
        message: error.to_string(),
    })?;
    for &index in candidates {
        chosen[index] = solution.value(pick[index].expect("a candidate has a variable")) > 0.5;
    }
    Ok(chosen)
}

/// The sets of windows of which at most one may be scheduled.
///
/// For each candidate window `w`, taken as "the instant `w` begins":
///
/// - **station clique**: every candidate at `w`'s station that is in progress at that instant,
///   or ended less than the turnaround time before it;
/// - **satellite clique**: every candidate of `w`'s satellite, at any station, in progress at
///   that instant.
///
/// Any two windows that conflict appear together in the clique of whichever begins later, so
/// together the cliques rule out exactly the conflicting pairs. Cliques of one are dropped.
fn cliques(
    windows: &[ContactWindow],
    candidates: &[usize],
    rules: &[StationRule],
) -> Vec<Vec<usize>> {
    let mut cliques = Vec::new();
    for &anchor in candidates {
        let start = windows[anchor].aos();
        let turnaround = turnaround_for(rules, windows[anchor].station());

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
    let turnaround = turnaround_for(rules, window.station());
    (0..windows.len()).find(|&other| {
        other != index && chosen[other] && conflict(window, &windows[other], turnaround).is_some()
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
        if check_no_conflicts(windows, &chosen, rules).is_ok() {
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
    use crate::testkit::{Instance, Lcg, random_instance, request, rule, window};
    use maat_flight::{Decision, schedule_greedy};

    // --- Helpers -------------------------------------------------------------------------------

    fn optimal(
        windows: &[ContactWindow],
        requests: &[SatelliteRequest],
        rules: &[StationRule],
    ) -> Vec<Verdict> {
        schedule_optimal(windows, requests, rules).expect("the solver succeeds")
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
                    let turnaround = turnaround_for(rules, window.station());
                    assert!(
                        conflict(window, &windows[blocker], turnaround).is_some(),
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
        )
        .unwrap_err();
        assert_eq!(error.reason(), Some(ReasonCode::DuplicateRequest));
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
                    let turnaround = turnaround_for(&instance.rules, wa.station());
                    let conflicting = conflict(wa, wb, turnaround).is_some();
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
