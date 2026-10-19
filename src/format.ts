export type Status = "ok" | "warm" | "hot" | "offline";

export const STATUS: Record<Status, { label: string; hint: string; icon: string }> = {
  ok: {
    label: "Normal",
    hint: "Below the recommended maximum",
    icon: '<circle cx="8" cy="8" r="6.25"/><path d="M5.2 8.3l1.9 1.9 3.8-4"/>',
  },
  warm: {
    label: "Warm",
    hint: "Above the recommended maximum, below the throttle point",
    icon: '<path d="M8 1.9l6.4 11.4H1.6z"/><path d="M8 6.4v3.1M8 11.4v.1"/>',
  },
  hot: {
    label: "Throttling",
    hint: "At or above the temperature where throttling is likely",
    icon: '<path d="M5.4 1.6h5.2l3.8 3.8v5.2l-3.8 3.8H5.4l-3.8-3.8V5.4z"/><path d="M8 4.8v3.8M8 11v.1"/>',
  },
  offline: {
    label: "No data",
    hint: "The sensor did not answer the last reading",
    icon: '<circle cx="8" cy="8" r="6.25"/><path d="M5.2 8h5.6"/>',
  },
};

export function statusIcon(status: Status): string {
  return `<svg viewBox="0 0 16 16" aria-hidden="true">${STATUS[status].icon}</svg>`;
}

export function setStatus(node: HTMLElement, status: Status): void {
  if (node.dataset.status === status) return;
  node.dataset.status = status;
  node.title = STATUS[status].hint;
  node.innerHTML = `${statusIcon(status)}<span>${STATUS[status].label}</span>`;
}

export function statusOf(value: number | null, safe: number, throttle: number): Status {
  if (value == null) return "offline";
  if (value >= throttle) return "hot";
  if (value >= safe) return "warm";
  return "ok";
}

/** Live readings: always one decimal, so the width does not jump. */
export function fmtTemp(celsius: number | null | undefined): string {
  return celsius == null ? "—" : `${celsius.toFixed(1)} °C`;
}

/** Limits: one decimal only when it carries information ("85 °C", "82.9 °C"). */
export function fmtLimit(celsius: number): string {
  const rounded = Math.round(celsius * 10) / 10;
  return `${Number.isInteger(rounded) ? rounded.toFixed(0) : rounded.toFixed(1)} °C`;
}

export function fmtClock(t: number): string {
  return new Date(t).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", second: "2-digit" });
}

export function fmtAgo(ms: number): string {
  const secs = Math.max(0, Math.round(ms / 1000));
  if (secs === 0) return "now";
  if (secs < 60) return `${secs} s ago`;
  const mins = Math.floor(secs / 60);
  const rest = secs % 60;
  return rest ? `${mins} min ${rest} s ago` : `${mins} min ago`;
}
