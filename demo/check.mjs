// Runs the WebAssembly build of the scheduler in Node and compares its plan with the plan the
// native `maat` command writes for the same files. The two must make the same plan: same code,
// same inputs, different machine.
//
//   node demo/check.mjs native-plan.json
//
// where native-plan.json comes from:
//
//   maat plan --catalog examples/catalog.json --stations examples/stations.toml \
//             --start 2026-08-01T12:00:00Z --hours 168 --scheduler optimal --format json
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import init, { plan, catalog, version } from "./pkg/maat_wasm.js";

const here = dirname(fileURLToPath(import.meta.url));
await init({ module_or_path: readFileSync(join(here, "pkg", "maat_wasm_bg.wasm")) });

const catalogText = readFileSync(join(here, "..", "examples", "catalog.json"), "utf8");
const scenario = readFileSync(join(here, "..", "examples", "stations.toml"), "utf8");

const started = performance.now();
const reply = JSON.parse(
  plan(JSON.stringify({
    catalog: catalogText, scenario, start: "2026-08-01T12:00:00Z", hours: 168,
    scheduler: "optimal", time_limit_s: 60,
  })),
);
const seconds = ((performance.now() - started) / 1000).toFixed(2);
if (!reply.ok) {
  console.error("the WebAssembly build failed to plan:", reply.error);
  process.exit(1);
}
const totals = reply.plan.totals;
console.log(`maat-wasm ${version()}: ${totals.scheduled} of ${totals.passes} passes scheduled in ${seconds} s`);
console.log(`catalog lists ${JSON.parse(catalog(catalogText)).satellites.length} satellites`);

const nativePath = process.argv[2];
if (nativePath) {
  const native = JSON.parse(readFileSync(nativePath, "utf8"));
  // Everything must match exactly except floating-point numbers, which may differ in their last
  // digits: the browser and the native build use different implementations of sine and cosine.
  let mismatches = 0;
  let lastDigits = 0;
  let largest = 0;
  const compare = (a, b, path) => {
    if (typeof a === "number" && typeof b === "number") {
      if (a === b) return;
      const difference = Math.abs(a - b);
      if (difference <= 1e-9 * Math.max(1, Math.abs(a))) {
        lastDigits += 1;
        largest = Math.max(largest, difference);
      } else {
        mismatches += 1;
        console.log(`  ${path}: native ${a}, WebAssembly ${b}`);
      }
    } else if (a && b && typeof a === "object" && typeof b === "object") {
      for (const key of new Set([...Object.keys(a), ...Object.keys(b)])) {
        compare(a[key], b[key], `${path}.${key}`);
      }
    } else if (a !== b) {
      mismatches += 1;
      console.log(`  ${path}: native ${JSON.stringify(a)}, WebAssembly ${JSON.stringify(b)}`);
    }
  };
  compare(native, reply.plan, "plan");
  if (mismatches === 0) {
    console.log(
      `same plan as the native build: every pass, time, reason and total matches` +
        (lastDigits ? `; ${lastDigits} numbers differ in their last digits (by at most ${largest.toExponential(1)})` : ""),
    );
  } else {
    console.log(`DIFFERENT from the native plan in ${mismatches} places`);
  }
  process.exit(mismatches === 0 ? 0 : 1);
}
