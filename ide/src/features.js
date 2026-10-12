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
  $("#modmap-fit").onclick = () => fitMap();
  renderIdb();
}

// ---- live values ---------------------------------------------------------------

let previousGlobals = new Map();

export function setTrace(t) {
  previousIdb = new Map((trace?.idb || []).map(([k, v]) => [k, v]));
  previousGlobals = new Map(Object.entries(trace?.globals || {}));
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

// ---- state inspector: module variables and idb -----------------------------------------

function table(head, rows, previous) {
  const t = document.createElement("table");
  t.className = "idb-table";
  t.innerHTML = `<thead><tr><th>${head}</th><th>value</th></tr></thead>`;
  const body = document.createElement("tbody");
  for (const [key, value] of rows) {
    const row = document.createElement("tr");
    if (previous.size && previous.has(key) && previous.get(key) !== value) row.className = "changed";
    const a = document.createElement("td");
    a.textContent = key;
    const b = document.createElement("td");
    b.textContent = value;
    b.title = value;
    row.append(a, b);
    body.append(row);
  }
  t.append(body);
  return t;
}

function section(title, content) {
  const box = document.createElement("div");
  box.className = "state-section";
  const h = document.createElement("div");
  h.className = "state-title";
  h.textContent = title;
  box.append(h, content);
  return box;
}

function note(text) {
  return Object.assign(document.createElement("div"), { className: "empty", textContent: text });
}

function renderIdb() {
  const list = $("#idb-list");
  if (!trace) {
    list.replaceChildren(note("Run a program to see its variables and idb slots here. They update live while it waits for input."));
    return;
  }
  const globals = Object.entries(trace.globals || {});
  const slots = trace.idb || [];
  list.replaceChildren(
    section(
      "Module variables",
      globals.length ? table("name", globals, previousGlobals) : note("No top-level variables.")
    ),
    section("idb", slots.length ? table("slot", slots, previousIdb) : note("This run did not use idb (`idb 1 = value;`)."))
  );
}

// ---- module map ------------------------------------------------------------------------
// Four views of the same code: modules and their `using`s; functions (and
// top-level variables) grouped by module with their calls; functions with
// their parameters and variables; and user-defined types with their
// members, bases and the types their fields hold. Everything can be
// dragged; positions are remembered per project and view.

const NODE_W = 200;
const COL_GAP = 120;
const ROW_GAP = 26;
const PAD = 14;
const HEAD = 34;
const MODES = ["modules", "functions", "variables", "classes"];
const CLASS_W = 260;
const MAX_MEMBERS = 12;
let mode = loadJson("mapMode", "modules");
if (!MODES.includes(mode)) mode = "modules";
let view = { x: 0, y: 0, w: 1000, h: 600 };
let bounds = { x: 0, y: 0, w: 1000, h: 600 };
/** The graph being shown: groups (modules), nodes and edges. */
let graph = { groups: [], nodes: [], edges: [] };

function loadJson(key, fallback) {
  try {
    return JSON.parse(localStorage.getItem(key)) ?? fallback;
  } catch {
    return fallback;
  }
}
function saveJson(key, value) {
  try {
    localStorage.setItem(key, JSON.stringify(value));
  } catch {}
}
const posKey = () => `mapPos:${ctx.state.folder?.root ?? ""}:${mode}`;

/** Shows the map in the editor area (it lives in its own tab). */
export function showModuleMap() {
  const first = $("#modmap").hidden;
  $("#modmap").hidden = false;
  renderModuleMap();
  if (first) fitMap();
}

export function hideModuleMap() {
  $("#modmap").hidden = true;
}

export function refreshModuleMap() {
  if (!$("#modmap").hidden) renderModuleMap();
}

function setMode(m) {
  mode = m;
  saveJson("mapMode", m);
  renderModuleMap();
  fitMap();
}

/** Columns: modules nobody uses on the left, what they use to the right. */
function moduleLevels(modules) {
  const usedBy = modules.map(() => []);
  modules.forEach((m, i) => m.uses.forEach((u) => usedBy[u]?.push(i)));
  const level = new Array(modules.length).fill(0);
  const cycle = new Set();
  const seen = new Array(modules.length).fill(0); // 0 new, 1 visiting, 2 done
  const visit = (i, depth) => {
    if (seen[i] === 1) return;
    if (seen[i] === 2 && depth <= level[i]) return;
    level[i] = Math.max(level[i], depth);
    seen[i] = 1;
    for (const u of modules[i].uses) {
      if (seen[u] === 1) cycle.add(`${i}>${u}`);
      else visit(u, level[i] + 1);
    }
    seen[i] = 2;
  };
  modules.forEach((m, i) => (!usedBy[i].length || m.entry) && visit(i, 0));
  modules.forEach((_, i) => seen[i] === 0 && visit(i, 0));
  return { level, cycle };
}

/** Places boxes of the given heights in columns, centered vertically. */
function columns(levels, heights, width) {
  const cols = [];
  levels.forEach((l, i) => (cols[l] ||= []).push(i));
  const tallest = Math.max(0, ...cols.map((c) => (c || []).reduce((s, i) => s + heights[i] + ROW_GAP, -ROW_GAP)));
  const pos = [];
  cols.forEach((col, c) => {
    const total = col.reduce((s, i) => s + heights[i] + ROW_GAP, -ROW_GAP);
    let y = (tallest - total) / 2;
    for (const i of col) {
      pos[i] = { x: c * (width + COL_GAP), y };
      y += heights[i] + ROW_GAP;
    }
  });
  return pos;
}

function moduleGraph(modules) {
  const { level, cycle } = moduleLevels(modules);
  const heights = modules.map((m) => 58 + Math.min(m.items.length, 6) * 17 + (m.items.length > 6 ? 17 : 0));
  const pos = columns(level, heights, NODE_W);
  const nodes = modules.map((m, i) => ({
    key: `m:${m.path}`,
    x: pos[i].x,
    y: pos[i].y,
    w: NODE_W,
    h: heights[i],
    title: m.name,
    sub: m.dependency ? `package ${m.package}` : m.entry ? "entry point" : baseName(m.path),
    lines: m.items.slice(0, 6).concat(m.items.length > 6 ? [`+${m.items.length - 6} more`] : []),
    cls: ["module", m.entry && "entry", m.dependency && "dep", m.errors && "has-errors", m.path === ctx.state.active && "current"],
    errors: m.errors,
    open: { path: m.path, line: 1 },
  }));
  const edges = [];
  modules.forEach((m, i) =>
    m.uses.forEach((u) => nodes[u] && edges.push({ from: nodes[i].key, to: nodes[u].key, bad: cycle.has(`${i}>${u}`) }))
  );
  return { groups: [], nodes, edges };
}

function codeGraph(modules, functions, globals, withLocals) {
  // Children of each module: its functions, then its top-level variables.
  const children = modules.map(() => []);
  functions.forEach((f, i) => {
    const locals = withLocals ? f.locals : [];
    const lines = withLocals
      ? locals.length
        ? locals.map((l) => `${l.param ? "◦" : "•"} ${l.name}: ${l.ty}`)
        : ["(no variables)"]
      : [f.signature.replace(/^fn /, "")];
    const h = 44 + lines.length * 17;
    children[f.module ?? 0]?.push({
      key: `f:${i}`,
      w: NODE_W,
      h,
      title: `fn ${f.name}`,
      lines,
      cls: ["fn", f.name === "main" && modules[f.module]?.entry && "entry"],
      open: { path: modules[f.module]?.path, line: f.line },
    });
  });
  globals.forEach((g, i) =>
    children[g.module ?? 0]?.push({
      key: `g:${i}`,
      w: NODE_W,
      h: 30,
      title: `var ${g.name}: ${g.ty}`,
      lines: [],
      cls: ["global"],
      open: { path: modules[g.module]?.path, line: g.line },
    })
  );
  const { groups, nodes } = grouped(modules, children, NODE_W);
  const edges = [];
  functions.forEach((f, i) => {
    for (const c of f.calls) if (c !== i) edges.push({ from: `f:${i}`, to: `f:${c}` });
    for (const g of f.globals) edges.push({ from: `f:${i}`, to: `g:${g}`, data: true });
  });
  return { groups, nodes, edges };
}

/** Places the children of each module in a group box, modules in columns. */
function grouped(modules, children, width) {
  const { level } = moduleLevels(modules);
  const groupW = width + PAD * 2;
  const heights = children.map((c) => HEAD + c.reduce((s, n) => s + n.h + 10, 0) + PAD - 10 + (c.length ? 0 : 20));
  const pos = columns(level, heights, groupW);
  const groups = [];
  const nodes = [];
  modules.forEach((m, i) => {
    const group = {
      key: `m:${m.path}`,
      x: pos[i].x,
      y: pos[i].y,
      w: groupW,
      h: heights[i],
      title: m.name,
      sub: m.dependency ? `package ${m.package}` : baseName(m.path),
      cls: ["group", m.entry && "entry", m.dependency && "dep", m.errors && "has-errors"],
      errors: m.errors,
      open: { path: m.path, line: 1 },
    };
    groups.push(group);
    let y = HEAD;
    for (const n of children[i]) {
      nodes.push({ ...n, group: group.key, rx: PAD, ry: y });
      y += n.h + 10;
    }
  });
  return { groups, nodes };
}

/** Shortens a line to fit a box of width `w`. */
function fit(line, w) {
  const max = Math.floor((w - 24) / 7);
  return line.length > max ? `${line.slice(0, max - 1)}…` : line;
}

function classGraph(modules, types) {
  const children = modules.map(() => []);
  types.forEach((t, i) => {
    const members = t.members.slice(0, MAX_MEMBERS);
    if (t.members.length > MAX_MEMBERS) members.push(`+${t.members.length - MAX_MEMBERS} more`);
    const lines = (members.length ? members : ["(no members)"]).map((l) => fit(l, CLASS_W));
    children[t.module ?? 0]?.push({
      key: `t:${i}`,
      w: CLASS_W,
      h: 44 + lines.length * 17,
      title: fit(t.signature, CLASS_W),
      lines,
      cls: [t.kind],
      open: { path: modules[t.module]?.path, line: t.line },
    });
  });
  const { groups, nodes } = grouped(modules, children, CLASS_W);
  const edges = [];
  types.forEach((t, i) => {
    for (const b of t.bases) edges.push({ from: `t:${i}`, to: `t:${b}`, inherit: true });
    for (const h of t.holds) if (!t.bases.includes(h)) edges.push({ from: `t:${i}`, to: `t:${h}`, data: true });
  });
  return { groups, nodes, edges };
}

/** Absolute position of a node (children are placed relative to their group). */
function abs(n) {
  if (!n.group) return { x: n.x, y: n.y };
  const g = graph.groups.find((g) => g.key === n.group);
  return { x: (g?.x ?? 0) + n.rx, y: (g?.y ?? 0) + n.ry };
}

function edgePath(a, b) {
  const pa = abs(a);
  const pb = abs(b);
  const ya = pa.y + Math.min(a.h / 2, 22);
  const yb = pb.y + Math.min(b.h / 2, 22);
  if (pb.x >= pa.x + a.w) {
    const x1 = pa.x + a.w;
    const x2 = pb.x - 2;
    const mid = (x1 + x2) / 2;
    return `M ${x1} ${ya} C ${mid} ${ya}, ${mid} ${yb}, ${x2} ${yb}`;
  }
  if (pb.x + b.w <= pa.x) {
    const x1 = pa.x;
    const x2 = pb.x + b.w + 2;
    const mid = (x1 + x2) / 2;
    return `M ${x1} ${ya} C ${mid} ${ya}, ${mid} ${yb}, ${x2} ${yb}`;
  }
  // Side by side vertically: loop out to the right.
  const x1 = pa.x + a.w;
  const x2 = pb.x + b.w + 2;
  const out = Math.max(x1, x2) + 40 + Math.abs(yb - ya) * 0.15;
  return `M ${x1} ${ya} C ${out} ${ya}, ${out} ${yb}, ${x2} ${yb}`;
}

function renderModuleMap() {
  const s = ctx.state;
  const modules = s.modules || [];
  const canvas = $("#modmap-svg");
  for (const b of document.querySelectorAll("#modmap-modes button")) b.classList.toggle("active", b.dataset.mode === mode);
  canvas.replaceChildren();
  if (!modules.length) {
    $("#modmap-info").textContent = "Open a .belsk2 file to see how its code fits together.";
    return;
  }
  graph =
    mode === "modules"
      ? moduleGraph(modules)
      : mode === "classes"
        ? classGraph(modules, s.types || [])
        : codeGraph(modules, s.functions || [], s.globals || [], mode === "variables");
  // Positions the user dragged things to.
  const saved = loadJson(posKey(), {});
  for (const item of [...graph.groups, ...graph.nodes]) {
    const p = saved[item.key];
    if (!p) continue;
    if (item.group) [item.rx, item.ry] = p;
    else [item.x, item.y] = p;
  }

  const errors = modules.reduce((n, m) => n + m.errors, 0);
  const counts =
    mode === "modules"
      ? `${modules.length} modules · ${graph.edges.length} using`
      : mode === "classes"
        ? `${(s.types || []).length} types · ${graph.edges.filter((e) => e.inherit).length} bases · ${graph.edges.filter((e) => e.data).length} fields`
        : `${(s.functions || []).length} functions · ${(s.globals || []).length} globals · ${graph.edges.length} links`;
  $("#modmap-info").textContent = `${counts} · ${errors ? `${errors} error${errors === 1 ? "" : "s"}` : "no errors"}`;

  const marker = (id, cls) =>
    svg("marker", { id, viewBox: "0 0 10 10", refX: "9", refY: "5", markerWidth: "7", markerHeight: "7", orient: "auto-start-reverse" },
      svg("path", { d: "M 0 0 L 10 5 L 0 10 z", class: `arrow-head ${cls}` }));
  canvas.append(
    svg("defs", {}, marker("arrow", ""), marker("arrow-bad", "bad"), marker("arrow-data", "data"), marker("arrow-inherit", "inherit")),
    svg("g", { class: "groups" }),
    svg("g", { class: "edges" }),
    svg("g", { class: "nodes" })
  );
  for (const g of graph.groups) canvas.querySelector(".groups").append(renderBox(g, true));
  for (const n of graph.nodes) canvas.querySelector(".nodes").append(renderBox(n, false));
  renderEdges();
  computeBounds();
  applyView();
}

function renderBox(item, isGroup) {
  const p = abs(item);
  const el = svg("g", { class: [isGroup ? "group" : "node", ...item.cls].filter(Boolean).join(" "), transform: `translate(${p.x} ${p.y})`, "data-key": item.key });
  el.append(svg("rect", { class: "box", width: item.w, height: item.h, rx: isGroup ? 16 : 12 }));
  if (!isGroup) el.append(svg("rect", { class: "stripe", width: 4, height: Math.max(item.h - 20, 8), x: 0, y: Math.min(10, item.h / 2 - 4), rx: 2 }));
  const small = item.h <= 32;
  el.append(svg("text", { x: 16, y: small ? 19 : isGroup ? 22 : 24, class: isGroup ? "group-title" : small ? "item" : "title" }, item.title));
  if (item.sub) el.append(svg("text", { x: isGroup ? 16 + item.title.length * 8.5 + 10 : 16, y: isGroup ? 22 : 42, class: "sub" }, item.sub));
  const top = item.sub && !isGroup ? 64 : 44;
  (item.lines || []).forEach((line, k) => el.append(svg("text", { x: 16, y: top + k * 17, class: "item" }, line)));
  if (item.errors) {
    el.append(
      svg("circle", { cx: item.w - 18, cy: 20, r: 10, class: "badge" }),
      svg("text", { x: item.w - 18, y: 24, class: "badge-text", "text-anchor": "middle" }, String(item.errors))
    );
  }
  el.append(svg("title", {}, `${item.open?.path ?? ""}${item.open?.line > 1 ? `:${item.open.line}` : ""}\nDrag to move · click to open`));
  dragBox(el, item, isGroup);
  el.addEventListener("mouseenter", () => highlight(item.key, true));
  el.addEventListener("mouseleave", () => highlight(item.key, false));
  return el;
}

function renderEdges() {
  const layer = $("#modmap-svg .edges");
  if (!layer) return;
  const byKey = new Map([...graph.groups, ...graph.nodes].map((n) => [n.key, n]));
  layer.replaceChildren(
    ...graph.edges
      .filter((e) => byKey.get(e.from) && byKey.get(e.to))
      .map((e) =>
        svg("path", {
          d: edgePath(byKey.get(e.from), byKey.get(e.to)),
          class: `edge${e.bad ? " bad" : ""}${e.data ? " data" : ""}${e.inherit ? " inherit" : ""}`,
          "data-from": e.from,
          "data-to": e.to,
          "marker-end": e.bad
            ? "url(#arrow-bad)"
            : e.data
              ? "url(#arrow-data)"
              : e.inherit
                ? "url(#arrow-inherit)"
                : "url(#arrow)",
        })
      )
  );
}

function computeBounds() {
  const items = [...graph.groups, ...graph.nodes.filter((n) => !n.group)];
  if (!items.length) return;
  const x1 = Math.min(...items.map((i) => i.x));
  const y1 = Math.min(...items.map((i) => i.y));
  const x2 = Math.max(...items.map((i) => i.x + i.w));
  const y2 = Math.max(...items.map((i) => i.y + i.h));
  bounds = { x: x1 - 50, y: y1 - 50, w: x2 - x1 + 100, h: y2 - y1 + 100 };
}

/** Moves a box with the mouse; a click without movement opens the code. */
function dragBox(el, item, isGroup) {
  el.addEventListener("pointerdown", (ev) => {
    if (ev.button !== 0) return;
    ev.stopPropagation();
    const canvas = $("#modmap-svg");
    const scale = view.w / canvas.getBoundingClientRect().width;
    const start = { x: ev.clientX, y: ev.clientY, ix: isGroup || !item.group ? item.x : item.rx, iy: isGroup || !item.group ? item.y : item.ry };
    let moved = false;
    el.setPointerCapture(ev.pointerId);
    el.classList.add("dragging");
    const move = (m) => {
      const dx = (m.clientX - start.x) * scale;
      const dy = (m.clientY - start.y) * scale;
      if (!moved && Math.hypot(m.clientX - start.x, m.clientY - start.y) < 4) return;
      moved = true;
      if (item.group) {
        item.rx = start.ix + dx;
        item.ry = start.iy + dy;
      } else {
        item.x = start.ix + dx;
        item.y = start.iy + dy;
      }
      const p = abs(item);
      el.setAttribute("transform", `translate(${p.x} ${p.y})`);
      if (isGroup) {
        // Children move with their module.
        for (const n of graph.nodes.filter((n) => n.group === item.key)) {
          const q = abs(n);
          $("#modmap-svg").querySelector(`[data-key="${CSS.escape(n.key)}"]`)?.setAttribute("transform", `translate(${q.x} ${q.y})`);
        }
      }
      renderEdges();
    };
    const up = () => {
      el.removeEventListener("pointermove", move);
      el.removeEventListener("pointerup", up);
      el.classList.remove("dragging");
      if (!moved) {
        if (item.open?.path) {
          ctx.openFile(item.open.path, item.open.line > 1 ? { chars: { start: { line: item.open.line, col: 1 }, end: { line: item.open.line, col: 1 } } } : undefined);
        }
        return;
      }
      const saved = loadJson(posKey(), {});
      saved[item.key] = item.group ? [item.rx, item.ry] : [item.x, item.y];
      saveJson(posKey(), saved);
      computeBounds();
    };
    el.addEventListener("pointermove", move);
    el.addEventListener("pointerup", up);
  });
}

function resetLayout() {
  saveJson(posKey(), {});
  renderModuleMap();
  fitMap();
}

function highlight(key, on) {
  const canvas = $("#modmap-svg");
  canvas.classList.toggle("focused", on);
  const linked = new Set([key]);
  // A module's highlight covers its functions too.
  for (const n of graph.nodes) if (n.group === key) linked.add(n.key);
  for (const e of canvas.querySelectorAll(".edge")) {
    const hit = on && (linked.has(e.dataset.from) || linked.has(e.dataset.to));
    e.classList.toggle("lit", hit);
    if (hit) {
      linked.add(e.dataset.from);
      linked.add(e.dataset.to);
    }
  }
  for (const n of canvas.querySelectorAll(".node, .group")) n.classList.toggle("lit", on && linked.has(n.dataset.key));
  if (on) {
    // Keep the module of a linked function visible.
    for (const n of graph.nodes) if (linked.has(n.key) && n.group) canvas.querySelector(`[data-key="${CSS.escape(n.group)}"]`)?.classList.add("lit");
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

/** Pan by dragging the background, zoom with the wheel; mode buttons. */
export function enableMapGestures() {
  const canvas = $("#modmap-svg");
  for (const b of document.querySelectorAll("#modmap-modes button[data-mode]")) b.onclick = () => setMode(b.dataset.mode);
  $("#modmap-reset").onclick = () => resetLayout();
  let drag = null;
  canvas.addEventListener("pointerdown", (ev) => {
    drag = { x: ev.clientX, y: ev.clientY, view: { ...view } };
    canvas.setPointerCapture(ev.pointerId);
    canvas.classList.add("panning");
  });
  canvas.addEventListener("pointermove", (ev) => {
    if (!drag) return;
    const scale = view.w / canvas.getBoundingClientRect().width;
    view = { ...drag.view, x: drag.view.x - (ev.clientX - drag.x) * scale, y: drag.view.y - (ev.clientY - drag.y) * scale };
    applyView();
  });
  canvas.addEventListener("pointerup", () => {
    drag = null;
    canvas.classList.remove("panning");
  });
  canvas.addEventListener(
    "wheel",
    (ev) => {
      ev.preventDefault();
      const rect = canvas.getBoundingClientRect();
      const factor = ev.deltaY > 0 ? 1.12 : 1 / 1.12;
      const mx = view.x + ((ev.clientX - rect.left) / rect.width) * view.w;
      const my = view.y + ((ev.clientY - rect.top) / rect.height) * view.h;
      const w = Math.min(10000, Math.max(200, view.w * factor));
      const h = (w / view.w) * view.h;
      view = { x: mx - ((mx - view.x) * w) / view.w, y: my - ((my - view.y) * h) / view.h, w, h };
      applyView();
    },
    { passive: false }
  );
}
