import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

import { chartMode, drawChart, readTheme, seriesColor, type ChartMode, type ChartTheme, type HoverPoint } from "./chart";
import { buildControlPanel, type ControlPanel, type SensorInfo } from "./fan-control";
import { buildFanView, fanName, renderFan, spinning, type FanView } from "./fans";
import { fmtAgo, fmtClock, fmtLimit, fmtTemp, setStatus, statusOf, type Status } from "./format";
import { buildLoadView, renderLoad, type LoadInput, type LoadView } from "./load";
import type { Category, ControlState, Fan, FanReading, Override, Sample, Sensor, Settings, Snapshot } from "./types";
import "./styles.css";

const WINDOWS = [
  { secs: 60, label: "1 min" },
  { secs: 300, label: "5 min" },
  { secs: 600, label: "10 min" },
  { secs: 1800, label: "30 min" },
];

/** Cards appear in this order; within a category they keep the backend's order. */
const CATEGORY_ORDER: Category[] = ["cpu", "gpu", "memory", "storage", "board", "wireless", "battery", "acpi", "other"];

/** A chart card takes the worst status of its sensors, in this order. */
const SEVERITY: Status[] = ["hot", "warm", "ok", "offline"];

/** Fastest-seen fan speeds are saved at most this often. */
const PEAK_SAVE_MS = 15_000;

const EDIT_ICON =
  '<svg viewBox="0 0 16 16" aria-hidden="true"><path d="M2 4.5h7M12.5 4.5H14M2 11.5h1.5M7 11.5h7"/>' +
  '<circle cx="10.75" cy="4.5" r="1.75"/><circle cx="5.25" cy="11.5" r="1.75"/></svg>';

/** Sensors of one device that share their thresholds. They draw as one chart. */
interface Group {
  /** Indexes into `Snapshot.sensors`. */
  members: number[];
}

interface LegendItem {
  label: HTMLElement;
  value: HTMLElement;
}

/** DOM handles for one chart card. */
interface View {
  group: Group;
  mode: ChartMode;
  card: HTMLElement;
  canvas: HTMLCanvasElement;
  tip: HTMLElement;
  pill: HTMLElement;
  /** Area: [current, peak]. Lines: one per member, then peak. Band: [hottest, coolest, peak]. */
  items: LegendItem[];
  safe: HTMLElement;
  throttle: HTMLElement;
  source: HTMLElement;
}

const $ = <T extends HTMLElement>(selector: string, root: ParentNode = document) =>
  root.querySelector(selector) as T;

let theme: ChartTheme;
let snap: Snapshot | null = null;
let settings: Settings = { overrides: {}, hidden: [], windowSecs: 300, fanPeaks: {}, fanCurves: {} };
let fanControl: ControlState = { notice: null, status: { blockedBy: null, fans: [] } };
/** Refreshes the open fan control dialog when the control state or a reading changes. */
let fanDialogUpdate: (() => void) | null = null;
let views: View[] = [];
let fanViews: FanView[] = [];
let loadView: LoadView | null = null;
/** Ids of the fans the top row shows now; the row is rebuilt when this changes. */
let shownFans = "";
/** Fans seen turning since the app started. */
const spun = new Set<string>();
let peaksChanged = false;
let hover: { view: View; x: number } | null = null;
let loading = false;
let pending: Sample[] = [];
let frame = 0;

// ---------- data ----------

async function load(): Promise<void> {
  if (loading) return;
  loading = true;
  try {
    const [next, saved, control] = await Promise.all([
      invoke<Snapshot>("snapshot"),
      invoke<Settings>("get_settings"),
      invoke<ControlState>("fan_control"),
    ]);
    snap = next;
    settings = saved;
    fanControl = control;
    notePeaks(next.fanReadings);
    const queued = pending;
    pending = [];
    for (const sample of queued) {
      if (sample.generation === next.generation) append(sample);
    }
    build();
    clearNotice();
  } catch (err) {
    showNotice(`Could not read sensors: ${err}`);
  } finally {
    loading = false;
  }
}

function onSample(sample: Sample): void {
  if (loading) {
    pending.push(sample);
    return;
  }
  if (!snap || sample.generation !== snap.generation) {
    // First load failed, or the backend found new sensors.
    void load();
    return;
  }
  append(sample);
  queueRender();
}

function append(sample: Sample): void {
  if (!snap) return;
  const last = snap.times[snap.times.length - 1];
  if (last !== undefined && sample.t <= last) return;
  snap.times.push(sample.t);
  sample.values.forEach((v, i) => snap!.series[i].push(v));
  if (snap.times.length > snap.historyLen) {
    snap.times.shift();
    for (const s of snap.series) s.shift();
  }
  snap.fanReadings = sample.fans;
  notePeaks(sample.fans);
}

/** Remembers which fans turn, and the fastest speed of each. The fastest speed stands in
 *  for the maximum when the hardware reports none. */
function notePeaks(readings: (FanReading | null)[]): void {
  if (!snap) return;
  readings.forEach((r, i) => {
    const id = snap!.fans[i]?.id;
    if (!id || !r || !spinning(r)) return;
    spun.add(id);
    if (r.rpm != null && r.rpm > (settings.fanPeaks[id] ?? 0)) {
      settings.fanPeaks[id] = r.rpm;
      peaksChanged = true;
    }
  });
}

function limitsOf(sensor: Sensor) {
  const o: Override | undefined = settings.overrides[sensor.id];
  return {
    safe: o?.safe ?? sensor.thresholds.safe,
    throttle: o?.throttle ?? sensor.thresholds.throttle,
    custom: o?.safe != null || o?.throttle != null,
  };
}

function deviceName(s: Sensor): string {
  return s.instance ? `${s.device} (${s.instance})` : s.device;
}

function sensorName(s: Sensor): string {
  return s.device === s.label ? deviceName(s) : `${deviceName(s)} · ${s.label}`;
}

function groupName(g: Group): string {
  const first = snap!.sensors[g.members[0]];
  return g.members.length === 1 ? sensorName(first) : deviceName(first);
}

/** Sensor name whose device part shrinks first, so the label that tells sensors apart stays visible. */
function nameNode(s: Sensor, withLabel: boolean): HTMLElement {
  const node = el("span", "name");
  node.append(el("span", "name-device", s.device));
  if (s.instance) node.append(el("span", "name-instance", `(${s.instance})`));
  if (withLabel && s.device !== s.label) node.append(el("span", "name-sep", "·"), el("span", "name-label", s.label));
  return node;
}

/** A fan counts once it has turned, or when its controller asks it to turn.
 *  This drops empty fan headers on desktop boards. */
function fanPresent(i: number): boolean {
  const fan = snap!.fans[i];
  return spun.has(fan.id) || (settings.fanPeaks[fan.id] ?? 0) > 0 || (snap!.fanReadings[i]?.target ?? 0) > 0;
}

function fanShown(i: number): boolean {
  return !settings.hidden.includes(snap!.fans[i].id) && fanPresent(i);
}

/** The fans that the fan control sets: each fan whose speed the app can set, when it is present
 *  or has a setting. Hidden fans count too, because hiding changes only the display. */
function controlledFans(): Fan[] {
  if (!snap) return [];
  return snap.fans.filter((fan, i) => fan.controllable && (fan.id in settings.fanCurves || fanPresent(i)));
}

function shownFanKey(): string {
  return snap ? snap.fans.filter((_, i) => fanShown(i)).map((f) => f.id).join("|") : "";
}

function windowLabel(): string {
  return WINDOWS.find((w) => w.secs === settings.windowSecs)?.label ?? `${settings.windowSecs} s`;
}

async function saveSettings(): Promise<void> {
  try {
    await invoke("save_settings", { settings });
    clearNotice();
  } catch (err) {
    showNotice(String(err));
  }
}

function hideIds(ids: string[]): void {
  settings.hidden = [...new Set([...settings.hidden, ...ids])];
  void saveSettings();
  build();
}

// ---------- DOM building ----------

function el<K extends keyof HTMLElementTagNameMap>(tag: K, className = "", text = ""): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text) node.textContent = text;
  return node;
}

/** Groups visible sensors by device and shared thresholds, in category order. */
function groupSensors(): Group[] {
  const hidden = new Set(settings.hidden);
  const rank = (c: Category) => CATEGORY_ORDER.indexOf(c);
  const visible = snap!.sensors
    .map((sensor, index) => ({ sensor, index }))
    .filter(({ sensor }) => !hidden.has(sensor.id))
    .sort((a, b) => rank(a.sensor.category) - rank(b.sensor.category) || a.index - b.index);
  const groups = new Map<string, Group>();
  for (const { sensor, index } of visible) {
    const { safe, throttle } = limitsOf(sensor);
    const key = [sensor.category, sensor.device, sensor.instance ?? "", safe, throttle].join("|");
    const group = groups.get(key) ?? { members: [] };
    group.members.push(index);
    groups.set(key, group);
  }
  return [...groups.values()];
}

function build(): void {
  if (!snap) return;
  const cards = $("#cards");
  cards.replaceChildren();
  hover = null;
  views = groupSensors().map(buildView);
  cards.append(...views.map((v) => v.card));
  buildGauges();

  $("#empty").hidden = snap.sensors.length > 0;
  const hiddenButton = $<HTMLButtonElement>("#hidden-button");
  hiddenButton.hidden = settings.hidden.length === 0;
  hiddenButton.textContent = `Hidden (${settings.hidden.length})`;
  for (const button of document.querySelectorAll<HTMLButtonElement>("#window button")) {
    button.setAttribute("aria-pressed", String(Number(button.dataset.secs) === settings.windowSecs));
  }
  queueRender();
}

/** The top row: system thermal load, then each shown fan. */
function buildGauges(): void {
  if (!snap) return;
  const gauges = $("#gauges");
  loadView = snap.sensors.length > 0 ? buildLoadView() : null;
  fanViews = snap.fans.flatMap((fan, i) => (fanShown(i) ? [buildFanView(fan, () => hideIds([fan.id]))] : []));
  shownFans = shownFanKey();
  gauges.replaceChildren(...(loadView ? [loadView.card] : []), ...fanViews.map((v) => v.card));
  gauges.hidden = gauges.childElementCount === 0;
  $("#fan-button").hidden = controlledFans().length === 0;
}

function legendItem(parent: HTMLElement, label: string, color?: string, wide = false): LegendItem {
  const item = el("div", wide ? "legend-item wide" : "legend-item");
  const labelNode = el("span", "legend-label", label);
  const value = el("span", "legend-value");
  if (color) {
    const swatch = el("span", "swatch");
    swatch.style.background = color;
    item.append(swatch);
  }
  item.append(labelNode, value);
  parent.append(item);
  return { label: labelNode, value };
}

function buildView(group: Group): View {
  const sensors = group.members.map((i) => snap!.sensors[i]);
  const first = sensors[0];
  const name = groupName(group);
  const mode = chartMode(sensors.length);

  const card = el("article", "card");
  const head = el("header", "card-head");
  const title = el("h3", "card-title");
  title.append(nameNode(first, sensors.length === 1));
  title.title = sensors.length === 1 ? `${name}\n${first.detail}\n${first.id}` : `${name}\n${first.detail}`;
  const pill = el("span", "pill");
  const edit = el("button", "icon-button");
  edit.type = "button";
  edit.title = "Edit thresholds";
  edit.setAttribute("aria-label", `Edit thresholds for ${name}`);
  edit.innerHTML = EDIT_ICON;
  const actions = el("span", "head-actions");
  actions.append(pill, edit);
  head.append(title, actions);

  const plot = el("div", "plot");
  const canvas = el("canvas");
  canvas.setAttribute("role", "img");
  canvas.setAttribute("aria-label", `${name} temperature over time`);
  const tip = el("div", "tip");
  tip.hidden = true;
  plot.append(canvas, tip);

  const legend = el("footer", "legend");
  const items: LegendItem[] = [];
  if (mode === "area") {
    items.push(legendItem(legend, "Current", theme.line));
  } else if (mode === "lines") {
    sensors.forEach((s, k) => items.push(legendItem(legend, s.label, seriesColor(k, sensors.length, theme))));
  } else {
    // Full width: these labels carry a sensor name ("Hottest: Core 7").
    items.push(legendItem(legend, "Hottest", theme.line, true), legendItem(legend, "Coolest", undefined, true));
  }
  items.push(legendItem(legend, "", undefined, mode === "band"));

  const limits = el("div", "limits");
  const safe = el("b");
  const throttle = el("b");
  const safeItem = el("span", "limit");
  safeItem.append(el("span", "dash warn"), "Keep below ", safe);
  const throttleItem = el("span", "limit");
  throttleItem.append(el("span", "dash crit"), "Throttling likely at ", throttle);
  const source = el("span", "source");
  limits.append(safeItem, throttleItem, source);

  card.append(head, plot, legend, limits);
  const view: View = { group, mode, card, canvas, tip, pill, items, safe, throttle, source };

  edit.addEventListener("click", () => openEditor(view));
  canvas.addEventListener("pointermove", (e) => {
    hover = { view, x: e.offsetX };
    queueRender();
  });
  canvas.addEventListener("pointerleave", () => {
    hover = null;
    queueRender();
  });
  return view;
}

// ---------- rendering ----------

function queueRender(): void {
  if (!frame) frame = requestAnimationFrame(render);
}

function render(): void {
  frame = 0;
  if (!snap) return;
  if (shownFanKey() !== shownFans) buildGauges();
  const end = snap.times[snap.times.length - 1] ?? Date.now();
  const start = end - settings.windowSecs * 1000;
  const counts: Record<Status, number> = { ok: 0, warm: 0, hot: 0, offline: 0 };
  const loadInputs: LoadInput[] = [];

  for (const view of views) {
    const sensors = view.group.members.map((i) => snap!.sensors[i]);
    const series = view.group.members.map((i) => snap!.series[i]);
    // Every member shares these; that is what put them in one group.
    const { safe, throttle, custom } = limitsOf(sensors[0]);
    const currents = series.map((values) => values[values.length - 1] ?? null);
    const statuses = currents.map((c) => statusOf(c, safe, throttle));
    statuses.forEach((s) => counts[s]++);
    currents.forEach((c, k) => {
      if (c != null) loadInputs.push({ name: sensorName(sensors[k]), value: c, throttle });
    });

    let peak: number | null = null;
    for (const values of series) {
      for (let i = values.length - 1; i >= 0 && snap.times[i] >= start; i--) {
        const v = values[i];
        if (v != null && (peak == null || v > peak)) peak = v;
      }
    }

    const status = SEVERITY.find((s) => statuses.includes(s)) ?? "offline";
    view.card.dataset.status = status;
    setStatus(view.pill, status);
    renderLegend(view, sensors, currents, peak);
    view.safe.textContent = fmtLimit(safe);
    view.throttle.textContent = fmtLimit(throttle);
    const sources = new Set(sensors.map((s) => s.thresholds.source));
    view.source.textContent = custom ? "custom" : sources.size === 1 ? [...sources][0] : "mixed";
    view.source.title = [...new Set(sensors.map((s) => s.thresholds.basis))].join("\n");

    const point = drawChart(
      view.canvas,
      {
        times: snap.times,
        series,
        start,
        end,
        safe,
        throttle,
        hoverX: hover?.view === view ? hover.x : null,
        spanLabel: windowLabel(),
      },
      theme,
    );
    renderTip(view, sensors, series, point, end);
  }

  if (loadView) {
    const worst = SEVERITY.find((s) => counts[s] > 0) ?? "offline";
    renderLoad(loadView, loadInputs, worst);
  }
  for (const view of fanViews) {
    const i = snap.fans.indexOf(view.fan);
    const id = view.fan.id;
    renderFan(view, snap.fanReadings[i] ?? null, settings.fanPeaks[id], {
      curve: settings.fanCurves[id],
      status: fanControl.status.fans.find((f) => f.fan === id),
      notice: fanControl.notice,
    });
  }
  renderSummary(counts);
  // The curve editor marks the sensor's temperature now.
  fanDialogUpdate?.();
}

interface Reading {
  sensor: Sensor;
  value: number;
}

/** Hottest and coolest of a set of readings, with the sensor each belongs to. */
function extremes(sensors: Sensor[], values: (number | null)[]): { hot: Reading | null; cool: Reading | null } {
  const readings = values.flatMap((value, k) => (value == null ? [] : [{ sensor: sensors[k], value }]));
  if (!readings.length) return { hot: null, cool: null };
  const hot = readings.reduce((a, b) => (b.value > a.value ? b : a));
  const cool = readings.reduce((a, b) => (b.value < a.value ? b : a));
  return { hot, cool };
}

function renderLegend(view: View, sensors: Sensor[], currents: (number | null)[], peak: number | null): void {
  const items = view.items;
  const peakItem = items[items.length - 1];
  peakItem.label.textContent = `Peak, last ${windowLabel()}`;
  peakItem.value.textContent = fmtTemp(peak);
  if (view.mode === "area") {
    items[0].value.textContent = fmtTemp(currents[0]);
  } else if (view.mode === "lines") {
    currents.forEach((c, k) => (items[k].value.textContent = fmtTemp(c)));
  } else {
    const { hot, cool } = extremes(sensors, currents);
    items[0].label.textContent = hot ? `Hottest: ${hot.sensor.label}` : "Hottest";
    items[0].value.textContent = fmtTemp(hot?.value);
    items[1].label.textContent = cool ? `Coolest: ${cool.sensor.label}` : "Coolest";
    items[1].value.textContent = fmtTemp(cool?.value);
  }
}

function renderTip(
  view: View,
  sensors: Sensor[],
  series: (number | null)[][],
  point: HoverPoint | null,
  end: number,
): void {
  if (!point || !snap) {
    view.tip.hidden = true;
    return;
  }
  const at = series.map((values) => values[point.index] ?? null);
  const rows: HTMLElement[] = [];
  const row = (label: string, value: number | null, color?: string) => {
    const r = el("span", "tip-row");
    if (color) {
      const swatch = el("span", "swatch");
      swatch.style.background = color;
      r.append(swatch);
    }
    if (label) r.append(el("span", "", label));
    r.append(el("b", "", fmtTemp(value)));
    rows.push(r);
  };
  if (view.mode === "area") {
    row("", at[0]);
  } else if (view.mode === "lines") {
    sensors.forEach((s, k) => row(s.label, at[k], seriesColor(k, sensors.length, theme)));
  } else {
    const { hot, cool } = extremes(sensors, at);
    row(hot ? `Hottest: ${hot.sensor.label}` : "Hottest", hot?.value ?? null);
    row(cool ? `Coolest: ${cool.sensor.label}` : "Coolest", cool?.value ?? null);
  }
  const t = snap.times[point.index];
  view.tip.hidden = false;
  view.tip.replaceChildren(...rows, el("span", "tip-time", `${fmtClock(t)} · ${fmtAgo(end - t)}`));
  const width = view.tip.offsetWidth;
  const flip = point.x + 12 + width > view.canvas.clientWidth;
  view.tip.style.left = `${flip ? point.x - 12 - width : point.x + 12}px`;
  view.tip.style.top = `${Math.max(4, point.y - 44)}px`;
}

function renderSummary(counts: Record<Status, number>): void {
  const summary = $("#summary");
  summary.replaceChildren();
  const part = (strong: string, rest: string, status?: Status) => {
    const span = el("span", "summary-part");
    if (status) span.dataset.status = status;
    span.append(el("b", "", strong), ` ${rest}`);
    summary.append(span);
  };
  const total = counts.ok + counts.warm + counts.hot + counts.offline;
  part(String(total), total === 1 ? "sensor" : "sensors");
  part(String(counts.warm), "warm", counts.warm ? "warm" : undefined);
  part(String(counts.hot), "throttling", counts.hot ? "hot" : undefined);
  if (counts.offline) part(String(counts.offline), "no data", "offline");
}

// ---------- dialogs ----------

function openEditor(view: View): void {
  if (!snap) return;
  const sensors = view.group.members.map((i) => snap!.sensors[i]);
  const dialog = $<HTMLDialogElement>("#editor");
  const form = $<HTMLFormElement>("form", dialog);
  const { safe, throttle } = limitsOf(sensors[0]);
  const single = sensors.length === 1;

  $("#editor-title").textContent = groupName(view.group);
  $("#editor-detail").textContent = single
    ? `${sensors[0].detail} · ${sensors[0].chip}`
    : `${sensors.length} sensors · ${sensors[0].detail} · ${sensors[0].chip}`;
  $("#editor-basis").textContent = [...new Set(sensors.map((s) => s.thresholds.basis))].join(" ");
  const defaults = new Set(
    sensors.map(
      (s) =>
        `keep below ${fmtLimit(s.thresholds.safe)}, throttling likely at ${fmtLimit(s.thresholds.throttle)} ` +
        `(${s.thresholds.source === "hardware" ? "from hardware" : "estimate"})`,
    ),
  );
  $("#editor-defaults").textContent =
    defaults.size === 1
      ? `Default: ${[...defaults][0]}.${single ? "" : " New values apply to every sensor in this chart."}`
      : "The sensors in this chart have different defaults. Reset restores each one.";

  const members = $("#editor-members");
  members.hidden = single;
  members.replaceChildren();
  if (!single) {
    members.append(el("h3", "", "Sensors in this chart"));
    for (const s of sensors) {
      const row = el("div", "member");
      const hide = el("button", "button", "Hide");
      hide.type = "button";
      hide.addEventListener("click", () => {
        hideIds([s.id]);
        row.remove();
      });
      row.append(el("span", "", s.label), hide);
      members.append(row);
    }
  }

  const safeInput = $<HTMLInputElement>("[name=safe]", form);
  const throttleInput = $<HTMLInputElement>("[name=throttle]", form);
  safeInput.value = String(safe);
  throttleInput.value = String(throttle);
  const error = $("#editor-error");
  error.hidden = true;

  form.onsubmit = (event) => {
    const action = (event.submitter as HTMLButtonElement | null)?.value ?? "save";
    // Sensors hidden from this dialog no longer belong to the chart.
    const remaining = sensors.filter((s) => !settings.hidden.includes(s.id));
    if (action === "save") {
      const s = Number(safeInput.value);
      const t = Number(throttleInput.value);
      const problem =
        !safeInput.value || !throttleInput.value || !Number.isFinite(s) || !Number.isFinite(t)
          ? "Enter both temperatures as numbers."
          : s <= 0 || t >= 200
            ? "Use temperatures between 0 °C and 200 °C."
            : s >= t
              ? "The recommended maximum must be lower than the throttle point."
              : null;
      if (problem) {
        event.preventDefault();
        error.textContent = problem;
        error.hidden = false;
        return;
      }
      for (const sensor of remaining) {
        const override: Override = {
          safe: s === sensor.thresholds.safe ? null : s,
          throttle: t === sensor.thresholds.throttle ? null : t,
        };
        if (override.safe == null && override.throttle == null) delete settings.overrides[sensor.id];
        else settings.overrides[sensor.id] = override;
      }
    } else if (action === "reset") {
      for (const sensor of remaining) delete settings.overrides[sensor.id];
    } else if (action === "hide") {
      hideIds(remaining.map((x) => x.id));
      return;
    } else {
      return;
    }
    void saveSettings();
    build();
  };
  dialog.showModal();
  safeInput.focus();
}

/** One setting for all fans, or a setting for each fan: Automatic, a fixed level, or a curve.
 *  The dialog opens on "All fans" while every fan has the same setting. */
function openFanControl(): void {
  const fans = controlledFans();
  if (!snap || fans.length === 0) return;
  const dialog = $<HTMLDialogElement>("#fan-control");
  const form = $<HTMLFormElement>("form", dialog);
  // The handler of the last open would react to the panels loaded below.
  form.oninput = null;

  const info: SensorInfo = {
    sensors: snap.sensors,
    name: sensorName,
    throttle: (s) => limitsOf(s).throttle,
    now: currentTemp,
  };
  const saved = fans.map((fan) => settings.fanCurves[fan.id]);
  const all = buildControlPanel(info, { subject: fans.length === 1 ? "the fan" : "every fan" });
  all.load(saved[0]);
  const each = fans.map((fan, i) => {
    const panel = buildControlPanel(info, { title: fanName(fan), detail: `${fan.chip} · ${fan.id}`, subject: "the fan" });
    panel.load(saved[i]);
    return panel;
  });
  $("#fan-control-all").replaceChildren(all.root);
  $("#fan-control-fans").replaceChildren(...each.map((p) => p.root));

  type Scope = "all" | "individual";
  const same = new Set(saved.map((c) => JSON.stringify(c ?? null))).size === 1;
  let scope: Scope = same ? "all" : "individual";
  /** Each panel's setting as last seen. The curve editor also sends input events when it only
   *  redraws, so a changed setting, not an event, shows which panel the user changed. */
  const seen = new Map<ControlPanel, string>();
  /** The panel changed last since the dialog opened or the scope changed. */
  let edited: ControlPanel | null = null;
  const look = () => {
    for (const panel of [all, ...each]) {
      const value = JSON.stringify(panel.value());
      if (seen.has(panel) && seen.get(panel) !== value) edited = panel;
      seen.set(panel, value);
    }
  };
  look();
  const scopeButtons = [...document.querySelectorAll<HTMLButtonElement>("#fan-control-scope button")];
  $("#fan-control-scope").hidden = fans.length === 1;
  const names = fans.map(fanName);
  const fanList = names.length === 1 ? names[0] : `${names.slice(0, -1).join(", ")} and ${names[names.length - 1]}`;

  const update = () => {
    for (const button of scopeButtons) button.setAttribute("aria-pressed", String(button.dataset.scope === scope));
    $("#fan-control-all").hidden = scope !== "all";
    $("#fan-control-fans").hidden = scope !== "individual";
    $("#fan-control-detail").textContent =
      fans.length === 1 ? fanList : scope === "all" ? `One setting for ${fanList}.` : "A setting for each fan.";
    for (const panel of scope === "all" ? [all] : each) panel.update();
    const notice = $("#fan-control-notice");
    notice.textContent = fanControl.notice ? `Fan control does not apply now: ${fanControl.notice}.` : "";
    notice.hidden = !fanControl.notice;
  };
  for (const button of scopeButtons) {
    button.onclick = () => {
      const next = button.dataset.scope as Scope;
      if (next === scope) return;
      // A change carries over: all fans take it, or each fan starts from it.
      if (edited) {
        const value = edited.value();
        for (const panel of next === "all" ? [all] : each) panel.load(value);
        look();
      }
      edited = null;
      scope = next;
      update();
    };
  }
  form.oninput = () => {
    look();
    update();
  };
  update();
  fanDialogUpdate = update;
  dialog.onclose = () => (fanDialogUpdate = null);

  form.onsubmit = (event) => {
    const action = (event.submitter as HTMLButtonElement | null)?.value ?? "apply";
    if (action !== "apply") return;
    fans.forEach((fan, i) => {
      const curve = (scope === "all" ? all : each[i]).value();
      if (curve) settings.fanCurves[fan.id] = curve;
      else delete settings.fanCurves[fan.id];
    });
    // Retry after a cancelled password dialog, even when the curves did not change.
    void saveSettings().then(() => invoke("retry_fan_control"));
    queueRender();
  };
  dialog.showModal();
}

/** The sensor's latest reading, or null when it has none. */
function currentTemp(sensor: Sensor): number | null {
  const values = snap?.series[snap.sensors.indexOf(sensor)];
  return values?.[values.length - 1] ?? null;
}

function openHidden(): void {
  if (!snap) return;
  const dialog = $<HTMLDialogElement>("#hidden");
  const list = $("#hidden-list");
  list.replaceChildren();
  const names = new Map<string, string>([
    ...snap.sensors.map((s) => [s.id, sensorName(s)] as [string, string]),
    ...snap.fans.map((f) => [f.id, `${fanName(f)} (fan)`] as [string, string]),
  ]);
  for (const id of settings.hidden) {
    const row = el("li");
    const show = el("button", "button", "Show");
    show.type = "button";
    show.addEventListener("click", () => {
      settings.hidden = settings.hidden.filter((h) => h !== id);
      void saveSettings();
      build();
      if (settings.hidden.length === 0) dialog.close();
      else openHidden();
    });
    row.append(el("span", "", names.get(id) ?? `${id} (not present now)`), show);
    list.append(row);
  }
  if (!dialog.open) dialog.showModal();
}

// ---------- notices ----------

function showNotice(text: string): void {
  const notice = $("#notice");
  notice.textContent = text;
  notice.hidden = false;
}

function clearNotice(): void {
  $("#notice").hidden = true;
}

// ---------- start-up ----------

function buildWindowButtons(): void {
  const group = $("#window");
  for (const w of WINDOWS) {
    const button = el("button", "", w.label);
    button.type = "button";
    button.dataset.secs = String(w.secs);
    button.addEventListener("click", () => {
      settings.windowSecs = w.secs;
      void saveSettings();
      build();
    });
    group.append(button);
  }
}

async function start(): Promise<void> {
  theme = readTheme();
  buildWindowButtons();
  $("#hidden-button").addEventListener("click", openHidden);
  $("#fan-button").addEventListener("click", openFanControl);
  $("#show-all").addEventListener("click", () => {
    settings.hidden = [];
    void saveSettings();
    build();
    $<HTMLDialogElement>("#hidden").close();
  });
  window.addEventListener("resize", queueRender);
  setInterval(() => {
    if (!peaksChanged) return;
    peaksChanged = false;
    void saveSettings();
  }, PEAK_SAVE_MS);
  // Listen first, so no sample is lost between the snapshot and the subscription.
  await listen<Sample>("sample", (event) => onSample(event.payload));
  await listen<ControlState>("fan-control", (event) => {
    fanControl = event.payload;
    fanDialogUpdate?.();
    queueRender();
  });
  await load();
}

void start();
