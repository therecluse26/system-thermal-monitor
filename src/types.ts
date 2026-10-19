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

export interface Settings {
  overrides: Record<string, Override>;
  hidden: string[];
  windowSecs: number;
  /** Fastest speed seen per fan id, in RPM. */
  fanPeaks: Record<string, number>;
}
