// Monaco setup: the Belsk2 language, the theme and the code intelligence
// providers backed by `belsk2-analysis` on the Rust side.

import * as monaco from "monaco-editor";
import EditorWorker from "monaco-editor/esm/vs/editor/editor.worker?worker";
import { call } from "./backend.js";

self.MonacoEnvironment = { getWorker: () => new EditorWorker() };

export { monaco };

const LANG = "belsk2";

monaco.languages.register({ id: LANG, extensions: [".belsk2"], aliases: ["Belsk2"] });
monaco.languages.setLanguageConfiguration(LANG, {
  comments: { lineComment: "//", blockComment: ["/*", "*/"] },
  brackets: [["{", "}"], ["[", "]"], ["(", ")"]],
  autoClosingPairs: [
    { open: "{", close: "}" },
    { open: "[", close: "]" },
    { open: "(", close: ")" },
    { open: '"', close: '"', notIn: ["string"] },
    { open: "'", close: "'", notIn: ["string"] },
  ],
  surroundingPairs: [{ open: "{", close: "}" }, { open: "(", close: ")" }, { open: '"', close: '"' }],
});
monaco.languages.setMonarchTokensProvider(LANG, {
  keywords: ["using", "var", "fn", "if", "else", "while", "for", "in", "return", "break", "continue", "idb"],
  constants: ["true", "false", "null"],
  types: ["any", "bool", "char", "string", "sbyte", "byte", "short", "ushort", "int", "uint", "long", "ulong", "float", "double", "bel", "ster"],
  builtins: ["prinb", "reab", "input", "len", "str", "num", "push", "pop", "substr", "type", "has", "remove", "keys", "values"],
  tokenizer: {
    root: [
      [/\/\/.*$/, "comment"],
      [/\/\*/, "comment", "@comment"],
      [/"([^"\\]|\\.)*"/, "string"],
      [/'([^'\\]|\\.)*'/, "string"],
      [/0[xX][0-9a-fA-F_]+[uUlL]*/, "number.hex"],
      [/0[bB][01_]+[uUlL]*/, "number"],
      [/\d[\d_]*(\.\d[\d_]*)?([eE][+-]?\d+)?[fFdDmMuUlL]*/, "number"],
      [/(fn)(\s+)([A-Za-z_]\w*)/, ["keyword", "", "function.declaration"]],
      [
        /[A-Za-z_]\w*(?=\s*\()/,
        { cases: { "@builtins": "predefined", "@types": "type", "@keywords": "keyword", "@default": "function" } },
      ],
      [
        /[A-Za-z_]\w*/,
        { cases: { "@keywords": "keyword", "@constants": "constant", "@types": "type", "@default": "identifier" } },
      ],
      [/[{}()[\]]/, "@brackets"],
      [/[=<>!+\-*/%&|^~]+/, "operator"],
    ],
    comment: [
      [/\*\//, "comment", "@pop"],
      [/./, "comment"],
    ],
  },
});

monaco.editor.defineTheme("belsk2-dark", {
  base: "vs-dark",
  inherit: true,
  rules: [
    { token: "keyword", foreground: "c792ea" },
    { token: "type", foreground: "4fd6be" },
    { token: "predefined", foreground: "82aaff", fontStyle: "italic" },
    { token: "function", foreground: "82aaff" },
    { token: "function.declaration", foreground: "ffc777" },
    { token: "constant", foreground: "ff966c" },
    { token: "number", foreground: "ff966c" },
    { token: "string", foreground: "c3e88d" },
    { token: "comment", foreground: "636da6", fontStyle: "italic" },
    { token: "operator", foreground: "89ddff" },
  ],
  colors: {
    "editor.background": "#1b1d2b",
    "editor.lineHighlightBackground": "#24273a",
    "editorLineNumber.foreground": "#3b4261",
    "editorLineNumber.activeForeground": "#a9b1d6",
    "editorInlayHint.foreground": "#7a83b5",
    "editorInlayHint.background": "#24273a",
  },
});

// ---- state shared with the workbench ------------------------------------

/** path -> model */
export const models = new Map();
let version = 0;
/** Last analysis report per path (for inlay hints). */
const hintsByPath = new Map();
const hintsChanged = new monaco.Emitter();

/** Set by the workbench: loads a file into a model (without a tab). */
export const hooks = { ensureModel: async () => null, open: () => {} };

export function pathOf(model) {
  for (const [path, m] of models) if (m === model) return path;
  return null;
}

export function bumpVersion() {
  version++;
}

/** What every request sends: the file, a version and unsaved texts. */
function doc(path) {
  const overlays = {};
  for (const [p, m] of models) if (m.belsk2Dirty) overlays[p] = m.getValue();
  return { path, version, overlays };
}

// The analysis counts columns in characters, Monaco in UTF-16 code units;
// they differ on lines with emoji and other characters outside the BMP.

/** Monaco column for character column `col` on `line`. */
export function toColumn(model, line, col) {
  if (!model || line < 1 || line > model.getLineCount()) return col;
  let units = 0;
  let chars = 1;
  for (const ch of model.getLineContent(line)) {
    if (chars >= col) break;
    units += ch.length;
    chars++;
  }
  return units + 1 + Math.max(0, col - chars);
}

/** Character column for Monaco column `column` on `line`. */
function fromColumn(model, line, column) {
  return [...model.getLineContent(line).slice(0, column - 1)].length + 1;
}

/** A Monaco range for a character range from the analysis. */
export function toRange(model, r) {
  return new monaco.Range(
    r.start.line,
    toColumn(model, r.start.line, r.start.col),
    r.end.line,
    toColumn(model, r.end.line, r.end.col)
  );
}
const pos = (model, p) => ({ line: p.lineNumber, col: fromColumn(model, p.lineNumber, p.column) });

/** Analyzes `path`; returns the report and updates markers and hints. */
export async function analyze(path) {
  const report = await call("analyze", { doc: doc(path) });
  const byPath = new Map();
  for (const d of report.diagnostics) {
    const key = d.path ?? path;
    if (!byPath.has(key)) byPath.set(key, []);
    byPath.get(key).push(d);
  }
  for (const [p, model] of models) {
    const list = byPath.get(p) || [];
    monaco.editor.setModelMarkers(
      model,
      "belsk2",
      list.map((d) => {
        const line = Math.max(1, d.range.start.line);
        const endLine = Math.max(1, d.range.end.line);
        return {
          severity: monaco.MarkerSeverity.Error,
          message: d.message,
          startLineNumber: line,
          startColumn: toColumn(model, line, Math.max(1, d.range.start.col)),
          endLineNumber: endLine,
          endColumn: toColumn(model, endLine, Math.max(1, d.range.end.col)),
          source: "belsk2",
        };
      })
    );
  }
  hintsByPath.set(path, report.hints);
  hintsChanged.fire();
  return report;
}

// ---- providers ------------------------------------------------------------

monaco.languages.registerHoverProvider(LANG, {
  async provideHover(model, position) {
    const h = await call("hover", { doc: doc(pathOf(model)), pos: pos(model, position) });
    return h && { range: toRange(model, h.range), contents: [{ value: h.text }] };
  },
});

const KINDS = {
  function: monaco.languages.CompletionItemKind.Function,
  variable: monaco.languages.CompletionItemKind.Variable,
  parameter: monaco.languages.CompletionItemKind.Variable,
  keyword: monaco.languages.CompletionItemKind.Keyword,
  type: monaco.languages.CompletionItemKind.Class,
  module: monaco.languages.CompletionItemKind.Module,
  text: monaco.languages.CompletionItemKind.Text,
};
const ORDER = { variable: 0, parameter: 0, function: 1, module: 1, keyword: 2, type: 2, text: 3 };

monaco.languages.registerCompletionItemProvider(LANG, {
  async provideCompletionItems(model, position) {
    const items = await call("complete", { doc: doc(pathOf(model)), pos: pos(model, position) });
    const word = model.getWordUntilPosition(position);
    const range = new monaco.Range(position.lineNumber, word.startColumn, position.lineNumber, word.endColumn);
    return {
      suggestions: items.map((c) => ({
        label: c.label,
        kind: KINDS[c.kind] ?? KINDS.text,
        detail: c.detail ?? undefined,
        documentation: c.doc ? { value: c.doc } : undefined,
        insertText: c.label,
        sortText: `${ORDER[c.kind] ?? 3}${c.label}`,
        range,
      })),
    };
  },
});

async function locations(list) {
  const out = [];
  for (const l of list) {
    const model = models.get(l.path) || (await hooks.ensureModel(l.path));
    if (model) out.push({ uri: model.uri, range: toRange(model, l.range) });
  }
  return out;
}

monaco.languages.registerDefinitionProvider(LANG, {
  async provideDefinition(model, position) {
    const l = await call("definition", { doc: doc(pathOf(model)), pos: pos(model, position) });
    return l ? locations([l]) : [];
  },
});

monaco.languages.registerReferenceProvider(LANG, {
  async provideReferences(model, position) {
    return locations(await call("references", { doc: doc(pathOf(model)), pos: pos(model, position) }));
  },
});

monaco.languages.registerRenameProvider(LANG, {
  async provideRenameEdits(model, position, newName) {
    try {
      const places = await call("rename", { doc: doc(pathOf(model)), pos: pos(model, position) });
      const locs = await locations(places);
      return {
        edits: locs.map((l) => ({
          resource: l.uri,
          versionId: undefined,
          textEdit: { range: l.range, text: newName },
        })),
      };
    } catch (e) {
      return { edits: [], rejectReason: String(e) };
    }
  },
});

monaco.languages.registerInlayHintsProvider(LANG, {
  onDidChangeInlayHints: hintsChanged.event,
  provideInlayHints(model) {
    const hints = hintsByPath.get(pathOf(model)) || [];
    return {
      hints: hints.map((h) => ({
        position: { lineNumber: h.pos.line, column: toColumn(model, h.pos.line, h.pos.col) },
        label: h.label,
        kind: h.kind === "type" ? monaco.languages.InlayHintKind.Type : monaco.languages.InlayHintKind.Parameter,
        paddingRight: h.kind === "parameter",
      })),
      dispose() {},
    };
  },
});

// Go to definition in another file opens it in a tab.
monaco.editor.registerEditorOpener({
  openCodeEditor(_source, resource, selection) {
    for (const [path, m] of models) {
      if (m.uri.toString() === resource.toString()) {
        hooks.open(path, selection);
        return true;
      }
    }
    return false;
  },
});

export function createEditor(element) {
  return monaco.editor.create(element, {
    theme: "belsk2-dark",
    automaticLayout: true,
    fontFamily: "'JetBrains Mono', 'Cascadia Code', Consolas, monospace",
    fontSize: 14,
    fontLigatures: true,
    minimap: { enabled: true, scale: 1 },
    smoothScrolling: true,
    cursorSmoothCaretAnimation: "on",
    bracketPairColorization: { enabled: true },
    guides: { bracketPairs: "active", indentation: true },
    inlayHints: { enabled: "on" },
    renderLineHighlight: "all",
    padding: { top: 8 },
    tabSize: 4,
    model: null,
  });
}

export function languageFor(path) {
  if (path.endsWith(".belsk2")) return LANG;
  if (path.endsWith(".toml")) return "ini";
  if (path.endsWith(".md")) return "markdown";
  if (path.endsWith(".json")) return "json";
  return "plaintext";
}
