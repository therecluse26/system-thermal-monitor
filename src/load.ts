// System thermal load: how close the most stressed sensor is to its own throttle point.
//
// A plain average of temperatures mixes parts with very different limits, so it says
// nothing about risk. Instead each sensor is placed on a 0–1 scale that reaches 1 at its
// throttle point, and the highest value wins. Android's thermal headroom API works the
// same way, with the same 30 °C span below the threshold.

import { buildGaugeCard, setRing, type GaugeCard } from "./gauge";
import { fmtLimit, fmtTemp, setStatus, type Status } from "./format";

/** Degrees below the throttle point where the load starts to rise from 0%. */
const SPAN = 30;

const THERMOMETER =
  '<svg viewBox="0 0 32 32" aria-hidden="true">' +
  '<path d="M13 7a3 3 0 0 1 6 0v11.6a6 6 0 1 1-6 0z" fill="none" stroke="currentColor" stroke-width="2.2"/>' +
  '<path d="M16 12v10" stroke="currentColor" stroke-width="2.6" stroke-linecap="round"/>' +
  '<circle cx="16" cy="23" r="3.4" fill="currentColor"/></svg>';

const HINT =
  "The sensor closest to its throttle point sets the load. " +
  `0% means every sensor is at least ${SPAN} °C below its throttle point. ` +
  "100% means one sensor has reached it. Hidden sensors do not count.";

export interface LoadInput {
  name: string;
  value: number;
  throttle: number;
}

export interface LoadView extends GaugeCard {
  pill: HTMLElement;
}

export function buildLoadView(): LoadView {
  const gauge = buildGaugeCard("load", "System thermal load", THERMOMETER);
  const pill = document.createElement("span");
  pill.className = "pill";
  const row = document.createElement("div");
  row.className = "gauge-row";
  gauge.value.replaceWith(row);
  row.append(gauge.value, pill);
  gauge.card.title = HINT;
  return { ...gauge, pill };
}

/** `status` is the worst status of any visible sensor, so the card agrees with the chart cards. */
export function renderLoad(view: LoadView, inputs: LoadInput[], status: Status): void {
  let driver: LoadInput | null = null;
  for (const s of inputs) {
    if (!driver || s.throttle - s.value < driver.throttle - driver.value) driver = s;
  }
  view.card.dataset.status = status;
  setStatus(view.pill, status);
  if (!driver) {
    view.value.textContent = "—";
    view.detail.textContent = "No sensor readings";
    setRing(view.arc, 0);
    return;
  }

  const headroom = driver.throttle - driver.value;
  const pct = Math.max(0, ((SPAN - headroom) / SPAN) * 100);
  view.value.textContent = `${Math.round(pct)}%`;
  view.detail.textContent =
    headroom >= 0
      ? `${driver.name} · ${fmtLimit(headroom)} below throttle`
      : `${driver.name} · ${fmtLimit(-headroom)} above throttle`;
  view.detail.title = `${driver.name}: ${fmtTemp(driver.value)}, throttling likely at ${fmtLimit(driver.throttle)}.`;
  setRing(view.arc, pct);
}
