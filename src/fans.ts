// Fan cards: a ring gauge for the share of maximum speed, with a rotor that spins faster as the fan does.

import { buildGaugeCard, setRing, type GaugeCard } from "./gauge";
import type { Fan, FanReading } from "./types";

const ROTOR =
  '<svg viewBox="-16 -16 32 32" aria-hidden="true">' +
  [0, 90, 180, 270]
    .map((a) => `<path transform="rotate(${a})" d="M0-3C-1-8 1-14 6-14C8-9 6-4 0-3Z"/>`)
    .join("") +
  '<circle r="3.2"/></svg>';

export interface FanView extends GaugeCard {
  fan: Fan;
}

export function fmtRpm(rpm: number): string {
  return `${rpm.toLocaleString("en-US")} RPM`;
}

/** Share of the fan's maximum speed, and what "maximum" means for this fan. */
function share(fan: Fan, r: FanReading, peak: number | undefined) {
  if (fan.ratedMax) {
    return { pct: (r.rpm / fan.ratedMax) * 100, basis: "of rated max", hint: `Rated maximum: ${fmtRpm(fan.ratedMax)}.` };
  }
  if (r.duty != null) {
    return { pct: r.duty, basis: "PWM duty", hint: "The share of full power that the controller sends to this fan." };
  }
  if (peak) {
    return {
      pct: (r.rpm / peak) * 100,
      basis: "of fastest seen",
      hint: `The hardware reports no maximum, so the fastest speed seen stands in for it: ${fmtRpm(peak)}.`,
    };
  }
  return null;
}

export function buildFanView(fan: Fan): FanView {
  return { fan, ...buildGaugeCard("fan", `${fan.device} · ${fan.label}`, ROTOR) };
}

export function renderFan(view: FanView, reading: FanReading | null, peak: number | undefined): void {
  const target = reading?.target ? `Target ${fmtRpm(reading.target)}.` : "";
  if (!reading) {
    view.value.textContent = "—";
    view.detail.textContent = "No reading";
    spin(view, 0);
    view.card.title = "The fan did not answer the last reading.";
    return;
  }

  view.value.textContent = fmtRpm(reading.rpm);
  if (reading.rpm === 0) {
    view.detail.textContent = "Stopped";
    spin(view, 0);
    view.card.title = target;
    return;
  }
  const s = share(view.fan, reading, peak);
  const pct = s ? Math.min(100, Math.max(0, s.pct)) : 50;
  view.detail.textContent = s ? `${Math.round(pct)}% ${s.basis}` : "Maximum unknown";
  spin(view, pct);
  view.card.title = [s?.hint, target].filter(Boolean).join(" ");
}

/** Fills the ring and picks one of five spin speeds. Coarse steps keep the rotor from jumping every second. */
function spin(view: FanView, pct: number): void {
  setRing(view.arc, pct);
  view.card.dataset.speed = String(pct <= 0 ? 0 : Math.min(5, Math.ceil(pct / 20)));
}
