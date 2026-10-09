// The IDE's own features: live values next to the code after a run, the idb
// inspector and the module map.

import { monaco, models } from "./editor.js";
import { settings, onSettingsChange } from "./theme.js";
import { isDemo } from "./backend.js";

const $ = (sel) => document.querySelector(sel);
const NS = "http://www.w3.org/2000/svg";
const svg = (tag, attrs = {}, ...children) => {
  const e = document.createElementNS(NS, tag);
  for (const [k, v] of Object.entries(attrs)) e.setAttribute(k, v);
  e.append(...children);
  return e;
};
const baseName = (p) => p.split(/[\\/]/).pop();

let ctx = null;
/** The latest trace from the running program: { files, idb }. */
let trace = null;
let previousIdb = new Map();

export function initFeatures(context) {
  ctx = context;
  // For poking at the UI in a plain browser.
  if (isDemo) window.belsk2Debug = { models, trace: () => trace, settings };
  onSettingsChange(() => applyLiveValues());
  $("#modmap-close").onclick = () => toggleModuleMap(false);
  $("#modmap-fit").onclick = () => fitMap();
  renderIdb();
}

// ---- live values ---------------------------------------------------------------

export function setTrace(t) {
  previousIdb = new Map((trace?.idb || []).map(([k, v]) => [k, v]));
  trace = t;
  applyLiveValues();
  renderIdb();
}

export function clearTrace() {
  trace = null;
  previousIdb = new Map();
  applyLiveValues();
  renderIdb();
}

/** Shows (or removes) the values of the last run in every open file. */
export function applyLiveValues() {
  for (const [path, model] of models) {
    const lines = settings.liveValues && !model.belsk2LiveStale ? trace?.files?.[path] : null;
    const decorations = [];
    for (const [line, entry] of Object.entries(lines || {})) {
      const n = Number(line);
      if (n < 1 || n > model.getLineCount()) continue;
      const parts = entry.values.map(([name, value]) => `${name} = ${value}`);
      if (entry.hits > 1) parts.push(`×${entry.hits}`);
      if (!parts.length) continue;
      decorations.push({
        range: new monaco.Range(n, 1, n, model.getLineMaxColumn(n)),
        options: {
          after: { content: `   ${parts.join("   ")}`, inlineClassName: "live-value", cursorStops: monaco.editor.InjectedTextCursorStops.None },
          showIfCollapsed: true,
          hoverMessage: { value: `**Last run** — ran ${entry.hits} time${entry.hits === 1 ? "" : "s"}` },
        },
      });
    }
    model.belsk2LiveIds = model.deltaDecorations(model.belsk2LiveIds || [], decorations);
  }
  $("#st-live").textContent = trace && settings.liveValues ? "◉ live values" : "";
}

/** Called when a file is edited: its values no longer match the code. */
export function markStale(model) {
  if (model.belsk2LiveIds?.length) {
    model.belsk2LiveStale = true;
    model.belsk2LiveIds = model.deltaDecorations(model.belsk2LiveIds, []);
  }
}

export function onRunStart() {
  for (const m of models.values()) m.belsk2LiveStale = false;
  clearTrace();
}

// ---- idb inspector -----------------------------------------------------------------

function renderIdb() {
  const list = $("#idb-list");
  const slots = trace?.idb || [];
  if (!slots.length) {
    list.replaceChildren(
      Object.assign(document.createElement("div"), {
        className: "empty",
        textContent: trace
          ? "This run did not store anything in idb."
          : "Run a program to see its idb slots here. They update live while it waits for input.",
      })
    );
    return;
  }
  const table = document.createElement("table");
  table.className = "idb-table";
  table.innerHTML = "<thead><tr><th>slot</th><th>value</th></tr></thead>";
  const body = document.createElement("tbody");
  for (const [slot, value] of slots) {
    const row = document.createElement("tr");
    const changed = previousIdb.size && previousIdb.get(slot) !== value;
    if (changed) row.className = "changed";
    const a = document.createElement("td");
    a.textContent = slot;
    const b = document.createElement("td");
    b.textContent = value;
    b.title = value;
    row.append(a, b);
    body.append(row);
  }
  table.append(body);
  list.replaceChildren(table);
}

// ---- module map ------------------------------------------------------------------------

const NODE_W = 190;
const COL_GAP = 110;
const ROW_GAP = 26;
let view = { x: 0, y: 0, w: 1000, h: 600 };
let bounds = { x: 0, y: 0, w: 1000, h: 600 };

export function toggleModuleMap(force) {
  const map = $("#modmap");
  const show = force ?? map.hidden;
  map.hidden = !show;
  if (show) {
    renderModuleMap();
    fitMap();
  }
}

export function refreshModuleMap() {
  if (!$("#modmap").hidden) renderModuleMap();
}

/** Columns: modules nobody uses on the left, what they use to the right. */
function layout(modules) {
  const usedBy = modules.map(() => []);
  modules.forEach((m, i) => m.uses.forEach((u) => usedBy[u]?.push(i)));
  const level = new Array(modules.length).fill(0);
  const cycle = new Set();
  const state = new Array(modules.length).fill(0); // 0 new, 1 visiting, 2 done
  const visit = (i, depth) => {
    if (state[i] === 1) return;
    if (depth <= level[i] && state[i] === 2) return;
    level[i] = Math.max(level[i], depth);
    state[i] = 1;
    for (const u of modules[i].uses) {
      if (state[u] === 1) cycle.add(`${i}>${u}`);
      else visit(u, level[i] + 1);
    }
    state[i] = 2;
  };
  modules.forEach((m, i) => {
    if (!usedBy[i].length || m.entry) visit(i, 0);
  });
  modules.forEach((_, i) => state[i] === 0 && visit(i, 0));
  const columns = [];
  modules.forEach((m, i) => (columns[level[i]] ||= []).push(i));
  const pos = [];
  const heights = modules.map((m) => 58 + Math.min(m.items.length, 6) * 17 + (m.items.length > 6 ? 17 : 0));
  const tallest = Math.max(
    0,
    ...columns.map((col) => col.reduce((sum, i) => sum + heights[i] + ROW_GAP, -ROW_GAP))
  );
  columns.forEach((col, c) => {
    col.sort((a, b) => Number(modules[b].entry) - Number(modules[a].entry) || modules[a].name.localeCompare(modules[b].name));
    const total = col.reduce((sum, i) => sum + heights[i] + ROW_GAP, -ROW_GAP);
    let y = (tallest - total) / 2;
    for (const i of col) {
      pos[i] = { x: c * (NODE_W + COL_GAP), y, h: heights[i] };
      y += heights[i] + ROW_GAP;
    }
  });
  return { pos, cycle, width: columns.length * (NODE_W + COL_GAP) - COL_GAP, height: tallest };
}

function renderModuleMap() {
  const modules = ctx.state.modules || [];
  const canvas = $("#modmap-svg");
  const info = $("#modmap-info");
  canvas.replaceChildren();
  if (!modules.length) {
    info.textContent = "Open a .belsk2 file to see how its modules use each other.";
    return;
  }
  const errors = modules.reduce((n, m) => n + m.errors, 0);
  info.textContent = `${modules.length} module${modules.length === 1 ? "" : "s"} · ${modules.reduce((n, m) => n + m.uses.length, 0)} using · ${errors ? `${errors} error${errors === 1 ? "" : "s"}` : "no errors"}`;

  const { pos, cycle, width, height } = layout(modules);
  bounds = { x: -40, y: -40, w: Math.max(width, 200) + 80, h: Math.max(height, 100) + 80 };
  const defs = svg(
    "defs",
    {},
    svg("marker", { id: "arrow", viewBox: "0 0 10 10", refX: "9", refY: "5", markerWidth: "7", markerHeight: "7", orient: "auto-start-reverse" },
      svg("path", { d: "M 0 0 L 10 5 L 0 10 z", class: "arrow-head" })),
    svg("marker", { id: "arrow-bad", viewBox: "0 0 10 10", refX: "9", refY: "5", markerWidth: "7", markerHeight: "7", orient: "auto-start-reverse" },
      svg("path", { d: "M 0 0 L 10 5 L 0 10 z", class: "arrow-head bad" }))
  );
  const edges = svg("g", { class: "edges" });
  const nodes = svg("g", { class: "nodes" });
  canvas.append(defs, edges, nodes);

  modules.forEach((m, i) => {
    for (const u of m.uses) {
      const a = pos[i];
      const b = pos[u];
      if (!a || !b) continue;
      const bad = cycle.has(`${i}>${u}`);
      let d;
      if (b.x > a.x) {
        const x1 = a.x + NODE_W;
        const y1 = a.y + 28;
        const x2 = b.x - 2;
        const y2 = b.y + 28;
        const mid = (x1 + x2) / 2;
        d = `M ${x1} ${y1} C ${mid} ${y1}, ${mid} ${y2}, ${x2} ${y2}`;
      } else {
        // Back edge (a cycle or a same-column use): loop around below.
        const x1 = a.x + NODE_W / 2;
        const y1 = a.y + a.h;
        const x2 = b.x + NODE_W / 2;
        const y2 = b.y + b.h + 2;
        const low = Math.max(y1, y2) + 50;
        d = `M ${x1} ${y1} C ${x1} ${low}, ${x2} ${low}, ${x2} ${y2}`;
      }
      edges.append(svg("path", { d, class: `edge${bad ? " bad" : ""}`, "data-from": i, "data-to": u, "marker-end": bad ? "url(#arrow-bad)" : "url(#arrow)" }));
    }
  });

  modules.forEach((m, i) => {
    const p = pos[i];
    if (!p) return;
    const cls = ["node", m.entry && "entry", m.dependency && "dep", m.errors && "has-errors", m.path === ctx.state.active && "current"]
      .filter(Boolean)
      .join(" ");
    const g = svg("g", { class: cls, transform: `translate(${p.x} ${p.y})`, "data-i": i });
    g.append(
      svg("rect", { width: NODE_W, height: p.h, rx: 12 }),
      svg("rect", { class: "stripe", width: 4, height: p.h - 20, x: 0, y: 10, rx: 2 }),
      svg("text", { x: 16, y: 24, class: "title" }, m.name),
      svg("text", { x: 16, y: 42, class: "sub" }, m.dependency ? `package ${m.package}` : m.entry ? "entry point" : baseName(m.path))
    );
    if (m.errors) {
      g.append(
        svg("circle", { cx: NODE_W - 18, cy: 20, r: 10, class: "badge" }),
        svg("text", { x: NODE_W - 18, y: 24, class: "badge-text", "text-anchor": "middle" }, String(m.errors))
      );
    }
    m.items.slice(0, 6).forEach((item, k) => g.append(svg("text", { x: 16, y: 64 + k * 17, class: "item" }, item)));
    if (m.items.length > 6) g.append(svg("text", { x: 16, y: 64 + 6 * 17, class: "item more" }, `+${m.items.length - 6} more`));
    const title = svg("title", {}, `${m.path}\nClick to open`);
    g.append(title);
    g.addEventListener("click", () => {
      toggleModuleMap(false);
      ctx.openFile(m.path);
    });
    g.addEventListener("mouseenter", () => highlight(i, true));
    g.addEventListener("mouseleave", () => highlight(i, false));
    nodes.append(g);
  });
  applyView();
}

function highlight(i, on) {
  const canvas = $("#modmap-svg");
  canvas.classList.toggle("focused", on);
  for (const e of canvas.querySelectorAll(".edge")) {
    const hit = e.dataset.from === String(i) || e.dataset.to === String(i);
    e.classList.toggle("lit", on && hit);
  }
  for (const n of canvas.querySelectorAll(".node")) {
    const j = Number(n.dataset.i);
    const linked =
      j === i ||
      [...canvas.querySelectorAll(".edge.lit")].some((e) => e.dataset.from === String(j) || e.dataset.to === String(j));
    n.classList.toggle("lit", on && linked);
  }
}

function applyView() {
  $("#modmap-svg").setAttribute("viewBox", `${view.x} ${view.y} ${view.w} ${view.h}`);
}

function fitMap() {
  const box = $("#modmap-svg").getBoundingClientRect();
  const aspect = box.width / Math.max(box.height, 1) || 1.6;
  let w = bounds.w;
  let h = bounds.h;
  if (w / h > aspect) h = w / aspect;
  else w = h * aspect;
  // Do not blow small graphs up.
  const minW = box.width * 0.9;
  if (w < minW) {
    h *= minW / w;
    w = minW;
  }
  view = { x: bounds.x - (w - bounds.w) / 2, y: bounds.y - (h - bounds.h) / 2, w, h };
  applyView();
}

// Pan with the mouse, zoom with the wheel.
export function enableMapGestures() {
  const canvas = $("#modmap-svg");
  let drag = null;
  canvas.addEventListener("pointerdown", (ev) => {
    if (ev.target.closest(".node")) return;
    drag = { x: ev.clientX, y: ev.clientY, view: { ...view } };
    canvas.setPointerCapture(ev.pointerId);
    canvas.classList.add("dragging");
  });
  canvas.addEventListener("pointermove", (ev) => {
    if (!drag) return;
    const scale = view.w / canvas.getBoundingClientRect().width;
    view = { ...drag.view, x: drag.view.x - (ev.clientX - drag.x) * scale, y: drag.view.y - (ev.clientY - drag.y) * scale };
    applyView();
  });
  canvas.addEventListener("pointerup", () => {
    drag = null;
    canvas.classList.remove("dragging");
  });
  canvas.addEventListener(
    "wheel",
    (ev) => {
      ev.preventDefault();
      const rect = canvas.getBoundingClientRect();
      const factor = ev.deltaY > 0 ? 1.12 : 1 / 1.12;
      const mx = view.x + ((ev.clientX - rect.left) / rect.width) * view.w;
      const my = view.y + ((ev.clientY - rect.top) / rect.height) * view.h;
      const w = Math.min(8000, Math.max(200, view.w * factor));
      const h = (w / view.w) * view.h;
      view = { x: mx - ((mx - view.x) * w) / view.w, y: my - ((my - view.y) * h) / view.h, w, h };
      applyView();
    },
    { passive: false }
  );
}
