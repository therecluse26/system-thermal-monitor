// Fan control panel: Automatic, a fixed level, or a curve. The fan control dialog shows one
// panel for all fans, or one panel per fan.

import { buildCurveEditor, DEFAULT_CURVE } from "./curve-editor";
import { fixedLevel } from "./fans";
import { fmtLimit } from "./format";
import type { FanCurve, Sensor } from "./types";

type Mode = "auto" | "fixed" | "curve";

/** What a panel needs from the window about the sensors. */
export interface SensorInfo {
  sensors: Sensor[];
  name(sensor: Sensor): string;
  /** The "Throttling likely at" temperature, the custom one when the user set it. */
  throttle(sensor: Sensor): number;
  /** The latest reading, or null when there is none. */
  now(sensor: Sensor): number | null;
}

export interface PanelOptions {
  /** The fan's name. A panel for all fans has none. */
  title?: string;
  /** Shown when the pointer is on the title. */
  detail?: string;
  /** Who runs at full speed in the failsafe text: "the fan" or "every fan". */
  subject: string;
}

export interface ControlPanel {
  root: HTMLElement;
  /** The setting in the panel. null means Automatic. A fixed level is a curve with one point. */
  value(): FanCurve | null;
  /** Shows a saved setting. undefined or null means Automatic. */
  load(curve: FanCurve | null | undefined): void;
  /** Shows the parts of the mode, and moves the curve editor's markers to the readings now. */
  update(): void;
}

/** Radio groups of different panels must not share a name. */
let panelCount = 0;

function el<K extends keyof HTMLElementTagNameMap>(tag: K, className = "", text = ""): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text) node.textContent = text;
  return node;
}

function modeOption(name: string, value: Mode, text: string, hint: string): HTMLLabelElement {
  const label = el("label");
  const input = el("input");
  input.type = "radio";
  input.name = name;
  input.value = value;
  label.append(input, ` ${text} `);
  if (hint) label.append(el("span", "muted", hint));
  return label;
}

export function buildControlPanel(info: SensorInfo, options: PanelOptions): ControlPanel {
  const root = el("section", options.title ? "fan-panel" : "fan-panel shared");
  if (options.title) {
    const title = el("h3", "", options.title);
    if (options.detail) title.title = options.detail;
    root.append(title);
  }

  const modeName = `mode-${++panelCount}`;
  const modes = el("fieldset", "modes");
  modes.append(
    el("legend", "", "Speed"),
    modeOption(modeName, "auto", "Automatic", "· the firmware sets it"),
    modeOption(modeName, "fixed", "Fixed level", ""),
    modeOption(modeName, "curve", "Curve", "· the duty follows a sensor"),
  );
  const radios = [...modes.querySelectorAll("input")];

  const level = el("label", "level");
  const levelInput = el("input");
  levelInput.type = "range";
  levelInput.min = "0";
  levelInput.max = "100";
  levelInput.step = "1";
  levelInput.setAttribute("aria-label", "Fixed level in percent");
  const levelValue = el("output");
  level.append(levelInput, levelValue);

  const editor = buildCurveEditor();
  const zero = el(
    "p",
    "warn-text",
    "0% stops the fan when the hardware allows it. Never use it for a pump. The failsafe still applies.",
  );

  const failsafe = el("div", "fields");
  const sensorField = el("label");
  const sensorLabel = el("span");
  const sensorSelect = el("select");
  sensorSelect.replaceChildren(
    ...info.sensors.map((s) => {
      const option = el("option", "", info.name(s));
      option.value = s.id;
      return option;
    }),
  );
  sensorField.append(sensorLabel, sensorSelect);
  const failsafeText = el("p", "muted");
  failsafe.append(sensorField, failsafeText);

  root.append(modes, level, editor.root, zero, failsafe);

  const mode = (): Mode => (radios.find((r) => r.checked)?.value as Mode | undefined) ?? "auto";

  return {
    root,
    value() {
      const m = mode();
      if (m === "auto") return null;
      const points = m === "fixed" ? [{ temp: 0, duty: Number(levelInput.value) }] : editor.points();
      return { sensor: sensorSelect.value, points };
    },
    load(curve) {
      const fixed = fixedLevel(curve ?? undefined);
      const m: Mode = !curve ? "auto" : fixed == null ? "curve" : "fixed";
      for (const r of radios) r.checked = r.value === m;
      levelInput.value = String(fixed ?? 50);
      editor.setPoints(curve && fixed == null ? curve.points : DEFAULT_CURVE);
      sensorSelect.value = curve?.sensor ?? "";
      if (!sensorSelect.value) {
        sensorSelect.value = (info.sensors.find((s) => s.category === "cpu") ?? info.sensors[0])?.id ?? "";
      }
    },
    update() {
      const m = mode();
      level.hidden = m !== "fixed";
      levelValue.textContent = `${levelInput.value}%`;
      editor.root.hidden = m !== "curve";
      zero.hidden =
        m === "fixed" ? Number(levelInput.value) !== 0 : m !== "curve" || !editor.points().some((p) => p.duty === 0);
      failsafe.hidden = m === "auto";
      sensorLabel.textContent = m === "curve" ? "The curve follows this sensor" : "Full speed when this sensor gets hot";
      const sensor = info.sensors.find((s) => s.id === sensorSelect.value);
      const throttle = sensor ? info.throttle(sensor) : null;
      editor.setMarkers(sensor ? info.now(sensor) : null, throttle);
      failsafeText.textContent =
        throttle == null
          ? ""
          : `At ${fmtLimit(throttle)} ("Throttling likely at"), or when the sensor stops reading, ` +
            `${options.subject} runs at full speed.` +
            (m === "curve" ? " The duty falls only after the temperature drops 3 °C." : "");
    },
  };
}
