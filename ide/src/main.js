// The workbench: explorer, tabs, editor, output and problems panels,
// status bar, command palette and quick open.

import { call, listen, pickFolder, isDemo } from "./backend.js";
import { monaco, models, hooks, createEditor, analyze, bumpVersion, languageFor, toRange } from "./editor.js";

const $ = (sel) => document.querySelector(sel);
const el = (tag, props = {}, ...children) => {
  const e = Object.assign(document.createElement(tag), props);
  e.append(...children);
  return e;
};
const sep = (p) => (p.includes("\\") ? "\\" : "/");
const baseName = (p) => p.split(/[\\/]/).pop();
const dirName = (p) => p.slice(0, Math.max(p.lastIndexOf("/"), p.lastIndexOf("\\")));

const state = {
  folder: null,
  tabs: [],
  active: null,
  expanded: new Set(),
  selectedDir: null,
  viewStates: new Map(),
  problems: [],
  running: false,
};

const editor = createEditor($("#editor"));

// ---- storage ---------------------------------------------------------------

function load(key, fallback) {
  try {
    return JSON.parse(localStorage.getItem(key)) ?? fallback;
  } catch {
    return fallback;
  }
}
function save(key, value) {
  try {
    localStorage.setItem(key, JSON.stringify(value));
  } catch {}
}

// ---- files and models --------------------------------------------------------

async function ensureModel(path) {
  let model = models.get(path);
  if (model) return model;
  const text = await call("read_file", { path });
  model = monaco.editor.createModel(text, languageFor(path), monaco.Uri.file(path));
  model.belsk2Saved = model.getAlternativeVersionId();
  model.belsk2Dirty = false;
  model.onDidChangeContent(() => {
    model.belsk2Dirty = model.getAlternativeVersionId() !== model.belsk2Saved;
    bumpVersion();
    renderTabs();
    scheduleAnalysis();
  });
  models.set(path, model);
  return model;
}
hooks.ensureModel = ensureModel;
hooks.open = (path, selection) => openFile(path, selection);

async function openFile(path, selection) {
  let model;
  try {
    model = await ensureModel(path);
  } catch (e) {
    return log(`${e}\n`, "stderr");
  }
  if (state.active && models.get(state.active)) state.viewStates.set(state.active, editor.saveViewState());
  if (!state.tabs.includes(path)) state.tabs.push(path);
  state.active = path;
  editor.setModel(model);
  const view = state.viewStates.get(path);
  if (view) editor.restoreViewState(view);
  if (selection?.chars) selection = toRange(model, selection.chars);
  if (selection) {
    const range = selection.startLineNumber
      ? selection
      : new monaco.Range(selection.lineNumber, selection.column, selection.lineNumber, selection.column);
    editor.setSelection(range);
    editor.revealRangeInCenterIfOutsideViewport(range);
  }
  editor.focus();
  $("#welcome").hidden = true;
  renderTabs();
  renderTree();
  updateStatus();
  scheduleAnalysis(0);
}

function closeTab(path) {
  const model = models.get(path);
  if (model?.belsk2Dirty && !confirm(`${baseName(path)} has unsaved changes. Close anyway?`)) return;
  state.tabs = state.tabs.filter((t) => t !== path);
  if (model) {
    model.dispose();
    models.delete(path);
  }
  state.viewStates.delete(path);
  if (state.active === path) {
    state.active = null;
    const next = state.tabs[state.tabs.length - 1];
    if (next) openFile(next);
    else {
      editor.setModel(null);
      $("#welcome").hidden = false;
      renderTabs();
      updateStatus();
    }
  } else renderTabs();
}

async function saveFile(path = state.active) {
  const model = path && models.get(path);
  if (!model) return;
  try {
    await call("write_file", { path, text: model.getValue() });
    model.belsk2Saved = model.getAlternativeVersionId();
    model.belsk2Dirty = false;
    bumpVersion();
    renderTabs();
  } catch (e) {
    log(`${e}\n`, "stderr");
  }
}

async function saveAll() {
  for (const [path, model] of models) if (model.belsk2Dirty) await saveFile(path);
}

// ---- folder and explorer -----------------------------------------------------

async function openFolder(path) {
  try {
    state.folder = await call("open_folder", { path });
  } catch (e) {
    return alert(String(e));
  }
  const recent = [state.folder.root, ...load("recent", []).filter((r) => r !== state.folder.root)].slice(0, 8);
  save("recent", recent);
  state.expanded.add(state.folder.root);
  for (const e of state.folder.entries) if (e.dir && e.name === "src") state.expanded.add(e.path);
  state.selectedDir = state.folder.root;
  $("#explorer-title").textContent = state.folder.name;
  $("#title-project").textContent = state.folder.project ? `${state.folder.project} — Belsk2 project` : state.folder.name;
  renderTree();
  updateStatus();
  const main = findEntry((e) => /[\\/]src[\\/]main\.belsk2$/.test(e.path)) || findEntry((e) => e.path.endsWith(".belsk2"));
  if (main && !state.active) openFile(main.path);
}

async function refreshFolder() {
  if (!state.folder) return;
  state.folder = await call("open_folder", { path: state.folder.root });
  renderTree();
}

function allFiles(entries = state.folder?.entries || [], out = []) {
  for (const e of entries) {
    if (e.dir) allFiles(e.children, out);
    else out.push(e);
  }
  return out;
}
function findEntry(pred) {
  return allFiles().find(pred);
}

function renderTree() {
  const tree = $("#tree");
  tree.replaceChildren();
  if (!state.folder) {
    tree.append(el("div", { className: "empty" }, el("button", { textContent: "Open Folder…", onclick: cmdOpenFolder })));
    return;
  }
  const errorFiles = new Set(state.problems.map((p) => p.path));
  const render = (entries, depth) => {
    const ul = el("ul");
    for (const e of entries) {
      const row = el("div", {
        className: "row" + (e.path === state.active ? " active" : "") + (errorFiles.has(e.path) ? " has-errors" : ""),
        title: e.path,
      });
      row.style.paddingLeft = `${8 + depth * 14}px`;
      const open = state.expanded.has(e.path);
      row.append(
        el("span", { className: "twisty", textContent: e.dir ? (open ? "▾" : "▸") : "" }),
        el("span", { className: "icon " + (e.dir ? "dir" : fileKind(e.name)) }),
        el("span", { className: "name", textContent: e.name })
      );
      row.onclick = () => {
        if (e.dir) {
          state.selectedDir = e.path;
          open ? state.expanded.delete(e.path) : state.expanded.add(e.path);
          renderTree();
        } else {
          state.selectedDir = dirName(e.path);
          openFile(e.path);
        }
      };
      row.oncontextmenu = (ev) => {
        ev.preventDefault();
        state.selectedDir = e.dir ? e.path : dirName(e.path);
        contextMenu(ev, [
          ...(e.dir
            ? [
                { label: "New File…", run: () => newEntry(false) },
                { label: "New Folder…", run: () => newEntry(true) },
              ]
            : [{ label: "Open", run: () => openFile(e.path) }]),
          { label: "Rename…", run: () => renamePath(e.path) },
          { label: "Delete", run: () => deletePath(e.path, e.dir), danger: true },
          { label: "Copy Path", run: () => navigator.clipboard?.writeText(e.path) },
        ]);
      };
      const li = el("li", {}, row);
      if (e.dir && open) li.append(render(e.children, depth + 1));
      ul.append(li);
    }
    return ul;
  };
  tree.append(render(state.folder.entries, 0));
}

function fileKind(name) {
  if (name.endsWith(".belsk2")) return "b2";
  if (name === "belsk2.toml" || name === "belsk2.lock") return "toml";
  return "file";
}

async function newEntry(isDir) {
  if (!state.folder) return;
  const base = state.selectedDir || state.folder.root;
  const name = await ask(isDir ? "New folder name" : "New file name (e.g. utils.belsk2)");
  if (!name) return;
  const path = base + sep(base) + name;
  try {
    const created = await call(isDir ? "create_dir" : "create_file", { path });
    state.expanded.add(base);
    await refreshFolder();
    if (!isDir) openFile(created);
  } catch (e) {
    alert(String(e));
  }
}

async function renamePath(path) {
  const name = await ask(`Rename ${baseName(path)} to`, baseName(path));
  if (!name || name === baseName(path)) return;
  const to = dirName(path) + sep(path) + name;
  try {
    const renamed = await call("rename_path", { from: path, to });
    if (models.has(path)) {
      const wasActive = state.active === path;
      const model = models.get(path);
      if (model.belsk2Dirty) await saveFile(path);
      state.tabs = state.tabs.filter((t) => t !== path);
      model.dispose();
      models.delete(path);
      if (wasActive) await openFile(renamed);
    }
    await refreshFolder();
  } catch (e) {
    alert(String(e));
  }
}

const under = (p, root) => p === root || p.startsWith(root + "/") || p.startsWith(root + "\\");

async function deletePath(path, isDir) {
  const what = isDir ? `the folder ${baseName(path)} and everything in it` : baseName(path);
  if (!confirm(`Move ${what} to the trash?`)) return;
  try {
    await call("delete_path", { path });
  } catch (e) {
    return alert(String(e));
  }
  for (const tab of state.tabs.filter((t) => under(t, path))) {
    const model = models.get(tab);
    if (model) model.belsk2Dirty = false;
    closeTab(tab);
  }
  await refreshFolder();
  scheduleAnalysis(0);
}

function contextMenu(ev, items) {
  const menu = $("#context-menu");
  menu.replaceChildren(
    ...items.map((it) =>
      el("li", {
        textContent: it.label,
        className: it.danger ? "danger" : "",
        onclick: () => {
          menu.hidden = true;
          it.run();
        },
      })
    )
  );
  menu.hidden = false;
  menu.style.left = `${Math.min(ev.clientX, innerWidth - 200)}px`;
  menu.style.top = `${Math.min(ev.clientY, innerHeight - items.length * 28 - 10)}px`;
}
window.addEventListener("pointerdown", (ev) => {
  if (!$("#context-menu").contains(ev.target)) $("#context-menu").hidden = true;
});

// ---- search ----

let searchTimer = null;
function scheduleSearch() {
  clearTimeout(searchTimer);
  searchTimer = setTimeout(runSearch, 250);
}

async function runSearch() {
  const query = $("#search-input").value;
  const results = $("#search-results");
  if (!state.folder || !query) {
    results.replaceChildren();
    $("#search-summary").textContent = state.folder ? "" : "Open a folder to search it.";
    return;
  }
  const matches = await call("search", { root: state.folder.root, query, case: $("#search-case").checked });
  const byFile = new Map();
  for (const m of matches) {
    if (!byFile.has(m.path)) byFile.set(m.path, []);
    byFile.get(m.path).push(m);
  }
  $("#search-summary").textContent = matches.length
    ? `${matches.length}${matches.length >= 2000 ? "+" : ""} results in ${byFile.size} files`
    : "No results";
  results.replaceChildren(
    ...[...byFile].map(([path, list]) => {
      const rel = path.slice(state.folder.root.length + 1);
      const group = el(
        "div",
        { className: "search-file" },
        el(
          "div",
          { className: "search-file-name", title: path },
          el("span", { className: "icon " + fileKind(path) }),
          el("span", { textContent: baseName(path) }),
          el("span", { className: "muted", textContent: dirName(rel) }),
          el("span", { className: "badge", textContent: list.length })
        )
      );
      for (const m of list) {
        const chars = [...m.text];
        const start = m.col - 1;
        // Keep a little context before the match on long lines.
        const from = Math.max(0, start - 30);
        group.append(
          el(
            "div",
            {
              className: "search-line",
              onclick: () =>
                openFile(m.path, { chars: { start: { line: m.line, col: m.col }, end: { line: m.line, col: m.col + m.len } } }),
            },
            el("span", { className: "muted", textContent: `${m.line}  ` }),
            (from > 0 ? "…" : "") + chars.slice(from, start).join("").trimStart(),
            el("mark", { textContent: chars.slice(start, start + m.len).join("") }),
            chars.slice(start + m.len).join("")
          )
        );
      }
      return group;
    })
  );
}

$("#search-input").oninput = scheduleSearch;
$("#search-case").onchange = runSearch;

function focusSearch() {
  if ($("#view-search").hidden || document.body.classList.contains("sidebar-hidden")) showView("search");
  const selected = editor.getModel()?.getValueInRange(editor.getSelection());
  if (selected && !selected.includes("\n")) {
    $("#search-input").value = selected;
    scheduleSearch();
  }
  $("#search-input").focus();
  $("#search-input").select();
}

// ---- tabs ----------------------------------------------------------------------

function renderTabs() {
  const tabs = $("#tabs");
  tabs.replaceChildren(
    ...state.tabs.map((path) => {
      const model = models.get(path);
      const errors = state.problems.filter((p) => p.path === path).length;
      const tab = el(
        "div",
        { className: "tab" + (path === state.active ? " active" : "") + (errors ? " has-errors" : ""), title: path },
        el("span", { className: "icon " + fileKind(path) }),
        el("span", { className: "name", textContent: baseName(path) }),
        el("span", {
          className: "close" + (model?.belsk2Dirty ? " dirty" : ""),
          textContent: model?.belsk2Dirty ? "●" : "×",
          onclick: (ev) => {
            ev.stopPropagation();
            closeTab(path);
          },
        })
      );
      tab.onclick = () => openFile(path);
      tab.onauxclick = (ev) => ev.button === 1 && closeTab(path);
      return tab;
    })
  );
}

// ---- analysis and problems -------------------------------------------------------

let analysisTimer = null;
function scheduleAnalysis(delay = 350) {
  clearTimeout(analysisTimer);
  analysisTimer = setTimeout(runAnalysis, delay);
}

async function runAnalysis() {
  const path = state.active;
  if (!path || !path.endsWith(".belsk2")) return;
  try {
    const report = await analyze(path);
    state.problems = report.diagnostics.map((d) => ({ ...d, path: d.path ?? path }));
    state.modules = report.modules;
  } catch (e) {
    console.error(e);
  }
  renderProblems();
  renderTabs();
  renderTree();
  updateStatus();
}

function renderProblems() {
  const make = () =>
    state.problems.length
      ? state.problems.map((p) =>
          el(
            "div",
            {
              className: "problem",
              onclick: () => openFile(p.path, { chars: p.range }),
            },
            el("span", { className: "sev", textContent: "⊗" }),
            el("span", { className: "msg", textContent: p.message }),
            el("span", { className: "where", textContent: `${baseName(p.path)}:${p.range.start.line}:${p.range.start.col}` })
          )
        )
      : [el("div", { className: "empty", textContent: "No problems. Nice." })];
  $("#panel-problems").replaceChildren(...make());
  $("#problems-side").replaceChildren(...make());
  $("#problem-count").textContent = state.problems.length;
}

// ---- output and running ------------------------------------------------------------

function log(text, stream = "stdout") {
  const out = $("#output");
  out.append(el("span", { className: stream, textContent: text }));
  out.scrollTop = out.scrollHeight;
}

function runTarget() {
  if (state.folder?.project) return state.folder.root + sep(state.folder.root) + "src" + sep(state.folder.root) + "main.belsk2";
  return state.active;
}

async function run(check = false) {
  const target = check ? state.active || runTarget() : runTarget();
  if (!target) return log("Open a file or a project to run it.\n", "info");
  await saveAll();
  showPanel("output");
  $("#output").replaceChildren();
  log(`${check ? "Checking" : "Running"} ${baseName(target)}…\n`, "info");
  try {
    await call("start", { path: target, check });
    setRunning(!check);
  } catch (e) {
    log(`${e}\n`, "stderr");
  }
}

function setRunning(on) {
  state.running = on;
  $("#btn-stop").disabled = !on;
  $("#stdin").disabled = !on;
  $("#st-run").textContent = on ? "● running" : "";
  if (on) $("#stdin").focus();
}

listen("run-output", (o) => log(o.text, o.stream));
listen("run-exit", (code) => {
  setRunning(false);
  log(`\n${code === 0 ? "Finished" : `Exited with code ${code ?? "?"}`}\n`, code === 0 ? "info" : "stderr");
});

$("#stdin-form").onsubmit = async (ev) => {
  ev.preventDefault();
  const input = $("#stdin");
  log(input.value + "\n", "stdin");
  try {
    await call("send_input", { text: input.value });
  } catch (e) {
    log(`${e}\n`, "stderr");
  }
  input.value = "";
};

// ---- panels, views, layout --------------------------------------------------------

function showPanel(name) {
  document.querySelectorAll(".panel-tabs [data-panel]").forEach((b) => b.classList.toggle("active", b.dataset.panel === name));
  $("#panel-output").hidden = name !== "output";
  $("#panel-problems").hidden = name !== "problems";
  document.body.classList.remove("panel-hidden");
}

function showView(name) {
  const same = !document.body.classList.contains("sidebar-hidden") && !$(`#view-${name}`).hidden;
  if (same) return document.body.classList.add("sidebar-hidden");
  document.body.classList.remove("sidebar-hidden");
  document.querySelectorAll("#activitybar [data-view]").forEach((b) => b.classList.toggle("active", b.dataset.view === name));
  document.querySelectorAll("#sidebar .view").forEach((v) => (v.hidden = v.id !== `view-${name}`));
}

document.querySelectorAll(".panel-tabs [data-panel]").forEach((b) => (b.onclick = () => showPanel(b.dataset.panel)));
document.querySelectorAll("#activitybar [data-view]").forEach((b) => (b.onclick = () => showView(b.dataset.view)));

function sash(id, cssVar, axis, min, max) {
  const handle = $(id);
  handle.onpointerdown = (ev) => {
    handle.setPointerCapture(ev.pointerId);
    const start = axis === "x" ? ev.clientX : ev.clientY;
    const initial = parseInt(getComputedStyle(document.documentElement).getPropertyValue(cssVar));
    handle.onpointermove = (m) => {
      const delta = (axis === "x" ? m.clientX : m.clientY) - start;
      const size = Math.min(max, Math.max(min, initial + (axis === "x" ? delta : -delta)));
      document.documentElement.style.setProperty(cssVar, `${size}px`);
    };
    handle.onpointerup = () => {
      handle.onpointermove = null;
      save(cssVar, getComputedStyle(document.documentElement).getPropertyValue(cssVar));
    };
  };
  const stored = load(cssVar, null);
  if (stored) document.documentElement.style.setProperty(cssVar, stored);
}
sash("#sash-side", "--sidebar-width", "x", 160, 600);
sash("#sash-panel", "--panel-height", "y", 80, 700);

// ---- status bar ------------------------------------------------------------------------

function updateStatus() {
  const errors = state.problems.length;
  $("#st-problems").textContent = `⊗ ${errors}`;
  $("#st-problems").classList.toggle("bad", errors > 0);
  $("#st-project").textContent = state.folder ? `⎇ ${state.folder.project || state.folder.name}` : isDemo ? "demo mode" : "";
  const model = editor.getModel();
  $("#st-lang").textContent = model ? (model.getLanguageId() === "belsk2" ? "Belsk2" : model.getLanguageId()) : "";
  const p = editor.getPosition();
  $("#st-pos").textContent = model && p ? `Ln ${p.lineNumber}, Col ${p.column}` : "";
}
editor.onDidChangeCursorPosition(updateStatus);
$("#st-problems").onclick = () => showPanel("problems");

// ---- prompt, palette, quick open ---------------------------------------------------------

function ask(label, value = "") {
  return new Promise((resolve) => {
    const dialog = $("#prompt-dialog");
    const input = $("#prompt-input");
    $("#prompt-label").textContent = label;
    input.value = value;
    dialog.hidden = false;
    input.focus();
    input.select();
    const done = (result) => {
      dialog.hidden = true;
      $("#prompt-form").onsubmit = null;
      input.onkeydown = null;
      resolve(result);
    };
    $("#prompt-form").onsubmit = (ev) => {
      ev.preventDefault();
      done(input.value.trim());
    };
    input.onkeydown = (ev) => ev.key === "Escape" && done(null);
    input.onblur = () => setTimeout(() => !dialog.hidden && done(null), 100);
  });
}

const commands = [
  { label: "File: Open Folder…", key: "Ctrl+O", run: () => cmdOpenFolder() },
  { label: "File: New Project…", run: () => cmdNewProject() },
  { label: "File: New File…", key: "Ctrl+N", run: () => newEntry(false) },
  { label: "File: New Folder…", run: () => newEntry(true) },
  { label: "File: Save", key: "Ctrl+S", run: () => saveFile() },
  { label: "File: Save All", key: "Ctrl+Alt+S", run: () => saveAll() },
  { label: "File: Close Editor", key: "Ctrl+W", run: () => state.active && closeTab(state.active) },
  { label: "Go to File…", key: "Ctrl+P", run: () => quickOpen() },
  { label: "Run: Run Program", key: "F5", run: () => run(false) },
  { label: "Run: Stop", key: "Shift+F5", run: () => call("stop") },
  { label: "Run: Check", key: "Ctrl+Shift+B", run: () => run(true) },
  { label: "View: Explorer", key: "Ctrl+Shift+E", run: () => showView("explorer") },
  { label: "Search: Find in Files", key: "Ctrl+Shift+F", run: () => focusSearch() },
  { label: "View: Problems", key: "Ctrl+Shift+M", run: () => showPanel("problems") },
  { label: "View: Output", run: () => showPanel("output") },
  { label: "View: Toggle Sidebar", key: "Ctrl+B", run: () => document.body.classList.toggle("sidebar-hidden") },
  { label: "View: Toggle Panel", key: "Ctrl+J", run: () => document.body.classList.toggle("panel-hidden") },
  { label: "Explorer: Refresh", run: () => refreshFolder() },
  { label: "Editor: Format Document", key: "Shift+Alt+F", run: () => editor.getAction("editor.action.formatDocument")?.run() },
  { label: "Editor: Rename Symbol", key: "F2", run: () => editor.getAction("editor.action.rename")?.run() },
  { label: "Editor: Go to Definition", key: "F12", run: () => editor.getAction("editor.action.revealDefinition")?.run() },
  { label: "Editor: Find References", key: "Shift+F12", run: () => editor.getAction("editor.action.goToReferences")?.run() },
];

function fuzzy(query, text) {
  query = query.toLowerCase();
  text = text.toLowerCase();
  let i = 0;
  let score = 0;
  for (let j = 0; j < text.length && i < query.length; j++) {
    if (text[j] === query[i]) {
      score += j === 0 || "/\\ :._-".includes(text[j - 1]) ? 3 : 1;
      i++;
    }
  }
  return i === query.length ? score : -1;
}

function palette(items, placeholder, initial = "") {
  const box = $("#palette");
  const input = $("#palette-input");
  const list = $("#palette-list");
  let shown = [];
  let selected = 0;
  const render = () => {
    const q = input.value.startsWith(">") ? input.value.slice(1).trim() : input.value.trim();
    shown = items()
      .map((it) => ({ it, score: q ? fuzzy(q, it.label + " " + (it.detail || "")) : 0 }))
      .filter((x) => x.score >= 0)
      .sort((a, b) => b.score - a.score)
      .slice(0, 50)
      .map((x) => x.it);
    selected = Math.min(selected, Math.max(0, shown.length - 1));
    list.replaceChildren(
      ...shown.map((it, i) =>
        el(
          "li",
          { className: i === selected ? "selected" : "", onmousedown: (ev) => (ev.preventDefault(), pick(i)) },
          el("span", { className: "label", textContent: it.label }),
          el("span", { className: "detail", textContent: it.detail || it.key || "" })
        )
      )
    );
    list.children[selected]?.scrollIntoView({ block: "nearest" });
  };
  const close = () => {
    box.hidden = true;
    input.oninput = input.onkeydown = input.onblur = null;
    editor.focus();
  };
  const pick = (i) => {
    const it = shown[i];
    close();
    it?.run();
  };
  input.placeholder = placeholder;
  input.value = initial;
  box.hidden = false;
  input.focus();
  input.oninput = () => {
    if (input.value.startsWith(">") && items !== commandItems) return (close(), palette(commandItems, "Type a command", input.value));
    selected = 0;
    render();
  };
  input.onkeydown = (ev) => {
    if (ev.key === "ArrowDown") selected = Math.min(shown.length - 1, selected + 1);
    else if (ev.key === "ArrowUp") selected = Math.max(0, selected - 1);
    else if (ev.key === "Enter") return pick(selected);
    else if (ev.key === "Escape") return close();
    else return;
    ev.preventDefault();
    render();
  };
  input.onblur = close;
  render();
}

const commandItems = () => commands;
const fileItems = () =>
  allFiles().map((f) => ({
    label: f.name,
    detail: state.folder ? f.path.slice(state.folder.root.length + 1) : f.path,
    run: () => openFile(f.path),
  }));

function quickOpen() {
  if (!state.folder) return palette(commandItems, "Type a command", ">");
  palette(fileItems, "Search files by name (type > for commands)");
}

// ---- commands ----------------------------------------------------------------------------

async function cmdOpenFolder() {
  const path = await pickFolder("Open Folder");
  if (path) openFolder(path);
}

async function cmdNewProject() {
  const name = await ask("Project name");
  if (!name) return;
  const parent = await pickFolder("Where to create the project");
  if (!parent) return;
  try {
    openFolder(await call("new_project", { parent, name, lib: false }));
  } catch (e) {
    alert(String(e));
  }
}

$("#btn-run").onclick = () => run(false);
$("#btn-stop").onclick = () => call("stop");
$("#btn-check").onclick = () => run(true);
$("#btn-palette").onclick = () => palette(commandItems, "Type a command", ">");
$("#btn-new-file").onclick = () => newEntry(false);
$("#btn-new-folder").onclick = () => newEntry(true);
$("#btn-refresh").onclick = () => refreshFolder();
$("#btn-clear").onclick = () => $("#output").replaceChildren();
$("#w-open").onclick = cmdOpenFolder;
$("#w-new").onclick = cmdNewProject;

const KEYS = {
  "ctrl+s": () => saveFile(),
  "ctrl+alt+s": () => saveAll(),
  "ctrl+o": () => cmdOpenFolder(),
  "ctrl+n": () => newEntry(false),
  "ctrl+w": () => state.active && closeTab(state.active),
  "ctrl+p": () => quickOpen(),
  "ctrl+shift+p": () => palette(commandItems, "Type a command", ">"),
  f1: () => palette(commandItems, "Type a command", ">"),
  f5: () => run(false),
  "shift+f5": () => call("stop"),
  "ctrl+shift+b": () => run(true),
  "ctrl+shift+e": () => showView("explorer"),
  "ctrl+shift+f": () => focusSearch(),
  "ctrl+shift+m": () => showPanel("problems"),
  "ctrl+b": () => document.body.classList.toggle("sidebar-hidden"),
  "ctrl+j": () => document.body.classList.toggle("panel-hidden"),
};
window.addEventListener(
  "keydown",
  (ev) => {
    const key = [ev.ctrlKey || ev.metaKey ? "ctrl" : "", ev.shiftKey ? "shift" : "", ev.altKey ? "alt" : "", ev.key.toLowerCase()]
      .filter(Boolean)
      .join("+");
    const action = KEYS[key];
    if (action) {
      ev.preventDefault();
      ev.stopPropagation();
      action();
    }
  },
  true
);

// Unsaved changes are not lost by accident.
window.addEventListener("beforeunload", (ev) => {
  if ([...models.values()].some((m) => m.belsk2Dirty)) ev.preventDefault();
});

// ---- start -----------------------------------------------------------------------------------

function renderRecent() {
  const recent = load("recent", []);
  $("#recent").replaceChildren(
    ...(recent.length
      ? recent.map((r) => el("li", {}, el("a", { href: "#", textContent: r, onclick: (ev) => (ev.preventDefault(), openFolder(r)) })))
      : [el("li", { className: "muted", textContent: "Nothing yet" })])
  );
}

renderRecent();
renderTree();
renderProblems();
updateStatus();
if (isDemo) openFolder("/demo");
