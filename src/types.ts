// Mirrors the Rust types that cross the Tauri bridge (serde, camelCase).

export type Category =
  | "cpu"
  | "gpu"
  | "memory"
  | "storage"
  | "board"
  | "wireless"
  | "battery"
  | "acpi"
  | "other";

export interface Thresholds {
  /** Keep the sensor below this. */
  safe: number;
  /** At or above this, throttling is likely. */
  throttle: number;
  source: "hardware" | "estimate";
  basis: string;
}

export interface Sensor {
  id: string;
  chip: string;
  category: Category;
  device: string;
  label: string;
  /** Kernel name, set only when another device has the same name, e.g. "nvme0". */
  instance: string | null;
  detail: string;
  thresholds: Thresholds;
}

export interface Fan {
  id: string;
  chip: string;
  /** What the fan belongs to, e.g. "System" or "GPU". */
  device: string;
  label: string;
  /** Rated maximum in RPM, when the hardware reports one. */
  ratedMax: number | null;
  /** What `FanReading.share` measures: PWM duty, or the driver's own share of maximum. */
  shareKind: "pwm" | "driver" | null;
  /** Whether the app can set this fan's speed. */
  controllable: boolean;
}

export interface FanReading {
  /** null when the hardware reports only a share of maximum speed. */
  rpm: number | null;
  target: number | null;
  /** Share of maximum in percent, when the hardware reports one. */
  share: number | null;
}

export interface Snapshot {
  generation: number;
  intervalMs: number;
  historyLen: number;
  sensors: Sensor[];
  times: number[];
  /** One series per sensor, aligned with `times`. */
  series: (number | null)[][];
  fans: Fan[];
  /** Latest reading per fan, aligned with `fans`. */
  fanReadings: (FanReading | null)[];
}

export interface Sample {
  generation: number;
  t: number;
  /** Aligned with `Snapshot.sensors` of the same generation. */
  values: (number | null)[];
  /** Aligned with `Snapshot.fans` of the same generation. */
  fans: (FanReading | null)[];
}

export interface Override {
  safe?: number | null;
  throttle?: number | null;
}

export interface CurvePoint {
  /** °C. */
  temp: number;
  /** PWM duty in percent. */
  duty: number;
}

export interface FanCurve {
  /** Id of the sensor whose temperature drives the fan. */
  sensor: string;
  points: CurvePoint[];
}

/** What happened to one fan in the last tick of the fan helper or the fan daemon. */
export interface HelperFanStatus {
  fan: string;
  /** The duty set, in percent. null when the fan did not take it. */
  duty: number | null;
  /** Full speed, because the sensor reached its throttle point or stopped reading. */
  failsafe: boolean;
  /** Why the fan did not take the duty. */
  error: string | null;
}

export interface ControlState {
  /** Why the fans with curves do not follow them, in a few words. null while they do. */
  notice: string | null;
  /** True while the fan daemon applies the settings. The window's fan helper does not run then. */
  daemon: boolean;
  status: {
    /** Another fan controller that runs now. */
    blockedBy: string | null;
    fans: HelperFanStatus[];
  };
}

export interface Settings {
  overrides: Record<string, Override>;
  hidden: string[];
  windowSecs: number;
  /** Fastest speed seen per fan id, in RPM. */
  fanPeaks: Record<string, number>;
  /** Keyed by fan id. A fan without a curve stays in automatic mode. */
  fanCurves: Record<string, FanCurve>;
}
