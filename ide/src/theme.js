// Appearance: color themes, the "islands" layout, editor settings and the
// nesting-depth stripe in the gutter. Settings are kept in localStorage.

import { monaco } from "./editor.js";

export const THEMES = [
  { id: "belsk2", label: "Belsk2 Night", monaco: "belsk2-dark" },
  { id: "dark", label: "Dark", monaco: "belsk2-plain-dark" },
  { id: "light", label: "Light", monaco: "belsk2-light" },
  { id: "hc", label: "High Contrast", monaco: "belsk2-hc" },
];

const syntax = (c) => [
  { token: "keyword", foreground: c.keyword },
  { token: "type", foreground: c.type },
  { token: "predefined", foreground: c.fn, fontStyle: "italic" },
  { token: "function", foreground: c.fn },
  { token: "function.declaration", foreground: c.decl },
  { token: "constant", foreground: c.number },
  { token: "number", foreground: c.number },
  { token: "string", foreground: c.string },
  { token: "comment", foreground: c.comment, fontStyle: "italic" },
  { token: "operator", foreground: c.op },
];

monaco.editor.defineTheme("belsk2-plain-dark", {
  base: "vs-dark",
  inherit: true,
  rules: syntax({
    keyword: "569cd6", type: "4ec9b0", fn: "dcdcaa", decl: "dcdcaa", number: "b5cea8",
    string: "ce9178", comment: "6a9955", op: "d4d4d4",
  }),
  colors: {
    "editor.background": "#1e1e1e",
    "editor.lineHighlightBackground": "#2a2a2a",
    "editorInlayHint.foreground": "#9a9a9a",
    "editorInlayHint.background": "#2a2a2a",
  },
});
monaco.editor.defineTheme("belsk2-light", {
  base: "vs",
  inherit: true,
  rules: syntax({
    keyword: "7c3aed", type: "0f766e", fn: "2563eb", decl: "b45309", number: "c2410c",
    string: "15803d", comment: "8a8fa8", op: "475569",
  }),
  colors: {
    "editor.background": "#ffffff",
    "editor.lineHighlightBackground": "#f3f4fa",
    "editorLineNumber.foreground": "#b4b8cc",
    "editorInlayHint.foreground": "#7a7f99",
    "editorInlayHint.background": "#eef0f7",
  },
});
monaco.editor.defineTheme("belsk2-hc", {
  base: "hc-black",
  inherit: true,
  rules: syntax({
    keyword: "ff9cf7", type: "5ff5e1", fn: "8ec5ff", decl: "ffe066", number: "ffb86b",
    string: "a6ff8f", comment: "b0b0b0", op: "ffffff",
  }),
  colors: { "editor.background": "#000000" },
});

const DEFAULTS = {
  theme: "belsk2",
  islands: true,
  depthStripe: true,
  fontSize: 14,
  minimap: true,
  inlayHints: true,
};

function load() {
  try {
    return { ...DEFAULTS, ...JSON.parse(localStorage.getItem("settings") || "{}") };
  } catch {
    return { ...DEFAULTS };
  }
}

export const settings = load();
const listeners = [];

export function onSettingsChange(f) {
  listeners.push(f);
}

export function setSetting(key, value) {
  settings[key] = value;
  try {
    localStorage.setItem("settings", JSON.stringify(settings));
  } catch {}
  apply();
}

let editorRef = null;

/** Applies all settings to the page and the editor. */
export function apply() {
  const theme = THEMES.find((t) => t.id === settings.theme) || THEMES[0];
  document.documentElement.dataset.theme = theme.id;
  document.body.classList.toggle("islands", !!settings.islands);
  monaco.editor.setTheme(theme.monaco);
  if (editorRef) {
    editorRef.updateOptions({
      fontSize: Number(settings.fontSize) || 14,
      minimap: { enabled: !!settings.minimap },
      inlayHints: { enabled: settings.inlayHints ? "on" : "off" },
      lineDecorationsWidth: settings.depthStripe ? 14 : 6,
    });
    updateDepth(editorRef);
  }
  listeners.forEach((f) => f(settings));
}

// ---- nesting-depth stripe ------------------------------------------------------

/** Brace depth at the start of each line, skipping strings and comments. */
function depths(text) {
  const out = [];
  let depth = 0;
  let inBlock = false;
  for (const line of text.split("\n")) {
    let lineDepth = depth;
    let inString = null;
    for (let i = 0; i < line.length; i++) {
      const c = line[i];
      const next = line[i + 1];
      if (inBlock) {
        if (c === "*" && next === "/") {
          inBlock = false;
          i++;
        }
      } else if (inString) {
        if (c === "\\") i++;
        else if (c === inString) inString = null;
      } else if (c === "/" && next === "/") break;
      else if (c === "/" && next === "*") {
        inBlock = true;
        i++;
      } else if (c === '"' || c === "'") inString = c;
      else if (c === "{") depth++;
      else if (c === "}") {
        depth = Math.max(0, depth - 1);
        // A line that starts by closing a block belongs to the outer level.
        if (line.slice(0, i).trim() === "") lineDepth = depth;
      }
    }
    out.push(lineDepth);
  }
  return out;
}

const collections = new WeakMap();

export function updateDepth(editor) {
  const model = editor.getModel();
  if (!model) return;
  let collection = collections.get(editor);
  if (!collection) {
    collection = editor.createDecorationsCollection();
    collections.set(editor, collection);
  }
  if (!settings.depthStripe || model.getLanguageId() !== "belsk2") return collection.clear();
  const decorations = depths(model.getValue()).map((d, i) => ({
    range: new monaco.Range(i + 1, 1, i + 1, 1),
    options: { isWholeLine: true, linesDecorationsClassName: `depth depth-${Math.min(d, 6)}` },
  }));
  collection.set(decorations);
}

export function attachEditor(editor) {
  editorRef = editor;
  let timer = null;
  const schedule = () => {
    clearTimeout(timer);
    timer = setTimeout(() => updateDepth(editor), 150);
  };
  editor.onDidChangeModel(schedule);
  editor.onDidChangeModelContent(schedule);
  apply();
}
