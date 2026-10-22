// Fan curve editor: the PWM duty at each temperature, as points joined by straight lines.
// Drag a point, or focus it and use the arrow keys (Shift for steps of 5).
// Double-click the grid to add a point; Delete removes the focused one.

import type { CurvePoint } from "./types";

const SVG = "http://www.w3.org/2000/svg";
const WIDTH = 480;
const HEIGHT = 220;
const PAD = { left: 38, right: 12, top: 10, bottom: 24 };
const PLOT_W = WIDTH - PAD.left - PAD.right;
const PLOT_H = HEIGHT - PAD.top - PAD.bottom;
/** A curve needs two points to be a line; one point is a fixed level. */
const MIN_POINTS = 2;
/** Point temperatures the editor accepts, in °C. */
const TEMP_RANGE = [0, 150] as const;

/** The first curve of a fan that had none. */
export const DEFAULT_CURVE: CurvePoint[] = [
  { temp: 40, duty: 20 },
  { temp: 60, duty: 40 },
  { temp: 75, duty: 70 },
  { temp: 85, duty: 100 },
];

/** Duty in percent at `temp`, by the fan helper's rule (`FanCurve::duty_at`): a straight line
 *  between two points, and the nearest point's duty outside them. `points` are in temperature order. */
export function dutyAt(points: CurvePoint[], temp: number): number {
  if (points.length === 0) return 100;
  const first = points[0];
  const last = points[points.length - 1];
  if (temp <= first.temp) return first.duty;
  if (temp >= last.temp) return last.duty;
  const i = points.findIndex((p) => p.temp > temp);
  const a = points[i - 1];
  const b = points[i];
  return a.duty + ((temp - a.temp) / (b.temp - a.temp)) * (b.duty - a.duty);
}

export interface CurveEditor {
  root: HTMLElement;
  /** A copy, in temperature order. */
  points(): CurvePoint[];
  setPoints(points: CurvePoint[]): void;
  /** Marks the sensor's temperature now, and the failsafe temperature: full speed from there. */
  setMarkers(now: number | null, failsafe: number | null): void;
}

const clamp = (v: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, v));

function svgEl<K extends keyof SVGElementTagNameMap>(
  tag: K,
  attrs: Record<string, string | number> = {},
): SVGElementTagNameMap[K] {
  const node = document.createElementNS(SVG, tag);
  for (const [name, value] of Object.entries(attrs)) node.setAttribute(name, String(value));
  return node;
}

function htmlEl<K extends keyof HTMLElementTagNameMap>(tag: K, className = "", text = ""): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text) node.textContent = text;
  return node;
}

export function buildCurveEditor(): CurveEditor {
  let points: CurvePoint[] = DEFAULT_CURVE.map((p) => ({ ...p }));
  let now: number | null = null;
  let failsafe: number | null = null;
  // The temperature axis. It changes only with the points or the failsafe, never during a drag.
  let tMin = 20;
  let tMax = 100;
  let dragging: number | null = null;

  const root = htmlEl("div", "curve-editor");
  const svg = svgEl("svg", { viewBox: `0 0 ${WIDTH} ${HEIGHT}`, role: "group", "aria-label": "Fan curve" });
  const grid = svgEl("g", { class: "curve-grid" });
  const zone = svgEl("rect", { class: "curve-zone", y: PAD.top, height: PLOT_H });
  const failsafeLine = svgEl("line", { class: "curve-failsafe", y1: PAD.top, y2: PAD.top + PLOT_H });
  const fill = svgEl("path", { class: "curve-fill" });
  const line = svgEl("path", { class: "curve-line" });
  const nowLine = svgEl("line", { class: "curve-now", y1: PAD.top, y2: PAD.top + PLOT_H });
  const nowDot = svgEl("circle", { class: "curve-now-dot", r: 3.5 });
  const nowLabel = svgEl("text", { class: "curve-now-label", y: PAD.top + 12 });
  const handles = svgEl("g");
  svg.append(grid, zone, failsafeLine, fill, line, nowLine, nowDot, nowLabel, handles);

  const hint = htmlEl(
    "p",
    "muted curve-hint",
    "Drag a point, or focus it and use the arrow keys. Double-click to add a point; Delete removes one.",
  );
  const list = htmlEl("div", "curve-points");
  const rows = htmlEl("div", "curve-rows");
  const add = htmlEl("button", "button", "Add point");
  add.type = "button";
  list.append(rows, add);
  root.append(svg, hint, list);

  const x = (temp: number) => PAD.left + ((temp - tMin) / (tMax - tMin)) * PLOT_W;
  const y = (duty: number) => PAD.top + (1 - duty / 100) * PLOT_H;
  const tempAtX = (px: number) => tMin + ((px - PAD.left) / PLOT_W) * (tMax - tMin);
  const dutyAtY = (py: number) => (1 - (py - PAD.top) / PLOT_H) * 100;

  /** Fits the axis to the points and the failsafe, in steps of 10 °C. */
  function rescale(): void {
    const temps = points.map((p) => p.temp);
    tMin = Math.floor(Math.min(20, ...temps) / 10) * 10;
    const top = Math.max(100, ...temps, failsafe != null ? failsafe + 5 : 0);
    tMax = Math.ceil(top / 10) * 10;
    grid.replaceChildren();
    for (let t = Math.ceil(tMin / 10) * 10; t <= tMax; t += 10) {
      grid.append(
        svgEl("line", { x1: x(t), x2: x(t), y1: PAD.top, y2: PAD.top + PLOT_H }),
        Object.assign(svgEl("text", { x: x(t), y: HEIGHT - 8, "text-anchor": "middle" }), { textContent: `${t}°` }),
      );
    }
    for (const d of [0, 25, 50, 75, 100]) {
      grid.append(
        svgEl("line", { x1: PAD.left, x2: PAD.left + PLOT_W, y1: y(d), y2: y(d) }),
        Object.assign(svgEl("text", { x: PAD.left - 6, y: y(d) + 3.5, "text-anchor": "end" }), {
          textContent: `${d}%`,
        }),
      );
    }
  }

  /** Moves point `i`, keeping the points in temperature order, 1 °C apart at least. */
  function move(i: number, temp: number, duty: number): void {
    const lo = i > 0 ? points[i - 1].temp + 1 : tMin;
    const hi = i < points.length - 1 ? points[i + 1].temp - 1 : tMax;
    points[i] = { temp: clamp(Math.round(temp), lo, hi), duty: clamp(Math.round(duty), 0, 100) };
    render();
  }

  /** Adds a point unless one sits within 1 °C. Returns its index. */
  function insert(temp: number, duty: number): number | null {
    const t = clamp(Math.round(temp), TEMP_RANGE[0], TEMP_RANGE[1]);
    if (points.some((p) => Math.abs(p.temp - t) < 1)) return null;
    const i = points.findIndex((p) => p.temp > t);
    const at = i < 0 ? points.length : i;
    points.splice(at, 0, { temp: t, duty: clamp(Math.round(duty), 0, 100) });
    rescale();
    render();
    return at;
  }

  function remove(i: number): void {
    if (points.length <= MIN_POINTS) return;
    points.splice(i, 1);
    rescale();
    render();
  }

  function focusHandle(i: number | null): void {
    if (i != null) (handles.children[i] as SVGCircleElement | undefined)?.focus();
  }

  function render(): void {
    const coords = points.map((p) => `${x(p.temp).toFixed(1)},${y(p.duty).toFixed(1)}`);
    // Flat before the first point and after the last, as the helper runs it.
    const left = `${PAD.left},${y(points[0].duty).toFixed(1)}`;
    const right = `${PAD.left + PLOT_W},${y(points[points.length - 1].duty).toFixed(1)}`;
    const path = `M${[left, ...coords, right].join("L")}`;
    line.setAttribute("d", path);
    fill.setAttribute("d", `${path}L${PAD.left + PLOT_W},${PAD.top + PLOT_H}L${PAD.left},${PAD.top + PLOT_H}Z`);

    while (handles.childElementCount > points.length) handles.lastElementChild!.remove();
    while (handles.childElementCount < points.length) {
      handles.append(svgEl("circle", { class: "curve-point", r: 6, tabindex: 0 }));
    }
    points.forEach((p, i) => {
      const handle = handles.children[i] as SVGCircleElement;
      handle.setAttribute("cx", x(p.temp).toFixed(1));
      handle.setAttribute("cy", y(p.duty).toFixed(1));
      handle.setAttribute("aria-label", `Point ${i + 1}: ${p.temp} °C, ${p.duty}%`);
      handle.dataset.index = String(i);
    });
    renderMarkers();
    renderRows();
    // The dialog reacts to the editor as to any other field.
    root.dispatchEvent(new Event("input", { bubbles: true }));
  }

  function renderMarkers(): void {
    const showFailsafe = failsafe != null && failsafe < tMax;
    zone.style.display = failsafeLine.style.display = showFailsafe ? "" : "none";
    if (showFailsafe) {
      const fx = x(Math.max(failsafe!, tMin));
      zone.setAttribute("x", fx.toFixed(1));
      zone.setAttribute("width", (PAD.left + PLOT_W - fx).toFixed(1));
      failsafeLine.setAttribute("x1", fx.toFixed(1));
      failsafeLine.setAttribute("x2", fx.toFixed(1));
    }

    const showNow = now != null;
    nowLine.style.display = nowDot.style.display = nowLabel.style.display = showNow ? "" : "none";
    if (now == null) return;
    const t = clamp(now, tMin, tMax);
    const full = failsafe != null && now >= failsafe;
    const duty = full ? 100 : dutyAt(points, now);
    const nx = x(t);
    nowLine.setAttribute("x1", nx.toFixed(1));
    nowLine.setAttribute("x2", nx.toFixed(1));
    nowDot.setAttribute("cx", nx.toFixed(1));
    nowDot.setAttribute("cy", y(duty).toFixed(1));
    nowLabel.textContent = `now ${Math.round(now)}° → ${Math.round(duty)}%${full ? " (failsafe)" : ""}`;
    // Keep the label inside the plot: right of the line, or left of it near the right edge.
    const leftOfLine = nx > PAD.left + PLOT_W * 0.6;
    nowLabel.setAttribute("x", (leftOfLine ? nx - 6 : nx + 6).toFixed(1));
    nowLabel.setAttribute("text-anchor", leftOfLine ? "end" : "start");
  }

  /** The point list. A field you are typing in keeps its text. */
  function renderRows(): void {
    while (rows.childElementCount > points.length) rows.lastElementChild!.remove();
    while (rows.childElementCount < points.length) rows.append(buildRow());
    points.forEach((p, i) => {
      const row = rows.children[i] as HTMLElement;
      row.dataset.index = String(i);
      const [temp, duty] = row.querySelectorAll("input");
      if (document.activeElement !== temp) temp.value = String(p.temp);
      if (document.activeElement !== duty) duty.value = String(p.duty);
      const removeButton = row.querySelector("button")!;
      removeButton.disabled = points.length <= MIN_POINTS;
      removeButton.setAttribute("aria-label", `Remove point ${i + 1}`);
      row.querySelector(".curve-row-name")!.textContent = `Point ${i + 1}`;
    });
  }

  function buildRow(): HTMLElement {
    const row = htmlEl("div", "curve-row");
    const field = (unit: string, max: number) => {
      const label = htmlEl("label", "input-unit");
      const input = htmlEl("input");
      input.type = "number";
      input.min = String(TEMP_RANGE[0]);
      input.max = String(max);
      input.step = "1";
      label.append(input, ` ${unit}`);
      return label;
    };
    const removeButton = htmlEl("button", "icon-button", "×");
    removeButton.type = "button";
    row.append(htmlEl("span", "curve-row-name"), field("°C", TEMP_RANGE[1]), field("%", 100), removeButton);
    return row;
  }

  // ---------- pointer and keyboard ----------

  const toSvg = (e: MouseEvent) => {
    const matrix = svg.getScreenCTM();
    return matrix ? new DOMPoint(e.clientX, e.clientY).matrixTransform(matrix.inverse()) : null;
  };
  const indexOf = (target: EventTarget | null) =>
    target instanceof SVGCircleElement && target.dataset.index != null ? Number(target.dataset.index) : null;

  svg.addEventListener("pointerdown", (e) => {
    const i = indexOf(e.target);
    if (i == null) return;
    e.preventDefault();
    dragging = i;
    svg.setPointerCapture(e.pointerId);
    focusHandle(i);
  });
  svg.addEventListener("pointermove", (e) => {
    if (dragging == null) return;
    const p = toSvg(e);
    if (p) move(dragging, tempAtX(p.x), dutyAtY(p.y));
  });
  const stop = () => (dragging = null);
  svg.addEventListener("pointerup", stop);
  svg.addEventListener("pointercancel", stop);

  svg.addEventListener("dblclick", (e) => {
    if (indexOf(e.target) != null) return;
    const p = toSvg(e);
    if (!p || p.x < PAD.left || p.x > PAD.left + PLOT_W || p.y < PAD.top || p.y > PAD.top + PLOT_H) return;
    focusHandle(insert(tempAtX(p.x), dutyAtY(p.y)));
  });

  svg.addEventListener("keydown", (e) => {
    const i = indexOf(e.target);
    if (i == null) return;
    const step = e.shiftKey ? 5 : 1;
    const p = points[i];
    const moves: Record<string, [number, number]> = {
      ArrowLeft: [-step, 0],
      ArrowRight: [step, 0],
      ArrowUp: [0, step],
      ArrowDown: [0, -step],
    };
    if (e.key in moves) {
      const [dt, dd] = moves[e.key];
      move(i, p.temp + dt, p.duty + dd);
    } else if (e.key === "Delete" || e.key === "Backspace") {
      remove(i);
      focusHandle(Math.min(i, points.length - 1));
    } else {
      return;
    }
    e.preventDefault();
  });

  rows.addEventListener("change", (e) => {
    const row = (e.target as HTMLElement).closest<HTMLElement>(".curve-row");
    if (!row) return;
    const i = Number(row.dataset.index);
    const [temp, duty] = [...row.querySelectorAll("input")].map((input) => Number(input.value));
    const t = clamp(Math.round(Number.isFinite(temp) ? temp : points[i].temp), TEMP_RANGE[0], TEMP_RANGE[1]);
    // Two points at one temperature would make a vertical step; keep the old value instead.
    if (!points.some((p, k) => k !== i && p.temp === t)) points[i].temp = t;
    points[i].duty = clamp(Math.round(Number.isFinite(duty) ? duty : points[i].duty), 0, 100);
    points.sort((a, b) => a.temp - b.temp);
    (e.target as HTMLInputElement).blur();
    rescale();
    render();
  });
  rows.addEventListener("click", (e) => {
    const button = (e.target as HTMLElement).closest("button");
    const row = button?.closest<HTMLElement>(".curve-row");
    if (row) remove(Number(row.dataset.index));
  });
  add.addEventListener("click", () => {
    // Halve the widest gap between two points.
    let at = 1;
    for (let i = 1; i < points.length; i++) {
      if (points[i].temp - points[i - 1].temp > points[at].temp - points[at - 1].temp) at = i;
    }
    const t = (points[at - 1].temp + points[at].temp) / 2;
    insert(t, dutyAt(points, t));
  });

  return {
    root,
    points: () => points.map((p) => ({ ...p })),
    setPoints(next) {
      const sorted = [...next].sort((a, b) => a.temp - b.temp).map((p) => ({ ...p }));
      points = sorted.length >= MIN_POINTS ? sorted : DEFAULT_CURVE.map((p) => ({ ...p }));
      rescale();
      render();
    },
    setMarkers(nextNow, nextFailsafe) {
      now = nextNow;
      if (nextFailsafe !== failsafe) {
        failsafe = nextFailsafe;
        rescale();
        render();
      } else {
        renderMarkers();
      }
    },
  };
}
