// Canvas chart in the style of KDE System Monitor, plus threshold lines.
//
// One series draws as a filled area. Up to MAX_LINES series draw as coloured lines.
// More than that draw as a band from the coolest to the hottest series, because that
// many colours cannot stay distinct.

import { fmtLimit } from "./format";

/** Line colours for charts with several series, on the dark card surface.
 *  Validated as a set (dataviz palette check, dark mode, surface #1b1e20): adjacent
 *  colour-blind ΔE ≥ 9.4, normal-vision ΔE ≥ 19.7, contrast ≥ 3:1. Yellow and red are
 *  left out because the threshold lines use them. */
export const PALETTE = ["#3987e5", "#d95926", "#199e70", "#9085e9", "#d55181", "#008300"];
export const MAX_LINES = PALETTE.length;

export interface ChartTheme {
  surface: string;
  grid: string;
  text: string;
  muted: string;
  line: string;
  fill: string;
  warn: string;
  crit: string;
  warnZone: string;
  critZone: string;
  font: string;
}

export function readTheme(): ChartTheme {
  const css = getComputedStyle(document.documentElement);
  const v = (name: string) => css.getPropertyValue(name).trim();
  return {
    surface: v("--surface"),
    grid: v("--grid"),
    text: v("--text-2"),
    muted: v("--text-3"),
    line: v("--accent"),
    fill: v("--accent-fill"),
    warn: v("--warn"),
    crit: v("--crit"),
    warnZone: v("--warn-zone"),
    critZone: v("--crit-zone"),
    font: v("--font"),
  };
}

export type ChartMode = "area" | "lines" | "band";

export function chartMode(seriesCount: number): ChartMode {
  return seriesCount <= 1 ? "area" : seriesCount <= MAX_LINES ? "lines" : "band";
}

/** Colour of series `k` in a chart of `count` series. */
export function seriesColor(k: number, count: number, theme: ChartTheme): string {
  return chartMode(count) === "lines" ? PALETTE[k] : theme.line;
}

export interface ChartInput {
  /** Unix ms, ascending. */
  times: readonly number[];
  /** °C, each aligned with `times`; null is a gap. */
  series: readonly (readonly (number | null)[])[];
  /** Visible time range, Unix ms. */
  start: number;
  end: number;
  safe: number;
  throttle: number;
  /** Pointer x in CSS pixels, or null when the pointer is elsewhere. */
  hoverX: number | null;
  /** Width of the window as text, for the left x-axis label ("5 min"). */
  spanLabel: string;
}

/** The sample under the crosshair: its index in `times`, and where to anchor a tooltip. */
export interface HoverPoint {
  index: number;
  x: number;
  y: number;
}

interface Scale {
  lo: number;
  hi: number;
  ticks: number[];
}

const STEPS = [5, 10, 20, 25, 50];
const MAX_INTERVALS = 5;

/** Fits the data and the throttle line, so the headroom to throttling is always in view. */
export function yScale(min: number | null, max: number | null, throttle: number): Scale {
  const top = Math.max(max ?? throttle, throttle) + 3;
  const bottom = Math.max(0, (min ?? throttle - 40) - 5);
  let step = STEPS[STEPS.length - 1];
  for (const s of STEPS) {
    if (Math.ceil(top / s) - Math.floor(bottom / s) <= MAX_INTERVALS) {
      step = s;
      break;
    }
  }
  const lo = Math.floor(bottom / step) * step;
  const hi = Math.ceil(top / step) * step;
  const ticks: number[] = [];
  for (let t = lo; t <= hi; t += step) ticks.push(t);
  return { lo, hi, ticks };
}

/** Index of the first time at or after `t`; `times.length` when there is none. */
function firstAtOrAfter(times: readonly number[], t: number): number {
  let lo = 0;
  let hi = times.length;
  while (lo < hi) {
    const mid = (lo + hi) >> 1;
    if (times[mid] < t) lo = mid + 1;
    else hi = mid;
  }
  return lo;
}

const clamp = (v: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, v));

/** Per-sample minimum and maximum across all series; null where every series is null. */
function envelope(series: ChartInput["series"], from: number, to: number) {
  const lo: (number | null)[] = [];
  const hi: (number | null)[] = [];
  for (let i = from; i < to; i++) {
    let min: number | null = null;
    let max: number | null = null;
    for (const s of series) {
      const v = s[i];
      if (v == null) continue;
      if (min == null || v < min) min = v;
      if (max == null || v > max) max = v;
    }
    lo.push(min);
    hi.push(max);
  }
  return { lo, hi };
}

export function drawChart(canvas: HTMLCanvasElement, d: ChartInput, theme: ChartTheme): HoverPoint | null {
  const w = canvas.clientWidth;
  const h = canvas.clientHeight;
  const ctx = canvas.getContext("2d");
  if (!w || !h || !ctx) return null;

  const dpr = window.devicePixelRatio || 1;
  const pxW = Math.round(w * dpr);
  const pxH = Math.round(h * dpr);
  if (canvas.width !== pxW || canvas.height !== pxH) {
    canvas.width = pxW;
    canvas.height = pxH;
  }
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  ctx.clearRect(0, 0, w, h);

  const mode = chartMode(d.series.length);
  // One point before the window keeps the lines running into the left edge.
  const from = Math.max(0, firstAtOrAfter(d.times, d.start) - 1);
  const to = d.times.length;
  const env = envelope(d.series, from, to);
  let min: number | null = null;
  let max: number | null = null;
  for (let k = 0; k < env.lo.length; k++) {
    const lo = env.lo[k];
    const hi = env.hi[k];
    if (lo != null && (min == null || lo < min)) min = lo;
    if (hi != null && (max == null || hi > max)) max = hi;
  }
  const scale = yScale(min, max, d.throttle);

  ctx.font = `11px ${theme.font}`;
  const tickLabels = scale.ticks.map((t) => `${t} °C`);
  const labelWidth = Math.max(...tickLabels.map((l) => ctx.measureText(l).width));
  const left = Math.ceil(labelWidth) + 10;
  const right = w - 2;
  const top = 8;
  const bottom = h - 20;
  const plotW = right - left;
  const plotH = bottom - top;
  const X = (t: number) => left + ((t - d.start) / (d.end - d.start)) * plotW;
  const Y = (c: number) => bottom - ((c - scale.lo) / (scale.hi - scale.lo)) * plotH;

  // Faint bands: amber between "keep below" and "throttle", red above "throttle".
  const yThrottle = clamp(Y(d.throttle), top, bottom);
  const ySafe = clamp(Y(d.safe), top, bottom);
  ctx.fillStyle = theme.critZone;
  ctx.fillRect(left, top, plotW, yThrottle - top);
  ctx.fillStyle = theme.warnZone;
  ctx.fillRect(left, yThrottle, plotW, ySafe - yThrottle);

  ctx.lineWidth = 1;
  ctx.strokeStyle = theme.grid;
  ctx.fillStyle = theme.text;
  ctx.textAlign = "right";
  ctx.textBaseline = "middle";
  scale.ticks.forEach((tick, i) => {
    const y = Math.round(Y(tick)) + 0.5;
    ctx.beginPath();
    ctx.moveTo(left, y);
    ctx.lineTo(right, y);
    ctx.stroke();
    ctx.fillText(tickLabels[i], left - 8, y);
  });

  ctx.save();
  ctx.beginPath();
  ctx.rect(left, top, plotW, plotH);
  ctx.clip();
  ctx.lineJoin = "round";
  ctx.lineCap = "round";

  /** Calls `draw` for each run of consecutive non-null values. */
  const eachRun = (values: readonly (number | null)[], offset: number, draw: (run: [number, number][], first: number) => void) => {
    let run: [number, number][] = [];
    let first = 0;
    for (let k = 0; k <= values.length; k++) {
      const v = k < values.length ? values[k] : null;
      if (v == null) {
        if (run.length) draw(run, first);
        run = [];
      } else {
        if (!run.length) first = k;
        run.push([X(d.times[offset + k]), Y(v)]);
      }
    }
  };
  const trace = (run: [number, number][]) => {
    ctx.beginPath();
    run.forEach(([x, y], k) => (k ? ctx.lineTo(x, y) : ctx.moveTo(x, y)));
  };
  const stroke = (run: [number, number][], color: string, width: number) => {
    if (run.length === 1) {
      ctx.fillStyle = color;
      ctx.beginPath();
      ctx.arc(run[0][0], run[0][1], width * 0.75, 0, Math.PI * 2);
      ctx.fill();
      return;
    }
    trace(run);
    ctx.strokeStyle = color;
    ctx.lineWidth = width;
    ctx.stroke();
  };

  if (mode === "area") {
    eachRun(d.series[0]?.slice(from) ?? [], from, (run) => {
      if (run.length > 1) {
        trace(run);
        ctx.lineTo(run[run.length - 1][0], bottom);
        ctx.lineTo(run[0][0], bottom);
        ctx.closePath();
        ctx.fillStyle = theme.fill;
        ctx.fill();
      }
      stroke(run, theme.line, 2);
    });
  } else if (mode === "lines") {
    d.series.forEach((s, k) => eachRun(s.slice(from), from, (run) => stroke(run, PALETTE[k], 2)));
  } else {
    eachRun(env.hi, from, (upper, first) => {
      const lower = env.lo.slice(first, first + upper.length).map((v, k) => [upper[k][0], Y(v!)] as [number, number]);
      if (upper.length > 1) {
        trace(upper);
        for (let k = lower.length - 1; k >= 0; k--) ctx.lineTo(lower[k][0], lower[k][1]);
        ctx.closePath();
        ctx.fillStyle = theme.fill;
        ctx.fill();
      }
      ctx.globalAlpha = 0.55;
      stroke(lower, theme.line, 1.25);
      ctx.globalAlpha = 1;
      stroke(upper, theme.line, 2);
    });
  }
  ctx.restore();

  // Threshold lines sit on top of the data so they stay visible under a hot trace.
  const limitLines: { y: number; color: string; text: string }[] = [
    { y: Y(d.throttle), color: theme.crit, text: `throttle ${fmtLimit(d.throttle)}` },
    { y: Y(d.safe), color: theme.warn, text: `keep below ${fmtLimit(d.safe)}` },
  ];
  // Labels go on the left: the newest, often hottest, data runs along the right edge.
  ctx.textAlign = "left";
  ctx.font = `10.5px ${theme.font}`;
  let previousLabelY = -Infinity;
  for (const line of limitLines) {
    if (line.y < top - 0.5 || line.y > bottom + 0.5) continue;
    const y = Math.round(line.y) + 0.5;
    ctx.setLineDash([6, 4]);
    ctx.strokeStyle = line.color;
    ctx.lineWidth = 1.5;
    ctx.beginPath();
    ctx.moveTo(left, y);
    ctx.lineTo(right, y);
    ctx.stroke();
    ctx.setLineDash([]);

    // Label above the line, unless it would leave the plot or hit the label above it.
    const above = y - 13 >= top && y - 13 >= previousLabelY + 2;
    ctx.textBaseline = above ? "bottom" : "top";
    const labelY = above ? y - 3 : y + 3;
    ctx.lineWidth = 3;
    ctx.strokeStyle = theme.surface;
    ctx.strokeText(line.text, left + 6, labelY);
    ctx.fillStyle = theme.text;
    ctx.fillText(line.text, left + 6, labelY);
    previousLabelY = above ? y - 3 : y + 14;
  }

  ctx.font = `11px ${theme.font}`;
  ctx.fillStyle = theme.muted;
  ctx.textBaseline = "top";
  ctx.textAlign = "left";
  ctx.fillText(`−${d.spanLabel}`, left, bottom + 5);
  ctx.textAlign = "right";
  ctx.fillText("now", right, bottom + 5);

  return drawHover(ctx, d, mode, { left, right, top, bottom }, X, Y, theme, from);
}

function drawHover(
  ctx: CanvasRenderingContext2D,
  d: ChartInput,
  mode: ChartMode,
  plot: { left: number; right: number; top: number; bottom: number },
  X: (t: number) => number,
  Y: (c: number) => number,
  theme: ChartTheme,
  from: number,
): HoverPoint | null {
  if (d.hoverX == null || d.hoverX < plot.left || d.hoverX > plot.right || from >= d.times.length) {
    return null;
  }
  const t = d.start + ((d.hoverX - plot.left) / (plot.right - plot.left)) * (d.end - d.start);
  let i = Math.min(firstAtOrAfter(d.times, t), d.times.length - 1);
  if (i > 0 && Math.abs(d.times[i - 1] - t) < Math.abs(d.times[i] - t)) i--;
  if (d.times[i] < d.start) return null;

  const x = X(d.times[i]);
  ctx.strokeStyle = theme.muted;
  ctx.lineWidth = 1;
  ctx.beginPath();
  ctx.moveTo(Math.round(x) + 0.5, plot.top);
  ctx.lineTo(Math.round(x) + 0.5, plot.bottom);
  ctx.stroke();

  // Dots: every series in line mode; the hottest and coolest in band mode.
  const values = d.series.map((s) => s[i]).filter((v): v is number => v != null);
  const dots: [number, string][] =
    mode === "lines"
      ? d.series.flatMap((s, k) => (s[i] == null ? [] : [[s[i]!, PALETTE[k]] as [number, string]]))
      : mode === "band" && values.length
        ? [
            [Math.max(...values), theme.line],
            [Math.min(...values), theme.line],
          ]
        : values.map((v) => [v, theme.line] as [number, string]);
  for (const [v, color] of dots) {
    const y = Y(v);
    ctx.fillStyle = theme.surface;
    ctx.beginPath();
    ctx.arc(x, y, 5.5, 0, Math.PI * 2);
    ctx.fill();
    ctx.fillStyle = color;
    ctx.beginPath();
    ctx.arc(x, y, 3.5, 0, Math.PI * 2);
    ctx.fill();
  }
  const y = values.length ? Y(Math.max(...values)) : plot.top;
  return { index: i, x, y };
}
