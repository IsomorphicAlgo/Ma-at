//! The bridge, tested natively: the same functions the page calls, without a browser.
//!
//! Deterministic and offline. The inputs are the repository's example files, which are also what
//! the page loads by default.

use maat_ground::flight::schedule::{conflict, rule_for};
use maat_ground::flight::{ContactWindow, StationRule};
use maat_ground::scenario::Scenario;
use maat_ground::{
    GroundStation, PlanOptions, Scheduler, contact_windows, plan_contacts_with,
    satellites_from_catalog,
};
use maat_wasm::{build_order, catalog_json, plan_json, scenario_json};
use serde_json::{Value, json};

const CATALOG: &str = include_str!("../../../examples/catalog.json");
const SCENARIO: &str = include_str!("../../../examples/stations.toml");
const START: &str = "2026-08-01T12:00:00Z";

fn request(extra: Value) -> String {
    let mut request = json!({
        "catalog": CATALOG, "scenario": SCENARIO, "start": START, "hours": 6,
    });
    for (key, value) in extra.as_object().expect("extra is an object") {
        request[key] = value.clone();
    }
    request.to_string()
}

fn reply(request: &str) -> Value {
    serde_json::from_str(&plan_json(request)).expect("the reply is always JSON")
}

/// The same plan built straight from the library, for comparison.
fn native(scheduler: Scheduler, hours: i64) -> (maat_ground::ContactPlan, Vec<GroundStation>) {
    let satellites = satellites_from_catalog(CATALOG).unwrap();
    let mut scenario = Scenario::default();
    scenario.add_toml("stations.toml", SCENARIO).unwrap();
    let start = chrono::DateTime::parse_from_rfc3339(START)
        .unwrap()
        .to_utc();
    let survey = contact_windows(
        &satellites,
        &scenario.stations,
        start,
        start + chrono::Duration::hours(hours),
    )
    .unwrap();
    let plan = plan_contacts_with(
        &survey,
        &satellites,
        &scenario.stations,
        &scenario.requests,
        &PlanOptions::new(scheduler),
    )
    .unwrap();
    (plan, scenario.stations)
}

#[test]
fn the_bridge_returns_the_librarys_plan_unchanged() {
    let reply = reply(&request(json!({})));
    assert_eq!(reply["ok"], true, "{reply}");
    assert_eq!(
        reply["window"],
        json!({ "start": "2026-08-01T12:00:00Z", "end": "2026-08-01T18:00:00Z" })
    );
    let (plan, _) = native(Scheduler::Greedy, 6);
    assert_eq!(reply["plan"], plan.to_json_value());
    assert_eq!(reply["plan"]["schema"], "maat.contact-plan/1");
    assert_eq!(reply["plan"]["totals"]["passes"], 14);
    assert_eq!(reply["plan"]["totals"]["scheduled"], 4);
}

#[test]
fn the_defaults_are_a_greedy_plan_for_a_day() {
    let request = json!({ "catalog": CATALOG, "scenario": SCENARIO, "start": START });
    let reply = reply(&request.to_string());
    assert_eq!(reply["ok"], true, "{reply}");
    assert_eq!(reply["window"]["end"], "2026-08-02T12:00:00Z");
    assert_eq!(reply["plan"]["scheduler"], "greedy");
}

#[test]
fn the_build_order_is_the_order_the_greedy_scheduler_decided_in() {
    // Replay a week's plan in the order given. If the order is the scheduler's own, then at
    // every step: a scheduled pass conflicts with nothing scheduled before it, and a pass that
    // was turned down for another pass finds that pass already scheduled.
    let (plan, stations) = native(Scheduler::Greedy, 168);
    let rules: Vec<StationRule> = stations.iter().map(GroundStation::rule).collect();
    let order = build_order(&plan);
    assert!(order.len() > 300, "a week has hundreds of contested passes");

    let mut placed: Vec<usize> = Vec::new();
    let mut seen = vec![false; plan.entries.len()];
    for &index in &order {
        assert!(!seen[index], "entry {index} appears twice");
        seen[index] = true;
        let entry = &plan.entries[index];
        let window: ContactWindow = entry.window.expect("ordered entries have a window");
        if entry.is_scheduled() {
            for &earlier in &placed {
                let other = plan.entries[earlier].window.unwrap();
                let rule = rule_for(&rules, window.station());
                assert_eq!(conflict(&window, &other, &rule), None, "entry {index}");
            }
            placed.push(index);
        } else if let Some(blocker) = entry.blocked_by {
            assert!(
                placed.contains(&blocker),
                "entry {index} lost to entry {blocker}, which had not been scheduled yet"
            );
        }
    }
    // Everything scheduled is in the order, and nothing the scheduler never ranked is.
    for (index, entry) in plan.entries.iter().enumerate() {
        if entry.is_scheduled() {
            assert!(
                seen[index],
                "scheduled entry {index} is missing from the order"
            );
        }
        if entry.priority.is_none() {
            assert!(
                !seen[index],
                "entry {index} has no request and was never ranked"
            );
        }
    }
}

#[test]
fn the_reply_carries_the_build_order_as_entry_positions() {
    let reply = reply(&request(json!({})));
    let order: Vec<usize> = reply["build_order"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| usize::try_from(v.as_u64().unwrap()).unwrap())
        .collect();
    let (plan, _) = native(Scheduler::Greedy, 6);
    assert_eq!(order, build_order(&plan));
    // In the six-hour example: four scheduled passes, and two that lost to one of them.
    assert_eq!(order.len(), 6);
    let entries = reply["plan"]["entries"].as_array().unwrap();
    // The first pass decided is the highest priority, longest one: EXAMPLE-A, 404 s.
    assert_eq!(entries[order[0]]["satellite"]["name"], "EXAMPLE-A");
    assert_eq!(entries[order[0]]["scheduled"], true);
}

#[test]
fn the_optimal_scheduler_has_no_build_order_and_reports_its_proof() {
    let reply = reply(&request(
        json!({ "scheduler": "optimal", "time_limit_s": 30, "floor": true }),
    ));
    assert_eq!(reply["ok"], true, "{reply}");
    assert_eq!(reply["build_order"], json!([]));
    assert_eq!(
        reply["plan"]["optimality"],
        json!({ "proven": true, "time_limit_s": 30.0, "satellite_floor": true })
    );
    // The floor is ignored for the greedy scheduler, as in the library.
    let greedy = self::reply(&request(json!({ "floor": true })));
    assert_eq!(greedy["plan"]["optimality"], Value::Null);
}

#[test]
fn every_failure_is_a_json_reply_that_says_where_and_what() {
    let cases: Vec<(String, &str, &str)> = vec![
        ("not json".to_owned(), "request", "the request is not JSON"),
        ("{}".to_owned(), "request", "needs \"start\" as text"),
        (
            request(json!({ "start": "noon" })),
            "request",
            "is not a date and time",
        ),
        (
            request(json!({ "hours": 0 })),
            "request",
            "\"hours\" must be a number above 0",
        ),
        (
            request(json!({ "hours": 100000 })),
            "request",
            "at most 744",
        ),
        (
            request(json!({ "hours": "six" })),
            "request",
            "\"hours\" must be a number",
        ),
        (
            request(json!({ "scheduler": "best" })),
            "request",
            "choose \"greedy\" or \"optimal\"",
        ),
        (
            request(json!({ "time_limit_s": -1 })),
            "request",
            "from 0 to 3600",
        ),
        (
            request(json!({ "catalog": "nothing useful" })),
            "catalog",
            "satellite catalog",
        ),
        (
            request(json!({ "catalog": 5 })),
            "request",
            "needs \"catalog\" as text",
        ),
        (
            request(json!({ "scenario": "[[station]]\nid = 1\n" })),
            "scenario",
            "missing field",
        ),
        (
            request(json!({ "scenario": "" })),
            "scenario",
            "choose at least one ground station",
        ),
        (
            request(
                json!({ "scenario": "[[station]]\nid = 1\nname = \"A\"\nlatitude_deg = 1.0\nlongitude_deg = 2.0\nminimum_elevation_deg = 10.0\n" }),
            ),
            "scenario",
            "choose at least one satellite",
        ),
    ];
    for (request, place, needle) in cases {
        let reply = reply(&request);
        assert_eq!(reply["ok"], false, "{reply}");
        assert_eq!(reply["where"], place, "{reply}");
        let error = reply["error"].as_str().unwrap();
        assert!(error.contains(needle), "expected {needle:?} in {error:?}");
        assert!(reply["hint"].is_null() || reply["hint"].is_string());
    }
}

#[test]
fn a_library_error_keeps_its_hint_and_its_reason_code() {
    let bad_catalog = reply(&request(json!({ "catalog": "nothing useful" })));
    assert!(bad_catalog["hint"].is_string(), "{bad_catalog}");

    let twice = format!("{SCENARIO}\n[[request]]\nsatellite = 65001\npriority = 1\n");
    let duplicate = reply(&request(json!({ "scenario": twice })));
    assert_eq!(duplicate["where"], "plan");
    assert!(
        duplicate["error"].as_str().unwrap().contains("MAAT-204"),
        "{duplicate}"
    );
}

#[test]
fn a_catalog_is_listed_for_the_satellite_picker() {
    let listing: Value = serde_json::from_str(&catalog_json(CATALOG)).unwrap();
    assert_eq!(listing["ok"], true);
    let satellites = listing["satellites"].as_array().unwrap();
    assert_eq!(satellites.len(), 5);
    assert_eq!(
        satellites[0],
        json!({ "id": 65001, "name": "EXAMPLE-A", "epoch": "2026-08-01T12:00:00Z" })
    );

    let tle = "ISS (ZARYA)\n\
               1 25544U 98067A   20194.88612269 -.00002218  00000-0 -31515-4 0  9992\n\
               2 25544  51.6461 221.2784 0001413  89.1723 280.4612 15.49507896236008\n";
    let listing: Value = serde_json::from_str(&catalog_json(tle)).unwrap();
    assert_eq!(listing["satellites"][0]["id"], 25_544);
    assert_eq!(listing["satellites"][0]["name"], "ISS (ZARYA)");

    let failed: Value = serde_json::from_str(&catalog_json("")).unwrap();
    assert_eq!(failed["ok"], false);
    assert_eq!(failed["where"], "catalog");
}

#[test]
fn the_same_request_gives_the_same_reply() {
    let request = request(json!({}));
    assert_eq!(plan_json(&request), plan_json(&request));
}

#[test]
fn a_station_file_is_listed_for_the_pickers() {
    let listing: Value = serde_json::from_str(&scenario_json(SCENARIO)).unwrap();
    assert_eq!(listing["ok"], true, "{listing}");
    assert_eq!(listing["stations"].as_array().unwrap().len(), 4);
    assert_eq!(
        listing["stations"][0],
        json!({
            "id": 1, "name": "Seattle", "latitude_deg": 47.61, "longitude_deg": -122.33,
            "altitude_m": 0.0, "minimum_elevation_deg": 10.0, "turnaround_s": 120,
            "slew_azimuth_deg_per_s": 2.0, "slew_elevation_deg_per_s": 2.0,
        })
    );
    assert_eq!(
        listing["stations"][1]["slew_azimuth_deg_per_s"],
        Value::Null
    );
    assert_eq!(
        listing["requests"][0],
        json!({ "satellite": 65001, "priority": 9, "min_contact_s": 0, "need_s": 1200 })
    );
    assert_eq!(listing["requests"][1]["need_s"], Value::Null);

    let failed: Value = serde_json::from_str(&scenario_json("[[station]]\nid = 1\n")).unwrap();
    assert_eq!(
        (&failed["ok"], &failed["where"]),
        (&json!(false), &json!("scenario"))
    );
}

#[test]
fn the_page_ships_the_repositorys_example_files() {
    // `demo/data.js` holds the catalog and station file the page starts with, each as a JSON
    // string on one line. They must be the files in `examples/`, so that the page, the README
    // and the `maat` command agree. Line endings are set aside.
    let unix = |text: &str| text.replace("\r\n", "\n");
    let data = unix(include_str!("../../../demo/data.js"));
    let shipped = |name: &str| -> String {
        let prefix = format!("export const {name} = ");
        let line = data
            .lines()
            .find(|line| line.starts_with(&prefix))
            .unwrap_or_else(|| panic!("demo/data.js has no {name}"));
        let literal = line[prefix.len()..].trim_end_matches(';');
        serde_json::from_str(literal).unwrap_or_else(|_| panic!("{name} is not a JSON string"))
    };
    assert_eq!(
        unix(&shipped("CATALOG")),
        unix(CATALOG),
        "run: node demo/make-data.mjs"
    );
    assert_eq!(
        unix(&shipped("STATION_FILE")),
        unix(SCENARIO),
        "run: node demo/make-data.mjs"
    );
}

#[test]
fn the_build_scripts_ask_for_the_wasm_bindgen_version_in_the_lock_file() {
    // `wasm-bindgen-cli` must be exactly the `wasm-bindgen` version the module was compiled
    // against, or it refuses the module. The lock file decides that version; the scripts and
    // the README tell people which to install.
    let lock = include_str!("../../../Cargo.lock").replace("\r\n", "\n");
    let after = lock
        .split("name = \"wasm-bindgen\"\nversion = \"")
        .nth(1)
        .expect("Cargo.lock lists wasm-bindgen");
    let version = after.split('"').next().unwrap();
    let wanted = format!("cargo install wasm-bindgen-cli --version {version} --locked");
    for (name, text) in [
        ("demo/build.ps1", include_str!("../../../demo/build.ps1")),
        ("demo/build.sh", include_str!("../../../demo/build.sh")),
        ("README.md", include_str!("../../../README.md")),
    ] {
        assert!(text.contains(&wanted), "{name} should say: {wanted}");
    }
}
