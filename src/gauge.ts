// Ring gauge shared by the top cards: an arc open at the bottom that covers 270° at 100%.

const RADIUS = 26;
const CIRCUMFERENCE = 2 * Math.PI * RADIUS;
const SWEEP = 0.75 * CIRCUMFERENCE;

export interface GaugeCard {
  card: HTMLElement;
  arc: SVGCircleElement;
  name: HTMLElement;
  value: HTMLElement;
  detail: HTMLElement;
}

/** A card with a ring around `centre` (inline SVG markup), a name, a large value and a detail line. */
export function buildGaugeCard(className: string, name: string, centre: string): GaugeCard {
  const card = document.createElement("div");
  card.className = `gauge-card ${className}`;
  card.innerHTML =
    '<div class="gauge">' +
    '<svg viewBox="0 0 64 64" aria-hidden="true">' +
    `<circle class="ring-track" cx="32" cy="32" r="${RADIUS}" transform="rotate(135 32 32)" stroke-dasharray="${SWEEP} ${CIRCUMFERENCE}"/>` +
    `<circle class="ring-arc" cx="32" cy="32" r="${RADIUS}" transform="rotate(135 32 32)" stroke-dasharray="0 ${CIRCUMFERENCE}"/>` +
    "</svg>" +
    `<div class="gauge-centre">${centre}</div>` +
    "</div>" +
    '<div class="gauge-text"><div class="gauge-name"></div><div class="gauge-value"></div><div class="gauge-detail"></div></div>';
  const gauge: GaugeCard = {
    card,
    arc: card.querySelector(".ring-arc") as SVGCircleElement,
    name: card.querySelector(".gauge-name") as HTMLElement,
    value: card.querySelector(".gauge-value") as HTMLElement,
    detail: card.querySelector(".gauge-detail") as HTMLElement,
  };
  gauge.name.textContent = name;
  return gauge;
}

/** Fills the ring to `pct` (0–100; values outside are clamped). */
export function setRing(arc: SVGCircleElement, pct: number): void {
  const clamped = Math.min(100, Math.max(0, pct));
  arc.setAttribute("stroke-dasharray", `${(SWEEP * clamped) / 100} ${CIRCUMFERENCE}`);
  arc.style.opacity = clamped > 0 ? "1" : "0";
}
