//! Runs the real `maat` binary on real files and checks what a user would get: the plan on
//! standard output, notes on standard error, and the exit status.
//!
//! Deterministic and offline. The satellites are the five synthetic element sets used by the
//! `contact_plan` example, with a fixed epoch, and every run but one gives `--start`.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;

const CATALOG: &str = r#"[
  {"OBJECT_NAME":"EXAMPLE-A","NORAD_CAT_ID":65001,"EPOCH":"2026-08-01T12:00:00",
   "MEAN_MOTION":15.49,"ECCENTRICITY":0.0002,"INCLINATION":51.64,"RA_OF_ASC_NODE":221.0,
   "ARG_OF_PERICENTER":89.0,"MEAN_ANOMALY":280.0,"BSTAR":0.00003},
  {"OBJECT_NAME":"EXAMPLE-B","NORAD_CAT_ID":100123,"EPOCH":"2026-08-01T12:00:00",
   "MEAN_MOTION":15.2,"ECCENTRICITY":0.0012,"INCLINATION":97.5,"RA_OF_ASC_NODE":120.0,
   "ARG_OF_PERICENTER":45.0,"MEAN_ANOMALY":315.0,"BSTAR":0.0001},
  {"OBJECT_NAME":"EXAMPLE-C","NORAD_CAT_ID":100124,"EPOCH":"2026-08-01T12:00:00",
   "MEAN_MOTION":15.2,"ECCENTRICITY":0.0012,"INCLINATION":97.5,"RA_OF_ASC_NODE":120.0,
   "ARG_OF_PERICENTER":45.0,"MEAN_ANOMALY":312.0,"BSTAR":0.0001},
  {"OBJECT_NAME":"EXAMPLE-D","NORAD_CAT_ID":100125,"EPOCH":"2026-08-01T12:00:00",
   "MEAN_MOTION":15.2,"ECCENTRICITY":0.0012,"INCLINATION":97.5,"RA_OF_ASC_NODE":120.0,
   "ARG_OF_PERICENTER":45.0,"MEAN_ANOMALY":290.0,"BSTAR":0.0001},
  {"OBJECT_NAME":"EXAMPLE-E","NORAD_CAT_ID":100126,"EPOCH":"2026-08-01T12:00:00",
   "MEAN_MOTION":14.9,"ECCENTRICITY":0.001,"INCLINATION":64.0,"RA_OF_ASC_NODE":300.0,
   "ARG_OF_PERICENTER":10.0,"MEAN_ANOMALY":40.0,"BSTAR":0.00005}
]"#;

/// The four stations of the `contact_plan` example. Quito and Cape Town have SatNOGS numbers.
const STATIONS: &str = r#"
[[station]]
id = 1
name = "Seattle"
latitude_deg = 47.61
longitude_deg = -122.33
minimum_elevation_deg = 10.0
turnaround_s = 120
slew_azimuth_deg_per_s = 2.0
slew_elevation_deg_per_s = 2.0

[[station]]
id = 2
name = "Quito"
latitude_deg = -0.18
longitude_deg = -78.47
minimum_elevation_deg = 10.0
satnogs_id = 17

[[station]]
id = 3
name = "Svalbard"
latitude_deg = 78.23
longitude_deg = 15.41
minimum_elevation_deg = 10.0
turnaround_s = 60

[[station]]
id = 4
name = "Cape Town"
latitude_deg = -33.92
longitude_deg = 18.42
minimum_elevation_deg = 10.0
satnogs_id = 2433
"#;

/// The example's requests. EXAMPLE-A and EXAMPLE-B have SatNOGS transmitters; EXAMPLE-D has not.
const REQUESTS: &str = r#"
[[request]]
satellite = 65001
priority = 9
need_s = 1200
satnogs_transmitter = "AAAAAAAAAAAAAAAAAAAAA1"

[[request]]
satellite = 100123
priority = 4
min_contact_s = 180
satnogs_transmitter = "BBBBBBBBBBBBBBBBBBBBB2"

[[request]]
satellite = 100124
priority = 4
min_contact_s = 180

[[request]]
satellite = 100125
priority = 2
"#;

const START: &str = "2026-08-01T12:00:00Z";

/// A directory of input files for one test, removed when the test ends.
struct Files {
    dir: PathBuf,
}

impl Files {
    fn new(test: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("maat-cli-{}-{test}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("the temporary directory can be created");
        let files = Self { dir };
        files.write("catalog.json", CATALOG);
        files.write("stations.toml", STATIONS);
        files.write("requests.toml", REQUESTS);
        files
    }

    fn write(&self, name: &str, text: &str) -> PathBuf {
        let path = self.dir.join(name);
        std::fs::write(&path, text).expect("the file can be written");
        path
    }

    fn path(&self, name: &str) -> String {
        self.dir.join(name).display().to_string()
    }

    /// `maat plan` on the standard three files for six hours from the epoch, plus `extra`.
    fn plan(&self, extra: &[&str]) -> Output {
        let (catalog, stations, requests) = (
            self.path("catalog.json"),
            self.path("stations.toml"),
            self.path("requests.toml"),
        );
        let mut args = vec![
            "plan",
            "--catalog",
            &catalog,
            "--stations",
            &stations,
            "--requests",
            &requests,
            "--start",
            START,
            "--hours",
            "6",
        ];
        args.extend_from_slice(extra);
        maat(&args)
    }
}

impl Drop for Files {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn maat(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_maat"))
        .args(args)
        .output()
        .expect("the maat binary runs")
}

fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).expect("stdout is UTF-8")
}

fn stderr(output: &Output) -> String {
    String::from_utf8(output.stderr.clone()).expect("stderr is UTF-8")
}

fn json(output: &Output) -> Value {
    serde_json::from_str(&stdout(output)).expect("stdout is JSON")
}

fn exit_code(output: &Output) -> i32 {
    output.status.code().expect("the process exited normally")
}

// ---------------------------------------------------------------------------------------------
// The plan
// ---------------------------------------------------------------------------------------------

#[test]
fn files_in_a_text_plan_out() {
    let files = Files::new("text");
    let output = files.plan(&[]);
    assert_eq!(exit_code(&output), 0, "{}", stderr(&output));
    let text = stdout(&output);
    // The same six hours as the library's `contact_plan` example, so the same plan.
    assert!(
        text.starts_with(
            "Contact plan (greedy scheduler)\n  14 passes found: 4 scheduled, 10 not scheduled\n  \
             1218 s of scheduled contact, plan value 9096\n"
        ),
        "{text}"
    );
    assert!(
        text.contains("  2026-08-01 16:40:39     432 s  EXAMPLE-B  Quito      57.16 deg    4\n")
    );
    assert!(text.contains("MAAT-107: dropped: lost a tie with an equal-priority pass"));
    assert!(text.contains("  Seattle         120 s  2.00/2.00 deg/s        0 s   0 of 3\n"));
    assert!(text.ends_with("  Cape Town         0 s                -      654 s   2 of 3\n"));

    // Notes go to standard error, never into the plan.
    assert_eq!(
        stderr(&output),
        "search window: 2026-08-01 12:00:00 to 2026-08-01 18:00:00 UTC\n"
    );
}

#[test]
fn the_same_command_gives_the_same_plan() {
    let files = Files::new("repeat");
    let (first, second) = (files.plan(&[]), files.plan(&[]));
    assert!(!stdout(&first).is_empty());
    assert_eq!(stdout(&first), stdout(&second));
}

#[test]
fn requests_can_live_in_the_station_file() {
    let files = Files::new("onefile");
    let both = files.write("scenario.toml", &format!("{STATIONS}\n{REQUESTS}"));
    let output = maat(&[
        "plan",
        "--catalog",
        &files.path("catalog.json"),
        "--stations",
        &both.display().to_string(),
        "--start",
        START,
        "--hours=6",
    ]);
    assert_eq!(exit_code(&output), 0, "{}", stderr(&output));
    assert_eq!(stdout(&output), stdout(&files.plan(&[])));
}

#[test]
fn json_format_is_the_librarys_contact_plan() {
    let files = Files::new("json");
    let output = files.plan(&["--format", "json"]);
    assert_eq!(exit_code(&output), 0, "{}", stderr(&output));
    let plan = json(&output);
    assert_eq!(plan["schema"], "maat.contact-plan/1");
    assert_eq!(plan["scheduler"], "greedy");
    assert_eq!(plan["optimality"], Value::Null);
    assert_eq!(plan["totals"]["passes"], 14);
    assert_eq!(plan["totals"]["scheduled"], 4);
    assert_eq!(plan["entries"].as_array().unwrap().len(), 14);
    assert!(stdout(&output).ends_with("}\n"));
}

#[test]
fn the_optimal_scheduler_with_a_floor_and_a_time_limit() {
    let files = Files::new("optimal");
    let output = files.plan(&[
        "--scheduler",
        "optimal",
        "--floor",
        "--time-limit",
        "30",
        "--format",
        "json",
    ]);
    assert_eq!(exit_code(&output), 0, "{}", stderr(&output));
    let plan = json(&output);
    assert_eq!(plan["scheduler"], "optimal");
    assert_eq!(
        plan["optimality"],
        serde_json::json!({ "proven": true, "time_limit_s": 30.0, "satellite_floor": true })
    );
    // Every satellite with a request has a floor, and is at or above it.
    for satellite in plan["satellites"].as_array().unwrap() {
        if satellite["request"].is_null() {
            assert!(satellite["floor_s"].is_null());
        } else {
            let (floor, got) = (
                satellite["floor_s"].as_u64().unwrap(),
                satellite["contact_scheduled_s"].as_u64().unwrap(),
            );
            assert!(got >= floor, "{satellite}");
        }
    }

    let text = stdout(&files.plan(&["--scheduler", "optimal", "--floor"]));
    assert!(text.starts_with("Contact plan (optimal scheduler)\n"));
    assert!(text.contains("\n  Floor: no satellite has less useful contact than the greedy plan"));
}

#[test]
fn a_time_limit_of_zero_still_gives_a_whole_plan_and_says_it_is_not_proven() {
    let files = Files::new("zero");
    let output = files.plan(&["--scheduler", "optimal", "--time-limit", "0"]);
    assert_eq!(exit_code(&output), 0);
    assert!(stdout(&output).contains("NOT PROVEN OPTIMAL"));
    assert!(stdout(&output).contains("14 passes found"));
    assert!(
        stderr(&output).contains("warning: the search reached its time limit"),
        "{}",
        stderr(&output)
    );
}

// ---------------------------------------------------------------------------------------------
// Exports
// ---------------------------------------------------------------------------------------------

#[test]
fn satnogs_format_lists_the_passes_that_have_their_numbers_and_reports_the_rest() {
    let files = Files::new("satnogs");
    let output = files.plan(&["--format", "satnogs"]);
    assert_eq!(exit_code(&output), 0, "{}", stderr(&output));

    // Four passes are scheduled. EXAMPLE-A over Cape Town twice and EXAMPLE-B over Quito have
    // both a station number and a transmitter. EXAMPLE-D over Svalbard has neither.
    assert_eq!(
        json(&output),
        serde_json::json!([
            { "ground_station": 2433, "transmitter_uuid": "AAAAAAAAAAAAAAAAAAAAA1",
              "start": "2026-08-01T13:13:10Z", "end": "2026-08-01T13:17:20Z" },
            { "ground_station": 2433, "transmitter_uuid": "AAAAAAAAAAAAAAAAAAAAA1",
              "start": "2026-08-01T14:48:58Z", "end": "2026-08-01T14:55:42Z" },
            { "ground_station": 17, "transmitter_uuid": "BBBBBBBBBBBBBBBBBBBBB2",
              "start": "2026-08-01T16:40:39Z", "end": "2026-08-01T16:47:51Z" },
        ])
    );
    let notes = stderr(&output);
    assert!(
        notes.contains(
            "skipped: the pass of EXAMPLE-D over Svalbard at 2026-08-01 12:26:26: station 3 \
             (Svalbard) has no SatNOGS station number and satellite 100125 (EXAMPLE-D) has no \
             SatNOGS transmitter"
        ),
        "{notes}"
    );
    assert!(notes.contains("warning: 1 of 4 scheduled passes are not in the SatNOGS output"));
}

#[test]
fn station_format_writes_one_stations_schedule() {
    let files = Files::new("station");
    let output = files.plan(&["--format", "station", "--station", "4"]);
    assert_eq!(exit_code(&output), 0, "{}", stderr(&output));
    let schedule = json(&output);
    assert_eq!(schedule["schema"], "maat.station-schedule/1");
    assert_eq!(schedule["station"]["name"], "Cape Town");
    assert_eq!(schedule["station"]["latitude_deg"], -33.92);
    let passes = schedule["passes"].as_array().unwrap();
    assert_eq!(passes.len(), 2);
    assert_eq!(passes[0]["satellite"]["norad_cat_id"], 65_001);
    assert_eq!(passes[0]["aos"], "2026-08-01T13:13:10Z");
    assert_eq!(passes[0]["duration_s"], 250);
    // Surveyed windows record where the antenna points.
    assert!(passes[0]["aos_azimuth_deg"].is_f64() && passes[0]["los_azimuth_deg"].is_f64());
}

#[test]
fn station_format_needs_to_know_which_station() {
    let files = Files::new("whichstation");
    let output = files.plan(&["--format", "station"]);
    assert_eq!(exit_code(&output), 1);
    assert!(stdout(&output).is_empty());
    assert_eq!(
        stderr(&output),
        "error: --format station writes one station's schedule and the station file has 4; \
         choose one with --station <ID> (1 Seattle, 2 Quito, 3 Svalbard, 4 Cape Town)\n"
    );

    let output = files.plan(&["--format", "station", "--station", "9"]);
    assert_eq!(exit_code(&output), 1);
    assert!(stderr(&output).contains("--station 9 is not in the station file; it has 1 Seattle"));
}

#[test]
fn output_can_go_to_a_file() {
    let files = Files::new("output");
    let target = files.path("plan.json");
    let to_file = files.plan(&["--format", "json", "--output", &target]);
    assert_eq!(exit_code(&to_file), 0, "{}", stderr(&to_file));
    assert!(stdout(&to_file).is_empty(), "nothing on standard output");
    let written = std::fs::read_to_string(&target).unwrap();
    assert_eq!(written, stdout(&files.plan(&["--format", "json"])));
    assert!(stderr(&to_file).contains(&format!("wrote {} bytes to {target}", written.len())));

    // A file that cannot be written is an error, not a silent loss.
    let nowhere = Path::new(&files.path("no-such-directory")).join("plan.json");
    let failed = files.plan(&["--output", &nowhere.display().to_string()]);
    assert_eq!(exit_code(&failed), 1);
    assert!(
        stderr(&failed).contains("error: cannot write"),
        "{}",
        stderr(&failed)
    );
}

// ---------------------------------------------------------------------------------------------
// Other inputs
// ---------------------------------------------------------------------------------------------

#[test]
fn a_tle_file_works_as_a_catalog() {
    // A public reference element set for the ISS, searched from its own epoch.
    let files = Files::new("tle");
    let tle = files.write(
        "iss.tle",
        "ISS (ZARYA)\n\
         1 25544U 98067A   20194.88612269 -.00002218  00000-0 -31515-4 0  9992\n\
         2 25544  51.6461 221.2784 0001413  89.1723 280.4612 15.49507896236008\n",
    );
    let requests = files.write("iss.toml", "[[request]]\nsatellite = 25544\npriority = 5\n");
    let output = maat(&[
        "plan",
        "--catalog",
        &tle.display().to_string(),
        "--stations",
        &files.path("stations.toml"),
        "--requests",
        &requests.display().to_string(),
        "--start",
        "2020-07-12T21:16:01Z",
        "--hours",
        "24",
        "--format",
        "json",
    ]);
    assert_eq!(exit_code(&output), 0, "{}", stderr(&output));
    let plan = json(&output);
    assert!(
        plan["totals"]["scheduled"].as_u64().unwrap() >= 5,
        "{}",
        plan["totals"]
    );
    assert_eq!(plan["entries"][0]["satellite"]["name"], "ISS (ZARYA)");
    // The ISS never rises above 10 degrees at Svalbard.
    assert_eq!(plan["stations"][2]["passes_found"], 0);
}

#[test]
fn without_a_start_time_the_search_begins_now() {
    let files = Files::new("now");
    let output = maat(&[
        "plan",
        "--catalog",
        &files.path("catalog.json"),
        "--stations",
        &files.path("stations.toml"),
        "--requests",
        &files.path("requests.toml"),
        "--hours",
        "1",
    ]);
    assert_eq!(exit_code(&output), 0, "{}", stderr(&output));
    assert!(stdout(&output).starts_with("Contact plan (greedy scheduler)\n"));
    assert!(
        stderr(&output).contains("search window: 20"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn a_search_far_from_the_elements_epoch_carries_a_warning() {
    let files = Files::new("age");
    let run = |start: &str| {
        maat(&[
            "plan",
            "--catalog",
            &files.path("catalog.json"),
            "--stations",
            &files.path("stations.toml"),
            "--requests",
            &files.path("requests.toml"),
            "--start",
            start,
            "--hours",
            "6",
        ])
    };
    // The elements' epoch is 2026-08-01. Two months later the plan is still made, with a
    // warning; at the epoch there is none.
    let late = run("2026-10-01T12:00:00Z");
    assert_eq!(exit_code(&late), 0);
    assert!(
        stderr(&late).contains(
            "warning: the elements of 5 of 5 satellites are more than 14 days from the search \
             window (furthest: EXAMPLE-A, 61 days)"
        ),
        "{}",
        stderr(&late)
    );
    assert!(!stderr(&run(START)).contains("warning"));
}

#[test]
fn a_request_for_a_satellite_that_is_not_in_the_catalog_is_noted() {
    let files = Files::new("absent");
    let extra = files.write(
        "extra.toml",
        &format!("{REQUESTS}\n[[request]]\nsatellite = 77\npriority = 1\n"),
    );
    let output = maat(&[
        "plan",
        "--catalog",
        &files.path("catalog.json"),
        "--stations",
        &files.path("stations.toml"),
        "--requests",
        &extra.display().to_string(),
        "--start",
        START,
        "--hours",
        "6",
    ]);
    assert_eq!(exit_code(&output), 0);
    assert!(
        stderr(&output).starts_with(
            "note: a request names satellite 77, which is not in the catalog; it has no passes \
             to schedule\n"
        ),
        "{}",
        stderr(&output)
    );
}

// ---------------------------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------------------------

/// Runs `plan` with the given catalog, station and request file names from `files`.
fn plan_with(files: &Files, catalog: &str, stations: &str, requests: Option<&str>) -> Output {
    let (catalog, stations) = (files.path(catalog), files.path(stations));
    let mut args = vec![
        "plan",
        "--catalog",
        &catalog,
        "--stations",
        &stations,
        "--start",
        START,
    ];
    let requests = requests.map(|name| files.path(name));
    if let Some(requests) = &requests {
        args.extend_from_slice(&["--requests", requests]);
    }
    maat(&args)
}

#[test]
fn a_file_that_cannot_be_read_is_named_with_its_role() {
    let files = Files::new("unreadable");
    let output = plan_with(
        &files,
        "missing.json",
        "stations.toml",
        Some("requests.toml"),
    );
    assert_eq!(exit_code(&output), 1);
    assert!(stdout(&output).is_empty());
    let message = stderr(&output);
    assert!(
        message.starts_with("error: cannot read the catalog file '"),
        "{message}"
    );
    assert!(message.contains("missing.json"), "{message}");

    let output = plan_with(&files, "catalog.json", "missing.toml", None);
    assert!(stderr(&output).starts_with("error: cannot read the station file '"));
    let output = plan_with(
        &files,
        "catalog.json",
        "stations.toml",
        Some("missing.toml"),
    );
    assert!(stderr(&output).starts_with("error: cannot read the request file '"));
}

#[test]
fn a_catalog_that_is_not_a_catalog_is_explained_with_a_hint() {
    let files = Files::new("badcatalog");
    files.write("catalog.json", "this is not an element set");
    let output = plan_with(
        &files,
        "catalog.json",
        "stations.toml",
        Some("requests.toml"),
    );
    assert_eq!(exit_code(&output), 1);
    let message = stderr(&output);
    assert!(
        message.contains("catalog.json: could not read the satellite catalog"),
        "{message}"
    );
    assert!(message.contains("\nHint: "), "{message}");
}

#[test]
fn a_mistake_in_the_station_file_is_located() {
    let files = Files::new("badstations");
    files.write(
        "stations.toml",
        "[[station]]\nid = 1\nname = \"Seattle\"\nlatitude_deg = 47.61\nlongitude = -122.33\n",
    );
    let output = plan_with(
        &files,
        "catalog.json",
        "stations.toml",
        Some("requests.toml"),
    );
    assert_eq!(exit_code(&output), 1);
    let message = stderr(&output);
    assert!(message.contains("stations.toml: "), "{message}");
    assert!(
        message.contains("line 5") && message.contains("unknown field `longitude`"),
        "{message}"
    );

    // A value the library refuses keeps the library's explanation.
    files.write(
        "stations.toml",
        "[[station]]\nid = 1\nname = \"Seattle\"\nlatitude_deg = 47.61\nlongitude_deg = 237.67\n\
         minimum_elevation_deg = 10.0\n",
    );
    let output = plan_with(
        &files,
        "catalog.json",
        "stations.toml",
        Some("requests.toml"),
    );
    assert_eq!(exit_code(&output), 1);
    assert!(
        stderr(&output).contains("longitude_deg is 237.67"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn a_plan_needs_stations_and_requests() {
    let files = Files::new("empty");
    let output = plan_with(&files, "catalog.json", "stations.toml", None);
    assert_eq!(exit_code(&output), 1);
    assert!(
        stderr(&output).starts_with("error: no [[request]] table was found"),
        "{}",
        stderr(&output)
    );
    assert!(stderr(&output).contains("[[request]]\nsatellite = 25544\npriority = 5"));

    files.write("stations.toml", "# no stations yet\n");
    let output = plan_with(
        &files,
        "catalog.json",
        "stations.toml",
        Some("requests.toml"),
    );
    assert_eq!(exit_code(&output), 1);
    assert!(
        stderr(&output).contains("has no [[station]] table"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn mistakes_the_scheduler_refuses_carry_their_reason_code() {
    let files = Files::new("duplicate");
    files.write(
        "requests.toml",
        &format!("{REQUESTS}\n[[request]]\nsatellite = 65001\npriority = 1\n"),
    );
    let output = plan_with(
        &files,
        "catalog.json",
        "stations.toml",
        Some("requests.toml"),
    );
    assert_eq!(exit_code(&output), 1);
    assert!(stderr(&output).contains("MAAT-204"), "{}", stderr(&output));
}

#[test]
fn a_bad_command_line_is_a_usage_error_with_status_2() {
    let files = Files::new("usage");
    let output = files.plan(&["--scheduler", "best"]);
    assert_eq!(exit_code(&output), 2);
    assert!(stdout(&output).is_empty());
    let message = stderr(&output);
    assert!(
        message
            .starts_with("error: --scheduler 'best' is not a scheduler; choose greedy or optimal"),
        "{message}"
    );
    assert!(message.contains("Usage: maat <COMMAND>"));

    let output = maat(&["plan"]);
    assert_eq!(exit_code(&output), 2);
    assert!(stderr(&output).starts_with("error: 'plan' needs --catalog <FILE>"));
}

#[test]
fn the_example_files_in_the_repository_give_the_documented_plan() {
    // `examples/` at the top of the repository is what the README tells people to run.
    let examples = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples");
    let output = maat(&[
        "plan",
        "--catalog",
        &examples.join("catalog.json").display().to_string(),
        "--stations",
        &examples.join("stations.toml").display().to_string(),
        "--start",
        START,
        "--hours",
        "6",
    ]);
    assert_eq!(exit_code(&output), 0, "{}", stderr(&output));
    assert!(stdout(&output).contains("14 passes found: 4 scheduled, 10 not scheduled"));
    // The shipped files are the ones these tests use, so the two cannot drift apart. Line
    // endings are set aside: a checkout on Windows may have changed them in either file.
    let unix = |text: &str| text.replace("\r\n", "\n");
    let shipped_catalog = unix(&std::fs::read_to_string(examples.join("catalog.json")).unwrap());
    assert_eq!(shipped_catalog.trim(), unix(CATALOG).trim());
    let shipped = unix(&std::fs::read_to_string(examples.join("stations.toml")).unwrap());
    assert!(shipped.contains(unix(STATIONS).trim()) && shipped.contains(unix(REQUESTS).trim()));
}

#[test]
fn help_describes_the_plan_command() {
    let output = maat(&["--help"]);
    let text = stdout(&output);
    for needle in [
        "plan            Build a contact plan",
        "--catalog <FILE>",
        "--stations <FILE>",
        "--format <NAME>       text, json, satnogs or station",
        "--floor",
    ] {
        assert!(text.contains(needle), "help lacks {needle:?}");
    }
}
