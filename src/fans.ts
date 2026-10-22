// Fan cards: a ring gauge for the share of maximum speed, with a rotor that spins faster as the fan does.

import { buildGaugeCard, setRing, type GaugeCard } from "./gauge";
import type { Fan, FanCurve, FanReading, HelperFanStatus } from "./types";

const ROTOR =
  '<svg viewBox="-16 -16 32 32" aria-hidden="true">' +
  [0, 90, 180, 270]
    .map((a) => `<path transform="rotate(${a})" d="M0-3C-1-8 1-14 6-14C8-9 6-4 0-3Z"/>`)
    .join("") +
  '<circle r="3.2"/></svg>';

const HIDE_ICON =
  '<svg viewBox="0 0 16 16" aria-hidden="true"><path d="M2 8s2.2-4 6-4 6 4 6 4-2.2 4-6 4-6-4-6-4z"/>' +
  '<circle cx="8" cy="8" r="1.8"/><path d="M3 13L13 3"/></svg>';

export interface FanView extends GaugeCard {
  fan: Fan;
  /** The mode line: what sets the fan's speed now. null when the app cannot set this fan's speed. */
  control: HTMLElement | null;
}

/** What the fan card needs to know about the control of its fan. */
export interface ControlInfo {
  /** The saved curve. One point means a fixed level. */
  curve: FanCurve | undefined;
  /** This fan in the fan helper's last status. */
  status: HelperFanStatus | undefined;
  /** Why curves do not apply now, from the window. */
  notice: string | null;
}

/** One point means one duty at every temperature. */
export function fixedLevel(curve: FanCurve | undefined): number | null {
  return curve?.points.length === 1 ? curve.points[0].duty : null;
}

/** The mode line: what controls the fan now, and whether that works. */
function controlLine(info: ControlInfo): { text: string; hint: string; state: "auto" | "on" | "off" | "failsafe" } {
  const { curve, status, notice } = info;
  if (!curve) {
    return { text: "Automatic", hint: "The firmware sets the speed. To set it yourself, use Fan control in the top bar.", state: "auto" };
  }
  const level = fixedLevel(curve);
  const mode = level != null ? `Fixed ${Math.round(level)}%` : "Curve";
  if (status?.failsafe) {
    return {
      text: "Full speed: failsafe",
      hint: "The sensor reached its throttle point, or stopped reading. The fan runs at full speed.",
      state: "failsafe",
    };
  }
  if (status?.error) return { text: `${mode}: not applied`, hint: status.error, state: "off" };
  if (status?.duty != null) {
    // The duty the helper set, not the saved one: right after Apply they can differ for a moment.
    const duty = Math.round(status.duty);
    const text = level != null ? `Fixed ${duty}%` : `Curve · ${duty}%`;
    return { text, hint: `This app sets the fan to ${duty}% PWM duty.`, state: "on" };
  }
  return {
    text: notice ?? "Not applied",
    hint: notice
      ? `${mode} does not apply: ${notice}.`
      : "The fan helper reports nothing for this fan. Its sensor may be missing.",
    state: "off",
  };
}

export function fmtRpm(rpm: number): string {
  return `${rpm.toLocaleString("en-US")} RPM`;
}

export function fanName(fan: Fan): string {
  return `${fan.device} · ${fan.label}`;
}

/** True when the fan reading shows it turning. */
export function spinning(r: FanReading | null): boolean {
  return (r?.rpm ?? 0) > 0 || (r?.share ?? 0) > 0;
}

/** Share of the fan's maximum speed, and what "maximum" means for this fan. */
function share(fan: Fan, r: FanReading, peak: number | undefined) {
  if (fan.ratedMax && r.rpm != null) {
    return { pct: (r.rpm / fan.ratedMax) * 100, basis: "of rated max", hint: `Rated maximum: ${fmtRpm(fan.ratedMax)}.` };
  }
  if (r.share != null && fan.shareKind === "driver") {
    return { pct: r.share, basis: "of max speed", hint: "The driver reports the fan speed as a share of its maximum." };
  }
  if (r.share != null) {
    return { pct: r.share, basis: "PWM duty", hint: "The share of full power that the controller sends to this fan." };
  }
  if (peak && r.rpm != null) {
    return {
      pct: (r.rpm / peak) * 100,
      basis: "of fastest seen",
      hint: `The hardware reports no maximum, so the fastest speed seen stands in for it: ${fmtRpm(peak)}.`,
    };
  }
  return null;
}

export function buildFanView(fan: Fan, onHide: () => void): FanView {
  const view: FanView = { fan, control: null, ...buildGaugeCard("fan", fanName(fan), ROTOR) };
  if (fan.controllable) {
    const control = document.createElement("div");
    control.className = "fan-control";
    view.detail.after(control);
    view.control = control;
  }
  const hide = document.createElement("button");
  hide.type = "button";
  hide.className = "icon-button gauge-hide";
  hide.title = "Hide this fan";
  hide.setAttribute("aria-label", `Hide ${fanName(fan)}`);
  hide.innerHTML = HIDE_ICON;
  hide.addEventListener("click", onHide);
  view.card.append(hide);
  return view;
}

export function renderFan(view: FanView, reading: FanReading | null, peak: number | undefined, control: ControlInfo): void {
  if (view.control) {
    const line = controlLine(control);
    view.control.textContent = line.text;
    view.control.title = line.hint;
    view.control.dataset.state = line.state;
  }
  const target = reading?.target ? `Target ${fmtRpm(reading.target)}.` : "";
  if (!reading) {
    view.value.textContent = "—";
    view.detail.textContent = "No reading";
    spin(view, 0);
    view.card.title = "The fan did not answer the last reading.";
    return;
  }

  const s = share(view.fan, reading, peak);
  view.value.textContent =
    reading.rpm != null ? fmtRpm(reading.rpm) : s ? `${Math.round(s.pct)}%` : "—";
  if (!spinning(reading)) {
    view.detail.textContent = "Stopped";
    spin(view, 0);
    view.card.title = target;
    return;
  }
  const pct = s ? Math.min(100, Math.max(0, s.pct)) : 50;
  // Without RPM the percentage is already the large value; do not repeat it.
  view.detail.textContent = !s ? "Maximum unknown" : reading.rpm == null ? s.basis : `${Math.round(pct)}% ${s.basis}`;
  spin(view, pct);
  view.card.title = [s?.hint, target].filter(Boolean).join(" ");
}

/** Fills the ring and picks one of five spin speeds. Coarse steps keep the rotor from jumping every second. */
function spin(view: FanView, pct: number): void {
  setRing(view.arc, pct);
  view.card.dataset.speed = String(pct <= 0 ? 0 : Math.min(5, Math.ceil(pct / 20)));
}
