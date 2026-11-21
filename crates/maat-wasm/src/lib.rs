//! The browser demo's bridge to `maat-ground`.
//!
//! This crate adds **no scheduling logic**. It takes text in, calls the same functions the
//! `maat` command calls, and hands text back:
//!
//! - [`plan_json`] takes a request as JSON (a catalog, a station file, a search window and a
//!   choice of scheduler) and returns the contact plan as JSON, or an error with a hint.
//! - [`catalog_json`] lists the satellites in a catalog, and [`scenario_json`] the stations and
//!   requests in a station file, for the page's pickers.
//!
//! Both are ordinary Rust functions and are tested natively. The `#[wasm_bindgen]` functions at
//! the bottom ([`plan`], [`catalog`], [`scenario`], [`version`]) are one-line wrappers, which is all that
//! differs between the browser and a native build.
//!
//! JSON text is used on both sides of the boundary so that the boundary carries strings only.
//! The page parses the reply with `JSON.parse`.
//!
//! ## The request
//!
//! ```json
//! {
//!   "catalog":  "OMM or two-line element sets, as text",
//!   "scenario": "[[station]] and [[request]] tables, as TOML text",
//!   "start":    "2026-08-01T12:00:00Z",
//!   "hours":    6,
//!   "scheduler": "greedy",
//!   "time_limit_s": 5,
//!   "floor": false
//! }
//! ```
//!
//! `catalog`, `scenario` and `start` are required. The station file is the one the `maat`
//! command reads (`maat_ground::scenario`).
//!
//! ## The reply
//!
//! On success: `{"ok": true, "window": {"start", "end"}, "plan": {...}, "build_order": [...]}`.
//! `plan` is the `maat.contact-plan/1` document. `build_order` is described at
//! [`build_order`].
//!
//! On failure: `{"ok": false, "where": "...", "error": "...", "hint": "..." or null}`, where
//! `where` is `request`, `catalog`, `scenario` or `plan`.

use std::time::Duration;

use chrono::{DateTime, Utc};
use maat_ground::flight::schedule::rank;
use maat_ground::scenario::Scenario;
use maat_ground::{
    ContactPlan, DecidedBy, GroundError, PlanOptions, Scheduler, contact_windows,
    plan_contacts_with, satellites_from_catalog,
};
use serde_json::{Value, json};
use wasm_bindgen::prelude::wasm_bindgen;

/// How long the optimal scheduler may search when the page does not say: five seconds.
///
/// Shorter than the library's minute, because in a browser the search holds up the page.
pub const DEFAULT_TIME_LIMIT_S: f64 = 5.0;

/// The longest search the demo accepts: 31 days.
pub const LONGEST_SEARCH_HOURS: f64 = 744.0;

/// A failure, with the part of the input it came from.
struct Failure {
    place: &'static str,
    error: String,
    hint: Option<String>,
}

impl Failure {
    fn new(place: &'static str, error: impl Into<String>) -> Self {
        Self {
            place,
            error: error.into(),
            hint: None,
        }
    }

    fn from_library(place: &'static str, error: &GroundError) -> Self {
        Self {
            place,
            error: error.to_string(),
            hint: error.hint().map(|hint| hint.to_string()),
        }
    }

    fn to_json(&self) -> String {
        json!({ "ok": false, "where": self.place, "error": self.error, "hint": self.hint })
            .to_string()
    }
}

/// Builds a contact plan from a JSON request and returns a JSON reply. See the
/// [crate docs](crate) for both.
///
/// Never panics on bad input and never returns anything but JSON: every failure is a reply with
/// `"ok": false`.
#[must_use]
pub fn plan_json(request: &str) -> String {
    match try_plan(request) {
        Ok(reply) => reply.to_string(),
        Err(failure) => failure.to_json(),
    }
}

fn try_plan(request: &str) -> Result<Value, Failure> {
    let request: Value = serde_json::from_str(request)
        .map_err(|error| Failure::new("request", format!("the request is not JSON: {error}")))?;
    let text = |key: &'static str| {
        request
            .get(key)
            .and_then(Value::as_str)
            .ok_or_else(|| Failure::new("request", format!("the request needs \"{key}\" as text")))
    };

    let start = DateTime::parse_from_rfc3339(text("start")?)
        .map(|time| time.with_timezone(&Utc))
        .map_err(|_| {
            Failure::new(
                "request",
                "\"start\" is not a date and time; write it as 2026-08-01T12:00:00Z",
            )
        })?;
    let hours = match request.get("hours") {
        None | Some(Value::Null) => 24.0,
        Some(value) => match value.as_f64() {
            Some(hours) if hours > 0.0 && hours <= LONGEST_SEARCH_HOURS => hours,
            _ => {
                return Err(Failure::new(
                    "request",
                    format!(
                        "\"hours\" must be a number above 0 and at most {LONGEST_SEARCH_HOURS}"
                    ),
                ));
            }
        },
    };
    let scheduler = match request.get("scheduler").and_then(Value::as_str) {
        None | Some("greedy") => Scheduler::Greedy,
        Some("optimal") => Scheduler::Optimal,
        Some(other) => {
            return Err(Failure::new(
                "request",
                format!("\"scheduler\" is \"{other}\"; choose \"greedy\" or \"optimal\""),
            ));
        }
    };
    let limit = match request.get("time_limit_s") {
        None | Some(Value::Null) => DEFAULT_TIME_LIMIT_S,
        Some(value) => match value.as_f64() {
            Some(seconds) if (0.0..=3_600.0).contains(&seconds) => seconds,
            _ => {
                return Err(Failure::new(
                    "request",
                    "\"time_limit_s\" must be a number of seconds from 0 to 3600",
                ));
            }
        },
    };
    let floor = request
        .get("floor")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let satellites = satellites_from_catalog(text("catalog")?)
        .map_err(|error| Failure::from_library("catalog", &error))?;
    let mut scenario = Scenario::default();
    scenario
        .add_toml("station file", text("scenario")?)
        .map_err(|error| Failure::new("scenario", error))?;
    if scenario.stations.is_empty() {
        return Err(Failure::new(
            "scenario",
            "choose at least one ground station",
        ));
    }
    if scenario.requests.is_empty() {
        return Err(Failure::new(
            "scenario",
            "choose at least one satellite to schedule",
        ));
    }

    // Whole seconds, as the `maat` command does.
    let start = DateTime::from_timestamp(start.timestamp(), 0).unwrap_or(start);
    let end = start + chrono::Duration::milliseconds((hours * 3_600_000.0).round() as i64);
    let survey = contact_windows(&satellites, &scenario.stations, start, end)
        .map_err(|error| Failure::from_library("plan", &error))?;
    let options = PlanOptions::new(scheduler)
        .with_optimal_time_limit(Duration::from_secs_f64(limit))
        .with_satellite_floor(floor && scheduler == Scheduler::Optimal);
    let plan = plan_contacts_with(
        &survey,
        &satellites,
        &scenario.stations,
        &scenario.requests,
        &options,
    )
    .map_err(|error| Failure::from_library("plan", &error))?;

    Ok(json!({
        "ok": true,
        "window": {
            "start": start.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
            "end": end.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
        },
        "build_order": build_order(&plan),
        "plan": plan.to_json_value(),
    }))
}

/// The order in which the greedy scheduler decided the passes it decided, as positions in
/// [`ContactPlan::entries`]. The page replays a plan in this order.
///
/// The greedy scheduler takes candidate passes best first, by the ranking in
/// `maat_flight::schedule::rank` (priority, then length, then AOS). This function sorts the
/// plan's scheduler-decided passes by that same function; it does not restate the ranking.
/// Passes that never reached the scheduler's ranking (turned down by the survey, no request, too
/// short) are not in the list: nothing was weighed against them.
///
/// Empty for a plan from the optimal scheduler, which does not decide pass by pass.
#[must_use]
pub fn build_order(plan: &ContactPlan) -> Vec<usize> {
    if plan.scheduler != Scheduler::Greedy {
        return Vec::new();
    }
    let mut order: Vec<usize> = plan
        .entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| {
            entry.decided_by == DecidedBy::Scheduler
                && entry.priority.is_some()
                && entry.window.is_some()
                && !matches!(
                    entry.reason,
                    maat_ground::flight::ReasonCode::BelowMinimumDuration
                        | maat_ground::flight::ReasonCode::NoRequest
                )
        })
        .map(|(index, _)| index)
        .collect();
    // A stable sort, so that passes the ranking cannot separate stay in the plan's order.
    order.sort_by(|&a, &b| {
        let (a, b) = (&plan.entries[a], &plan.entries[b]);
        match (a.priority, a.window, b.priority, b.window) {
            (Some(pa), Some(wa), Some(pb), Some(wb)) => rank(pa, &wa, pb, &wb),
            _ => std::cmp::Ordering::Equal,
        }
    });
    order
}

/// Lists the satellites in a catalog as JSON, for the page's satellite picker.
///
/// On success: `{"ok": true, "satellites": [{"id", "name", "epoch"}]}`. On failure the same
/// shape as [`plan_json`]'s, with `"where": "catalog"`.
#[must_use]
pub fn catalog_json(catalog: &str) -> String {
    match satellites_from_catalog(catalog) {
        Ok(satellites) => {
            let satellites: Vec<Value> = satellites
                .iter()
                .map(|satellite| {
                    json!({
                        "id": satellite.id().get(),
                        "name": satellite.name(),
                        "epoch": satellite.epoch().format("%Y-%m-%dT%H:%M:%SZ").to_string(),
                    })
                })
                .collect();
            json!({ "ok": true, "satellites": satellites }).to_string()
        }
        Err(error) => Failure::from_library("catalog", &error).to_json(),
    }
}

/// Lists the stations and requests in a station file as JSON, for the page's pickers.
///
/// On success: `{"ok": true, "stations": [...], "requests": [...]}`, each with the keys of the
/// TOML tables (optional keys that were not given are `null`). On failure the same shape as
/// [`plan_json`]'s, with `"where": "scenario"`.
#[must_use]
pub fn scenario_json(toml: &str) -> String {
    let mut scenario = Scenario::default();
    if let Err(error) = scenario.add_toml("station file", toml) {
        return Failure::new("scenario", error).to_json();
    }
    let stations: Vec<Value> = scenario
        .stations
        .iter()
        .map(|station| {
            let slew = station.slew_limits();
            json!({
                "id": station.id().get(),
                "name": station.name(),
                "latitude_deg": station.site().latitude_deg,
                "longitude_deg": station.site().longitude_deg,
                "altitude_m": station.site().altitude_m,
                "minimum_elevation_deg": station.min_elevation_deg(),
                "turnaround_s": station.turnaround().get(),
                "slew_azimuth_deg_per_s": slew.map(|limits| limits.azimuth_deg_per_s),
                "slew_elevation_deg_per_s": slew.map(|limits| limits.elevation_deg_per_s),
            })
        })
        .collect();
    let requests: Vec<Value> = scenario
        .requests
        .iter()
        .map(|request| {
            json!({
                "satellite": request.satellite().get(),
                "priority": request.priority().get(),
                "min_contact_s": request.min_contact().get(),
                "need_s": (!request.is_unlimited()).then(|| request.need().get()),
            })
        })
        .collect();
    json!({ "ok": true, "stations": stations, "requests": requests }).to_string()
}

/// [`plan_json`], exported to JavaScript.
#[wasm_bindgen]
#[must_use]
pub fn plan(request: &str) -> String {
    plan_json(request)
}

/// [`catalog_json`], exported to JavaScript.
#[wasm_bindgen]
#[must_use]
pub fn catalog(text: &str) -> String {
    catalog_json(text)
}

/// [`scenario_json`], exported to JavaScript.
#[wasm_bindgen]
#[must_use]
pub fn scenario(toml: &str) -> String {
    scenario_json(toml)
}

/// The version of the scheduler the page is running.
#[wasm_bindgen]
#[must_use]
pub fn version() -> String {
    env!("CARGO_PKG_VERSION").to_owned()
}
