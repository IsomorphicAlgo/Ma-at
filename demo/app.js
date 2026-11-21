// The Ma'at demo page.
//
// Everything about scheduling happens in the WebAssembly module (`crates/maat-wasm`), which is
// `maat-ground` compiled for the browser. This file only gathers the choices, turns them into
// the station file the `maat` command would read, and draws the plan that comes back.

import { CATALOG, STATION_FILE, START } from "./data.js";

// The scheduler's four functions, set once the WebAssembly module has loaded.
let plan, catalog, scenario, version;

const STONES = ["--lapis", "--carnelian", "--malachite", "--turquoise", "--gold", "--amethyst"];
const AGE_WARNING_DAYS = 14;
const SVG = "http://www.w3.org/2000/svg";
const $ = (id) => document.getElementById(id);

const state = {
  catalogText: CATALOG,
  satellites: [], // { id, name, epoch, on, priority, min_contact_s, need_s }
  stations: [], // station tables from the station file, plus { on }
  reply: null,
  step: 0,
  timer: null,
  picked: null,
};

// --- Start-up ---------------------------------------------------------------------------------

async function boot() {
  try {
    // Loaded here and not at the top of the file, so that a missing build is reported on the
    // page and not only in the console.
    const scheduler = await import("./pkg/maat_wasm.js");
    await scheduler.default();
    ({ plan, catalog, scenario, version } = scheduler);
  } catch (error) {
    $("summary").textContent =
      "The scheduler could not be loaded. Build it first, then open this page through a local " +
      "web server; a browser will not load WebAssembly from a file opened directly. The README " +
      "has the two commands.";
    console.error(error);
    return;
  }
  $("version").textContent = `scheduler ${version()}`;

  const files = JSON.parse(scenario(STATION_FILE));
  state.stations = files.stations.map((station) => ({ ...station, on: true }));
  useCatalog(CATALOG, files.requests);
  $("start").value = START;
  $("catalog-text").value = "";

  $("controls").addEventListener("submit", (event) => {
    event.preventDefault();
    build();
  });
  $("controls").addEventListener("change", onChange);
  $("load-catalog").addEventListener("click", loadPastedCatalog);
  $("replay-button").addEventListener("click", startReplay);
  $("step").addEventListener("input", () => {
    stopReplay();
    showStep(Number($("step").value));
  });
  build();
}

/// Reads a catalog and fills the satellite picker. `requests` gives starting priorities.
function useCatalog(text, requests = []) {
  const listing = JSON.parse(catalog(text));
  if (!listing.ok) return listing;
  state.catalogText = text;
  state.satellites = listing.satellites.map((satellite) => {
    const request = requests.find((r) => r.satellite === satellite.id);
    return {
      ...satellite,
      // With requests to go by, a satellite without one starts unticked. Without, all are on.
      on: requests.length === 0 || Boolean(request),
      priority: request ? request.priority : 5,
      min_contact_s: request ? request.min_contact_s : 0,
      need_s: request ? request.need_s : null,
    };
  });
  drawPickers();
  return listing;
}

function loadPastedCatalog() {
  const text = $("catalog-text").value.trim();
  if (!text) return showProblem({ error: "Paste a catalog first.", hint: null });
  const listing = useCatalog(text);
  if (!listing.ok) return showProblem(listing);
  // Start the search at the first satellite's epoch, where its elements are freshest.
  $("start").value = state.satellites[0].epoch;
  build();
}

// --- The pickers ------------------------------------------------------------------------------

function stoneOf(satelliteId) {
  const position = state.satellites.findIndex((s) => s.id === satelliteId);
  return position < 0 ? "var(--unranked)" : `var(${STONES[position % STONES.length]})`;
}

function drawPickers() {
  $("satellites").replaceChildren(
    ...state.satellites.map((satellite, position) => {
      const row = el("label", { class: "pick" });
      row.style.setProperty("--stone", stoneOf(satellite.id));
      const tick = el("input", { type: "checkbox", "data-satellite": position });
      tick.checked = satellite.on;
      const priority = el("input", {
        type: "number", min: 0, max: 255, step: 1, value: satellite.priority,
        "data-priority": position, "aria-label": `Priority of ${satellite.name}`,
      });
      row.append(tick, el("span", { class: "stone" }), el("span", {}, satellite.name), priority);
      return row;
    }),
  );
  $("stations").replaceChildren(
    ...state.stations.map((station, position) => {
      const row = el("label", { class: "pick station" });
      const tick = el("input", { type: "checkbox", "data-station": position });
      tick.checked = station.on;
      const where = `${Math.abs(station.latitude_deg).toFixed(1)}° ${station.latitude_deg < 0 ? "S" : "N"}, ` +
        `${Math.abs(station.longitude_deg).toFixed(1)}° ${station.longitude_deg < 0 ? "W" : "E"}`;
      const text = el("span", {}, station.name, " ", el("span", { class: "where" }, where));
      row.append(tick, text);
      return row;
    }),
  );
  $("station-file").textContent = stationFile();
}

function onChange(event) {
  const target = event.target;
  if (target.dataset.satellite !== undefined) state.satellites[target.dataset.satellite].on = target.checked;
  if (target.dataset.station !== undefined) state.stations[target.dataset.station].on = target.checked;
  if (target.dataset.priority !== undefined) {
    const value = Math.round(Number(target.value));
    state.satellites[target.dataset.priority].priority = Math.min(255, Math.max(0, value || 0));
  }
  if (target.name === "scheduler") {
    const optimal = target.value === "optimal" && target.checked;
    $("floor").disabled = !optimal;
    if (!optimal) $("floor").checked = false;
  }
  $("station-file").textContent = stationFile();
}

/// The chosen stations and satellites as the TOML the `maat` command reads.
function stationFile() {
  const lines = [];
  for (const station of state.stations.filter((s) => s.on)) {
    lines.push("[[station]]", `id = ${station.id}`, `name = ${JSON.stringify(station.name)}`,
      `latitude_deg = ${float(station.latitude_deg)}`, `longitude_deg = ${float(station.longitude_deg)}`,
      `altitude_m = ${float(station.altitude_m)}`,
      `minimum_elevation_deg = ${float(station.minimum_elevation_deg)}`);
    if (station.turnaround_s) lines.push(`turnaround_s = ${station.turnaround_s}`);
    if (station.slew_azimuth_deg_per_s !== null) {
      lines.push(`slew_azimuth_deg_per_s = ${float(station.slew_azimuth_deg_per_s)}`,
        `slew_elevation_deg_per_s = ${float(station.slew_elevation_deg_per_s)}`);
    }
    lines.push("");
  }
  for (const satellite of state.satellites.filter((s) => s.on)) {
    lines.push("[[request]]", `satellite = ${satellite.id}`, `priority = ${satellite.priority}`);
    if (satellite.min_contact_s) lines.push(`min_contact_s = ${satellite.min_contact_s}`);
    if (satellite.need_s !== null) lines.push(`need_s = ${satellite.need_s}`);
    lines.push("");
  }
  return lines.join("\n");
}

/// A number written so that TOML reads it as a float.
const float = (value) => (Number.isInteger(value) ? value.toFixed(1) : String(value));

// --- Building a plan --------------------------------------------------------------------------

function build() {
  stopReplay();
  hideProblem();
  const button = $("build");
  button.disabled = true;
  button.textContent = "Building…";
  // Let the browser paint the button before the scheduler takes the thread.
  requestAnimationFrame(() => setTimeout(() => {
    const reply = JSON.parse(plan(JSON.stringify({
      catalog: state.catalogText,
      scenario: stationFile(),
      start: $("start").value.trim(),
      hours: Number($("hours").value),
      scheduler: document.querySelector('input[name="scheduler"]:checked').value,
      floor: $("floor").checked,
    })));
    button.disabled = false;
    button.textContent = "Build schedule";
    if (!reply.ok) return showProblem(reply);
    state.reply = reply;
    state.picked = null;
    draw();
  }, 0));
}

function showProblem(reply) {
  const box = $("problem");
  box.replaceChildren(el("strong", {}, reply.error));
  if (reply.hint) box.append(reply.hint);
  box.hidden = false;
}

function hideProblem() {
  $("problem").hidden = true;
}

// --- Drawing ----------------------------------------------------------------------------------

function draw() {
  const { plan: result, window: span, build_order: order } = state.reply;
  const totals = result.totals;

  const summary = $("summary");
  summary.replaceChildren(
    `${totals.scheduled} of ${totals.passes} passes scheduled, ${duration(totals.contact_scheduled_s)} of contact.`,
  );
  if (result.optimality || !result.complete) summary.append(" ");
  if (result.optimality) {
    summary.append(result.optimality.proven
      ? "Proven: no plan is worth more."
      : el("span", { class: "flag" }, "Stopped at its time limit: the best plan found, not proven best."));
  }
  if (!result.complete) {
    summary.append(" ", el("span", { class: "flag" },
      `Incomplete: ${result.failures.length} satellite and station pairs could not be searched.`));
  }
  showAgeNote(span);

  drawChart(result, span);
  drawRows(result);
  for (const id of ["timeline", "decisions", "caption"]) $(id).hidden = false;

  const replay = $("replay");
  replay.hidden = order.length === 0;
  $("step").max = order.length;
  if (order.length === 0) {
    setCaption(null, result.scheduler === "optimal"
      ? "The optimal scheduler weighs every pass together, so there is no order to replay. Select a pass to see its reason."
      : "No pass was contested. Select a pass to see its reason.");
    revealAll();
  } else if (matchMedia("(prefers-reduced-motion: reduce)").matches) {
    showStep(order.length);
  } else {
    startReplay();
  }
}

function showAgeNote(span) {
  const far = Math.max(...state.satellites.filter((s) => s.on).map((satellite) => {
    const epoch = Date.parse(satellite.epoch);
    return Math.max(Math.abs(Date.parse(span.start) - epoch), Math.abs(Date.parse(span.end) - epoch));
  }), 0) / 86_400_000;
  const note = $("age-note");
  note.hidden = far <= AGE_WARNING_DAYS;
  note.textContent = `This search runs ${Math.floor(far)} days from the catalog's epoch. ` +
    "Pass times drift as orbital elements age.";
}

function drawChart(result, span) {
  const chart = $("chart");
  const [start, end] = [Date.parse(span.start), Date.parse(span.end)];
  const stations = result.stations;
  const geometry = { width: 1000, left: 150, right: 26, top: 30, row: 46, bar: 24 };
  const height = geometry.top + stations.length * geometry.row + 8;
  const x = (time) => geometry.left + ((time - start) / (end - start)) * (geometry.width - geometry.left - geometry.right);

  chart.setAttribute("viewBox", `0 0 ${geometry.width} ${height}`);
  chart.replaceChildren(svg("title", { id: "chart-title" },
    `Contact schedule by ground station, ${span.start} to ${span.end}`));

  // Hour or day ticks, about a dozen at most.
  const hours = (end - start) / 3_600_000;
  const stepHours = [1, 2, 3, 6, 12, 24, 48].find((step) => hours / step <= 12) ?? 48;
  const first = Math.ceil(start / (stepHours * 3_600_000)) * stepHours * 3_600_000;
  for (let time = first; time <= end; time += stepHours * 3_600_000) {
    const when = new Date(time);
    const label = stepHours >= 24 || when.getUTCHours() === 0
      ? when.toISOString().slice(5, 10)
      : when.toISOString().slice(11, 16);
    chart.append(
      svg("line", { class: "tick", x1: x(time), x2: x(time), y1: geometry.top - 6, y2: height - 4 }),
      svg("text", { class: "axis-text", x: x(time), y: 14, "text-anchor": "middle" }, label),
    );
  }

  stations.forEach((station, row) => {
    const y = geometry.top + row * geometry.row;
    chart.append(
      svg("line", { class: "row-line", x1: geometry.left, x2: geometry.width - geometry.right, y1: y + geometry.row - 6, y2: y + geometry.row - 6 }),
      svg("text", { class: "station-name", x: 0, y: y + 17 }, station.name),
      svg("text", { class: "station-count", x: 0, y: y + 33 },
        `${station.passes_scheduled} of ${station.passes_found} passes`),
    );
  });

  // Passes the scheduler never ranked go underneath; scheduled passes are drawn last, on top.
  const layer = (entry) => (entry.scheduled ? 2 : ranked(entry) ? 1 : 0);
  const entries = [...result.entries].sort((a, b) => layer(a) - layer(b));
  for (const entry of entries) {
    const row = stations.findIndex((station) => station.id === entry.station.id);
    if (row < 0) continue;
    const [aos, los] = [Date.parse(entry.aos), Date.parse(entry.los)];
    const kind = entry.scheduled ? "scheduled" : ranked(entry) ? "refused" : "unranked";
    const inset = kind === "scheduled" ? 0 : 4;
    const group = svg("g", {
      class: `pass ${kind}`, tabindex: 0, role: "button", "data-entry": entry.index,
      "aria-label": `${entry.satellite.name} over ${entry.station.name} at ${clock(entry.aos)}: ${entry.reason.summary}`,
    });
    group.style.setProperty("--stone", stoneOf(entry.satellite.id));
    group.append(svg("rect", {
      x: x(aos), y: geometry.top + row * geometry.row + inset,
      width: Math.max(3, x(los) - x(aos)), height: geometry.bar - 2 * inset, rx: 2,
    }));
    group.addEventListener("click", () => pick(entry.index));
    group.addEventListener("keydown", (event) => {
      if (event.key === "Enter" || event.key === " ") {
        event.preventDefault();
        pick(entry.index);
      }
    });
    chart.append(group);
  }
}

/// Whether the greedy scheduler weighed this pass against others (it is in the build order), or
/// for an optimal plan, whether it had a request at all.
function ranked(entry) {
  const order = state.reply.build_order;
  return order.length ? order.includes(entry.index) : entry.priority !== null && entry.decided_by === "scheduler";
}

function drawRows(result) {
  $("rows").replaceChildren(...result.entries.map((entry) => {
    const row = el("tr", { class: entry.scheduled ? "yes" : "not", "data-row": entry.index });
    row.style.setProperty("--stone", stoneOf(entry.satellite.id));
    row.append(
      el("td", { class: "num" }, clock(entry.aos)),
      el("td", { class: "num" }, `${Math.round(entry.duration_s)} s`),
      el("td", { class: "name" }, el("span", { class: "swatch" }), entry.satellite.name),
      el("td", { class: "name" }, entry.station.name),
      el("td", {},
        el("span", { class: "outcome" }, entry.scheduled ? "Scheduled" : `${entry.reason.code} ${sentence(entry.reason.summary)}`),
        el("span", { class: "detail" }, entry.detail)),
    );
    row.addEventListener("click", () => pick(entry.index));
    return row;
  }));
}

// --- Replay and selection ---------------------------------------------------------------------

function startReplay() {
  stopReplay();
  const order = state.reply.build_order;
  if (!order.length) return;
  // The whole replay takes about eight seconds, however many decisions there are.
  const interval = Math.min(700, Math.max(25, 8000 / order.length));
  showStep(0);
  $("replay-button").textContent = "Stop";
  $("replay-button").onclick = stopReplay;
  state.timer = setInterval(() => {
    showStep(state.step + 1);
    if (state.step >= order.length) stopReplay();
  }, interval);
}

function stopReplay() {
  if (state.timer) clearInterval(state.timer);
  state.timer = null;
  const button = $("replay-button");
  button.textContent = "Replay";
  button.onclick = null;
}

/// Shows the plan as it stood after the scheduler's first `step` decisions.
function showStep(step) {
  const { build_order: order, plan: result } = state.reply;
  state.step = step;
  $("step").value = step;
  $("step-count").textContent = `${step} of ${order.length}`;
  const decided = new Set(order.slice(0, step));
  const current = step > 0 ? order[step - 1] : null;
  for (const group of $("chart").querySelectorAll(".pass")) {
    const index = Number(group.dataset.entry);
    group.classList.toggle("waiting", order.includes(index) && !decided.has(index));
    group.classList.toggle("current", index === current);
    group.classList.remove("picked", "blocker");
  }
  if (current === null) {
    setCaption(null, "The greedy scheduler takes the passes one at a time, best first: highest priority, then the longest pass, then the earliest.");
  } else {
    describe(result.entries[current], `Decision ${step}. `);
  }
}

function revealAll() {
  for (const group of $("chart").querySelectorAll(".pass")) group.classList.remove("waiting", "current");
}

/// Selects a pass from the chart or the table and shows its reason and what beat it.
function pick(index) {
  stopReplay();
  const { plan: result, build_order: order } = state.reply;
  if (order.length && state.step < order.length) showStep(order.length);
  const entry = result.entries[index];
  state.picked = index;
  for (const group of $("chart").querySelectorAll(".pass")) {
    const own = Number(group.dataset.entry);
    group.classList.remove("current");
    group.classList.toggle("picked", own === index);
    group.classList.toggle("blocker", own === entry.blocked_by);
  }
  for (const row of $("rows").children) row.classList.toggle("picked", Number(row.dataset.row) === index);
  describe(entry, "");
}

function describe(entry, lead) {
  const what = `${entry.satellite.name} over ${entry.station.name}, ${clock(entry.aos)}, ${Math.round(entry.duration_s)} s. `;
  setCaption(entry.satellite.id, lead, what,
    el("span", { class: "code" }, entry.scheduled ? "Scheduled. " : `${entry.reason.code}: ${sentence(entry.reason.summary)}. `),
    sentence(entry.detail) + ".");
}

function setCaption(satelliteId, ...content) {
  const caption = $("caption");
  caption.style.setProperty("--stone", satelliteId === null ? "var(--rule)" : stoneOf(satelliteId));
  caption.replaceChildren(...content);
}

// --- Small helpers ----------------------------------------------------------------------------

function el(tag, attributes = {}, ...children) {
  const node = document.createElement(tag);
  for (const [name, value] of Object.entries(attributes)) node.setAttribute(name, value);
  node.append(...children);
  return node;
}

function svg(tag, attributes = {}, ...children) {
  const node = document.createElementNS(SVG, tag);
  for (const [name, value] of Object.entries(attributes)) node.setAttribute(name, value);
  node.append(...children);
  return node;
}

/// "2026-08-01T12:26:26.000Z" as "08-01 12:26:26".
const clock = (iso) => `${iso.slice(5, 10)} ${iso.slice(11, 19)}`;

/// "dropped: lost a tie…" as "Dropped: lost a tie…".
const sentence = (text) => text.charAt(0).toUpperCase() + text.slice(1);

function duration(seconds) {
  if (seconds < 3600) return `${Math.round(seconds / 60)} minutes`;
  return `${(seconds / 3600).toFixed(1)} hours`;
}

boot();
