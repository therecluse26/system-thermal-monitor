import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

import { drawChart, readTheme, type ChartTheme, type HoverPoint } from "./chart";
import { buildFanView, renderFan, type FanView } from "./fans";
import { fmtAgo, fmtClock, fmtLimit, fmtTemp, setStatus, statusOf, type Status } from "./format";
import { buildLoadView, renderLoad, type LoadInput, type LoadView } from "./load";
import type { Category, FanReading, Override, Sample, Sensor, Settings, Snapshot } from "./types";
import "./styles.css";

const WINDOWS = [
  { secs: 60, label: "1 min" },
  { secs: 300, label: "5 min" },
  { secs: 600, label: "10 min" },
  { secs: 1800, label: "30 min" },
];

/** Cards appear in this order; within a category they keep the backend's order. */
const CATEGORY_ORDER: Category[] = ["cpu", "gpu", "memory", "storage", "board", "wireless", "battery", "acpi", "other"];

const EDIT_ICON =
  '<svg viewBox="0 0 16 16" aria-hidden="true"><path d="M2 4.5h7M12.5 4.5H14M2 11.5h1.5M7 11.5h7"/>' +
  '<circle cx="10.75" cy="4.5" r="1.75"/><circle cx="5.25" cy="11.5" r="1.75"/></svg>';

/** Fastest-seen fan speeds are saved at most this often. */
const PEAK_SAVE_MS = 15_000;

/** DOM handles for one sensor's chart card. */
interface View {
  index: number;
  card: HTMLElement;
  canvas: HTMLCanvasElement;
  tip: HTMLElement;
  pill: HTMLElement;
  now: HTMLElement;
  peakLabel: HTMLElement;
  peak: HTMLElement;
  safe: HTMLElement;
  throttle: HTMLElement;
  source: HTMLElement;
}

const $ = <T extends HTMLElement>(selector: string, root: ParentNode = document) =>
  root.querySelector(selector) as T;

let theme: ChartTheme;
let snap: Snapshot | null = null;
let settings: Settings = { overrides: {}, hidden: [], windowSecs: 300, fanPeaks: {} };
let views: View[] = [];
let fanViews: FanView[] = [];
let loadView: LoadView | null = null;
let peaksChanged = false;
let hover: { index: number; x: number } | null = null;
let loading = false;
let pending: Sample[] = [];
let frame = 0;

// ---------- data ----------

async function load(): Promise<void> {
  if (loading) return;
  loading = true;
  try {
    const [next, saved] = await Promise.all([
      invoke<Snapshot>("snapshot"),
      invoke<Settings>("get_settings"),
    ]);
    snap = next;
    settings = saved;
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

/** Remembers the fastest speed of each fan; it stands in for the maximum when the hardware reports none. */
function notePeaks(readings: (FanReading | null)[]): void {
  if (!snap) return;
  readings.forEach((r, i) => {
    const id = snap!.fans[i]?.id;
    if (id && r && r.rpm > (settings.fanPeaks[id] ?? 0)) {
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

function sensorName(s: Sensor): string {
  const device = s.instance ? `${s.device} (${s.instance})` : s.device;
  return s.device === s.label ? device : `${device} · ${s.label}`;
}

/** Sensor name whose device part shrinks first, so the label that tells sensors apart stays visible. */
function nameNode(s: Sensor, className: string): HTMLElement {
  const node = el("span", `name ${className}`);
  node.append(el("span", "name-device", s.device));
  if (s.instance) node.append(el("span", "name-instance", `(${s.instance})`));
  if (s.device !== s.label) node.append(el("span", "name-sep", "·"), el("span", "name-label", s.label));
  return node;
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

// ---------- DOM building ----------

function el<K extends keyof HTMLElementTagNameMap>(tag: K, className = "", text = ""): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text) node.textContent = text;
  return node;
}

function build(): void {
  if (!snap) return;
  const cards = $("#cards");
  cards.replaceChildren();
  views = [];
  hover = null;

  const gauges = $("#gauges");
  loadView = snap.sensors.length > 0 ? buildLoadView() : null;
  fanViews = snap.fans.map(buildFanView);
  gauges.replaceChildren(...(loadView ? [loadView.card] : []), ...fanViews.map((v) => v.card));
  gauges.hidden = gauges.childElementCount === 0;

  const hidden = new Set(settings.hidden);
  const rank = (c: Category) => CATEGORY_ORDER.indexOf(c);
  const visible = snap.sensors
    .map((sensor, index) => ({ sensor, index }))
    .filter(({ sensor }) => !hidden.has(sensor.id))
    .sort((a, b) => rank(a.sensor.category) - rank(b.sensor.category) || a.index - b.index);
  for (const { sensor, index } of visible) {
    const view = buildView(index, sensor);
    cards.append(view.card);
    views.push(view);
  }

  $("#empty").hidden = snap.sensors.length > 0;
  const hiddenButton = $<HTMLButtonElement>("#hidden-button");
  hiddenButton.hidden = settings.hidden.length === 0;
  hiddenButton.textContent = `Hidden (${settings.hidden.length})`;
  for (const button of document.querySelectorAll<HTMLButtonElement>("#window button")) {
    button.setAttribute("aria-pressed", String(Number(button.dataset.secs) === settings.windowSecs));
  }
  queueRender();
}

function buildView(index: number, sensor: Sensor): View {
  const name = sensorName(sensor);

  const card = el("article", "card");
  card.id = `card-${index}`;
  const head = el("header", "card-head");
  const title = el("h3", "card-title");
  title.append(nameNode(sensor, ""));
  title.title = `${name}\n${sensor.detail}\n${sensor.id}`;
  const pill = el("span", "pill");
  const edit = el("button", "icon-button");
  edit.type = "button";
  edit.title = "Edit thresholds";
  edit.setAttribute("aria-label", `Edit thresholds for ${name}`);
  edit.innerHTML = EDIT_ICON;
  edit.addEventListener("click", () => openEditor(index));
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
  canvas.addEventListener("pointermove", (e) => {
    hover = { index, x: e.offsetX };
    queueRender();
  });
  canvas.addEventListener("pointerleave", () => {
    hover = null;
    queueRender();
  });

  const legend = el("footer", "legend");
  const nowItem = el("div", "legend-item");
  const now = el("span", "legend-value");
  nowItem.append(el("span", "swatch"), el("span", "legend-label", "Current"), now);
  const peakItem = el("div", "legend-item");
  const peakLabel = el("span", "legend-label");
  const peak = el("span", "legend-value");
  peakItem.append(peakLabel, peak);
  legend.append(nowItem, peakItem);

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

  return { index, card, canvas, tip, pill, now, peakLabel, peak, safe, throttle, source };
}

// ---------- rendering ----------

function queueRender(): void {
  if (!frame) frame = requestAnimationFrame(render);
}

function render(): void {
  frame = 0;
  if (!snap) return;
  const end = snap.times[snap.times.length - 1] ?? Date.now();
  const start = end - settings.windowSecs * 1000;
  const counts: Record<Status, number> = { ok: 0, warm: 0, hot: 0, offline: 0 };
  const loadInputs: LoadInput[] = [];

  for (const view of views) {
    const sensor = snap.sensors[view.index];
    const values = snap.series[view.index];
    const { safe, throttle, custom } = limitsOf(sensor);
    const current = values[values.length - 1] ?? null;
    const status = statusOf(current, safe, throttle);
    counts[status]++;
    if (current != null) loadInputs.push({ name: sensorName(sensor), value: current, throttle });

    let peak: number | null = null;
    for (let i = values.length - 1; i >= 0 && snap.times[i] >= start; i--) {
      const v = values[i];
      if (v != null && (peak == null || v > peak)) peak = v;
    }

    view.card.dataset.status = status;
    setStatus(view.pill, status);
    view.now.textContent = fmtTemp(current);
    view.peakLabel.textContent = `Peak, last ${windowLabel()}`;
    view.peak.textContent = fmtTemp(peak);
    view.safe.textContent = fmtLimit(safe);
    view.throttle.textContent = fmtLimit(throttle);
    view.source.textContent = custom ? "custom" : sensor.thresholds.source;
    view.source.title = sensor.thresholds.basis;

    const point = drawChart(
      view.canvas,
      {
        times: snap.times,
        values,
        start,
        end,
        safe,
        throttle,
        hoverX: hover?.index === view.index ? hover.x : null,
        spanLabel: windowLabel(),
      },
      theme,
    );
    renderTip(view, point, end);
  }

  fanViews.forEach((view, i) => renderFan(view, snap!.fanReadings[i] ?? null, settings.fanPeaks[view.fan.id]));
  if (loadView) {
    const worst: Status = counts.hot ? "hot" : counts.warm ? "warm" : counts.ok ? "ok" : "offline";
    renderLoad(loadView, loadInputs, worst);
  }
  renderSummary(counts);
}

function renderTip(view: View, point: HoverPoint | null, end: number): void {
  if (!point) {
    view.tip.hidden = true;
    return;
  }
  view.tip.hidden = false;
  view.tip.replaceChildren(
    el("b", "", fmtTemp(point.value)),
    el("span", "", `${fmtClock(point.t)} · ${fmtAgo(end - point.t)}`),
  );
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
  part(String(views.length), views.length === 1 ? "sensor" : "sensors");
  part(String(counts.warm), "warm", counts.warm ? "warm" : undefined);
  part(String(counts.hot), "throttling", counts.hot ? "hot" : undefined);
  if (counts.offline) part(String(counts.offline), "no data", "offline");
}

// ---------- dialogs ----------

function openEditor(index: number): void {
  if (!snap) return;
  const sensor = snap.sensors[index];
  const dialog = $<HTMLDialogElement>("#editor");
  const form = $<HTMLFormElement>("form", dialog);
  const { safe, throttle } = limitsOf(sensor);
  const defaults = sensor.thresholds;

  $("#editor-title").textContent = sensorName(sensor);
  $("#editor-detail").textContent = `${sensor.detail} · ${sensor.chip}`;
  $("#editor-basis").textContent = defaults.basis;
  $("#editor-defaults").textContent =
    `Default: keep below ${fmtLimit(defaults.safe)}, throttling likely at ${fmtLimit(defaults.throttle)} ` +
    `(${defaults.source === "hardware" ? "from hardware" : "estimate"}).`;
  const safeInput = $<HTMLInputElement>("[name=safe]", form);
  const throttleInput = $<HTMLInputElement>("[name=throttle]", form);
  safeInput.value = String(safe);
  throttleInput.value = String(throttle);
  const error = $("#editor-error");
  error.hidden = true;

  form.onsubmit = (event) => {
    const action = (event.submitter as HTMLButtonElement | null)?.value ?? "save";
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
      const override: Override = {
        safe: s === defaults.safe ? null : s,
        throttle: t === defaults.throttle ? null : t,
      };
      if (override.safe == null && override.throttle == null) delete settings.overrides[sensor.id];
      else settings.overrides[sensor.id] = override;
    } else if (action === "reset") {
      delete settings.overrides[sensor.id];
    } else if (action === "hide") {
      settings.hidden = [...new Set([...settings.hidden, sensor.id])];
    } else {
      return;
    }
    void saveSettings();
    build();
  };
  dialog.showModal();
  safeInput.focus();
}

function openHidden(): void {
  if (!snap) return;
  const dialog = $<HTMLDialogElement>("#hidden");
  const list = $("#hidden-list");
  list.replaceChildren();
  const byId = new Map(snap.sensors.map((s) => [s.id, s]));
  for (const id of settings.hidden) {
    const sensor = byId.get(id);
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
    row.append(el("span", "", sensor ? sensorName(sensor) : `${id} (not present now)`), show);
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
  await load();
}

void start();
