/** Rendering helpers shared by the console views. */

const SVG_NS = "http://www.w3.org/2000/svg";

/**
 * Inline icon set. The paths are drawn on a 24x24 grid with strokes, so they
 * inherit the current text color and stay crisp at any size.
 */
const ICONS: Record<string, string> = {
  tasks:
    '<line x1="8" y1="6" x2="21" y2="6"/><line x1="8" y1="12" x2="21" y2="12"/>' +
    '<line x1="8" y1="18" x2="21" y2="18"/><line x1="3" y1="6" x2="3.01" y2="6"/>' +
    '<line x1="3" y1="12" x2="3.01" y2="12"/><line x1="3" y1="18" x2="3.01" y2="18"/>',
  inbox:
    '<polyline points="22 12 16 12 14 15 10 15 8 12 2 12"/>' +
    '<path d="M5.45 5.11 2 12v6a2 2 0 0 0 2 2h16a2 2 0 0 0 2-2v-6l-3.45-6.89A2 2 0 0 0 16.76 4H7.24a2 2 0 0 0-1.79 1.11z"/>',
  agents:
    '<rect x="4" y="4" width="16" height="16" rx="3"/><rect x="9" y="9" width="6" height="6" rx="1"/>' +
    '<line x1="9" y1="1" x2="9" y2="4"/><line x1="15" y1="1" x2="15" y2="4"/>' +
    '<line x1="9" y1="20" x2="9" y2="23"/><line x1="15" y1="20" x2="15" y2="23"/>' +
    '<line x1="20" y1="9" x2="23" y2="9"/><line x1="20" y1="14" x2="23" y2="14"/>' +
    '<line x1="1" y1="9" x2="4" y2="9"/><line x1="1" y1="14" x2="4" y2="14"/>',
  shield:
    '<path d="M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10z"/>' +
    '<polyline points="9 12 11 14 15 10"/>',
  search:
    '<circle cx="11" cy="11" r="7"/><line x1="21" y1="21" x2="16.65" y2="16.65"/>',
  refresh:
    '<polyline points="23 4 23 10 17 10"/><polyline points="1 20 1 14 7 14"/>' +
    '<path d="M3.51 9a9 9 0 0 1 14.85-3.36L23 10M1 14l4.64 4.36A9 9 0 0 0 20.49 15"/>',
  back: '<line x1="19" y1="12" x2="5" y2="12"/><polyline points="12 19 5 12 12 5"/>',
  chevron:
    '<polyline points="9 18 15 12 9 6"/>',
  check: '<polyline points="20 6 9 17 4 12"/>',
  cross: '<line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/>',
  ban: '<circle cx="12" cy="12" r="9"/><line x1="5.64" y1="5.64" x2="18.36" y2="18.36"/>',
  clock:
    '<circle cx="12" cy="12" r="9"/><polyline points="12 7 12 12 15.5 14"/>',
  alert:
    '<path d="M10.29 3.86 1.82 18a2 2 0 0 0 1.71 3h16.94a2 2 0 0 0 1.71-3L13.71 3.86a2 2 0 0 0-3.42 0z"/>' +
    '<line x1="12" y1="9" x2="12" y2="13"/><line x1="12" y1="17" x2="12.01" y2="17"/>',
  activity:
    '<polyline points="22 12 18 12 15 21 9 3 6 12 2 12"/>',
  user:
    '<path d="M20 21v-2a4 4 0 0 0-4-4H8a4 4 0 0 0-4 4v2"/><circle cx="12" cy="7" r="4"/>',
  agent:
    '<path d="M12 2 4 6v6c0 5 3.4 9.4 8 10 4.6-.6 8-5 8-10V6z"/>' +
    '<circle cx="12" cy="11" r="2.5"/>',
  layers:
    '<polygon points="12 2 2 7 12 12 22 7 12 2"/>' +
    '<polyline points="2 17 12 22 22 17"/><polyline points="2 12 12 17 22 12"/>',
  history:
    '<path d="M3 3v5h5"/><path d="M3.05 13A9 9 0 1 0 6 5.3L3 8"/><path d="M12 7v5l4 2"/>',
  file:
    '<path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"/>' +
    '<polyline points="14 2 14 8 20 8"/><line x1="16" y1="13" x2="8" y2="13"/>' +
    '<line x1="16" y1="17" x2="8" y2="17"/>',
  copy:
    '<rect x="9" y="9" width="12" height="12" rx="2"/>' +
    '<path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"/>' +
    '<line x1="16" y1="13" x2="8" y2="13"/>' +
    '<line x1="16" y1="17" x2="8" y2="17"/>',
  feed: '<path d="M22 2H11l-8 8 8 8h11a2 2 0 0 0 2-2V4a2 2 0 0 0-2-2z"/>' +
    '<polyline points="16 4 20 4 16 8"/><polyline points="8 12 12 12 8 16"/>' +
    '<polyline points="4 16 8 16 4 20"/>',
  rotate: '<polyline points="1 4 1 10 7 10"/>' +
    '<path d="M3.51 15a9 9 0 1 0 2.13-9.36 1 1 0 0 1 1.86 1.86"/>' +
    '<polyline points="23 4 23 10 17 10"/>' +
    '<path d="M20.49 15a9 9 0 1 0 -2.13 9.36 1 1 0 0 1 -1.86-1.86"/>',
  mask: '<path d="M6 12H5c-1.1 0-2 .9-2 2v1c0 1.1.9 2 2 2h1c1.1 0 2-.9 2-2v-1c0-1.1-.9-2-2-2zM19 4H5c-1.1 0-2 .9-2 2v14c0 1.1.9 2 2 2h14c1.1 0 2-.9 2-2V6c0-1.1-.9-2-2-2zm0 2H5L19 18z"/>' +
    '<line x1="10" y1="19" x2="10" y2="22"/><line x1="14" y1="19" x2="14" y2="22"/>',
  zap: '<polygon points="13 2 3 14 12 14 11 22 21 10 12 10 13 2"/>',
  flag: '<path d="M4 15s1-1 4-1 5 2 8 2 4-1 4-1V4s-1 1-4 1-5-2-8-2-4 1-4 1z"/><line x1="4" y1="22" x2="4" y2="15"/>',
  chart: '<line x1="18" y1="20" x2="18" y1="10"/><line x1="12" y1="20" x2="12" y1="4"/><line x1="6" y1="20" x2="6" y1="14"/>',
  link:
    '<path d="M10 13a5 5 0 0 0 7.54.54l3-3a5 5 0 0 0-7.07-7.07l-1.72 1.71"/>' +
    '<path d="M14 11a5 5 0 0 0-7.54-.54l-3 3a5 5 0 0 0 7.07 7.07l1.71-1.71"/>',
  inboxEmpty:
    '<path d="M22 12h-6l-2 3h-4l-2-3H2"/><path d="M5.45 5.11 2 12v6a2 2 0 0 0 2 2h16a2 2 0 0 0 2-2v-6l-3.45-6.89A2 2 0 0 0 16.76 4H7.24a2 2 0 0 0-1.79 1.11z"/>',
  menu: '<line x1="3" y1="6" x2="21" y2="6"/><line x1="3" y1="12" x2="21" y2="12"/><line x1="3" y1="18" x2="21" y2="18"/>',
};

/** Builds an inline SVG icon that inherits the current text color. */
export function icon(name: string, className = "icon"): SVGElement {
  const svg = document.createElementNS(SVG_NS, "svg");
  svg.setAttribute("viewBox", "0 0 24 24");
  svg.setAttribute("fill", "none");
  svg.setAttribute("stroke", "currentColor");
  svg.setAttribute("stroke-width", "1.7");
  svg.setAttribute("stroke-linecap", "round");
  svg.setAttribute("stroke-linejoin", "round");
  svg.setAttribute("aria-hidden", "true");
  svg.setAttribute("focusable", "false");
  svg.setAttribute("class", className);
  svg.innerHTML = ICONS[name] ?? "";
  return svg;
}

export function el<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  attributes: Record<string, string> = {},
  children: (Node | string)[] = [],
): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag);
  for (const [name, value] of Object.entries(attributes)) {
    if (name === "class") {
      node.className = value;
    } else if (name.startsWith("data-")) {
      node.dataset[name.slice(5)] = value;
    } else {
      node.setAttribute(name, value);
    }
  }
  for (const child of children) {
    node.append(typeof child === "string" ? document.createTextNode(child) : child);
  }
  return node;
}

/** A status pill. Unknown states fall back to the neutral style. */
export function badge(status: string): HTMLElement {
  const normalized = status.trim().toLowerCase();
  return el("span", { class: `badge ${normalized}` }, [normalized]);
}

/** Formats a timestamp for display, keeping the raw value when unparsable. */
export function formatTime(value: string): string {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) {
    return value;
  }
  return date.toLocaleString(undefined, {
    year: "numeric",
    month: "short",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
  });
}

/** Formats a timestamp as a short relative label ("3m ago"). */
export function relativeTime(value: string): string {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) {
    return value;
  }
  const seconds = Math.round((Date.now() - date.getTime()) / 1000);
  const future = seconds < 0;
  const absolute = Math.abs(seconds);
  const label = (text: string) => (future ? `in ${text}` : `${text} ago`);

  if (absolute < 45) {
    return future ? "in a moment" : "just now";
  }
  if (absolute < 3600) {
    return label(`${Math.round(absolute / 60)}m`);
  }
  if (absolute < 86400) {
    return label(`${Math.round(absolute / 3600)}h`);
  }
  if (absolute < 604800) {
    return label(`${Math.round(absolute / 86400)}d`);
  }
  return formatTime(value);
}

export function shortId(value: string, length = 8): string {
  return value.length <= length ? value : `${value.slice(0, length)}…`;
}

/** Extracts the readable text of a stored message. */
export function messageText(partsJson: string): string {
  try {
    const parts = JSON.parse(partsJson) as {
      type: string;
      text?: string;
      data?: unknown;
    }[];
    return parts
      .map((part) => {
        if (part.type === "text") {
          return part.text ?? "";
        }
        if (part.type === "data") {
          return JSON.stringify(part.data);
        }
        return `[${part.type} part]`;
      })
      .join("\n");
  } catch {
    return partsJson;
  }
}

/** A KPI tile used on top of the list views. */
export function statTile(options: {
  label: string;
  value: number | string;
  icon: string;
  tone?: "accent" | "ok" | "warn" | "err" | "violet" | "neutral";
}): HTMLElement {
  const tone = options.tone && options.tone !== "neutral" ? ` stat--${options.tone}` : "";
  return el("div", { class: `stat${tone}` }, [
    el("span", { class: "stat__icon" }, [icon(options.icon)]),
    el("div", { class: "stat__body" }, [
      el("div", { class: "stat__label" }, [options.label]),
      el("div", { class: "stat__value" }, [String(options.value)]),
    ]),
  ]);
}

/** The empty state shown when a collection has no rows. */
export function emptyState(options: {
  icon: string;
  title: string;
  text?: string;
  action?: HTMLElement;
}): HTMLElement {
  const children: (Node | string)[] = [
    el("span", { class: "empty__icon" }, [icon(options.icon)]),
    el("p", { class: "empty__title" }, [options.title]),
  ];
  if (options.text) {
    children.push(el("p", { class: "empty__text" }, [options.text]));
  }
  if (options.action) {
    children.push(options.action);
  }
  return el("div", { class: "empty" }, children);
}

/** The error state shown when the administration API cannot be reached. */
export function errorState(error: unknown, retry?: () => void): HTMLElement {
  const message = error instanceof Error ? error.message : String(error);
  const body = el("div", { class: "alert__body" }, [
    el("p", { class: "alert__title" }, ["Could not reach the warden API"]),
    el("p", { class: "alert__text" }, [message]),
  ]);
  const children: (Node | string)[] = [icon("alert"), body];
  if (retry) {
    const button = el("button", { class: "btn btn--sm", type: "button" }, [
      icon("refresh"),
      "Retry",
    ]);
    button.addEventListener("click", () => {
      retry();
    });
    body.append(button);
  }
  return el("div", { class: "alert" }, children);
}

/** A shimmering placeholder used while a view loads. */
export function skeletonRows(rows = 5, columns = 4): HTMLElement {
  const wrap = el("div", { class: "card" });
  for (let row = 0; row < rows; row += 1) {
    const line = el("div", { class: "skeleton-row" });
    for (let column = 0; column < columns; column += 1) {
      const bar = el("div", { class: "skeleton" });
      bar.style.width = `${[18, 14, 22, 16][column % 4] ?? 16}%`;
      line.append(bar);
    }
    wrap.append(line);
  }
  return wrap;
}

/** A monospace identifier with a tooltip holding the full value. */
export function idCell(value: string, length = 10): HTMLElement {
  const code = el("code", { class: "cell-id", title: value }, [shortId(value, length)]);
  return code;
}

/**
 * Shows a transient notification in the corner of the screen. The toast fades
 * out on its own, so callers never have to clean it up.
 */
export function toast(tone: "ok" | "err", message: string): void {
  const stack = document.getElementById("toast-stack");
  if (!stack) {
    return;
  }
  const node = el("div", { class: `toast toast--${tone}`, role: "status" }, [
    icon(tone === "ok" ? "check" : "alert"),
    el("span", {}, [message]),
  ]);
  stack.append(node);
  window.setTimeout(() => {
    node.style.opacity = "0";
    node.style.transition = "opacity .2s ease";
    window.setTimeout(() => node.remove(), 220);
  }, 3200);
}
