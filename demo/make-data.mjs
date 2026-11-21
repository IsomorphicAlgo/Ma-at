// Rewrites demo/data.js from the files in examples/. Run from the repository root:
//   node demo/make-data.mjs
import { readFileSync, writeFileSync } from "node:fs";

const unix = (text) => text.replace(/\r\n/g, "\n");
const catalog = unix(readFileSync("examples/catalog.json", "utf8"));
const stations = unix(readFileSync("examples/stations.toml", "utf8"));
const current = readFileSync("demo/data.js", "utf8");
const header = current.slice(0, current.indexOf("export const CATALOG"));
writeFileSync(
  "demo/data.js",
  `${header}export const CATALOG = ${JSON.stringify(catalog)};\n\nexport const STATION_FILE = ${JSON.stringify(stations)};\n`,
);
console.log("demo/data.js rewritten from examples/");
