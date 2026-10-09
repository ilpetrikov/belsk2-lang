// Talks to the Rust side of the IDE. In a plain browser (`npm run dev`
// without Tauri) a small in-memory demo backend is used instead, so the UI
// can be worked on without building the app.

const inTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

let tauri = null;
if (inTauri) {
  const core = await import("@tauri-apps/api/core");
  const event = await import("@tauri-apps/api/event");
  const dialog = await import("@tauri-apps/plugin-dialog");
  tauri = { invoke: core.invoke, listen: event.listen, open: dialog.open };
}

export const isDemo = !inTauri;

export function call(cmd, args = {}) {
  return tauri ? tauri.invoke(cmd, args) : demo(cmd, args);
}

export function listen(name, handler) {
  if (tauri) return tauri.listen(name, (e) => handler(e.payload));
  (demoListeners[name] ||= []).push(handler);
  return Promise.resolve(() => {});
}

export async function pickFolder(title) {
  if (tauri) return tauri.open({ directory: true, title });
  return "/demo";
}

// ---- demo backend -------------------------------------------------------

const demoListeners = {};
const emit = (name, payload) => (demoListeners[name] || []).forEach((h) => h(payload));

const demoFiles = {
  "/demo/belsk2.toml": '[package]\nname = "demo"\nversion = "0.1.0"\n',
  "/demo/src/main.belsk2":
    'using util;\n\nfn main() {\n    var name = input("Your name: ");\n    prinb("Hello, " + name + "!");\n    prinb(twice(21));\n}\n',
  "/demo/src/util.belsk2": "fn twice(n: int): int {\n    return n * 2;\n}\n",
};

function demoTree() {
  const root = { children: [] };
  for (const path of Object.keys(demoFiles).sort()) {
    const parts = path.split("/").slice(2);
    let node = root;
    let acc = "/demo";
    parts.forEach((part, i) => {
      acc += "/" + part;
      let child = node.children.find((c) => c.name === part);
      if (!child) {
        child = { name: part, path: acc, dir: i < parts.length - 1, children: [] };
        node.children.push(child);
      }
      node = child;
    });
  }
  const sort = (n) => {
    n.children.sort((a, b) => b.dir - a.dir || a.name.localeCompare(b.name));
    n.children.forEach(sort);
  };
  sort(root);
  return root.children;
}

async function demo(cmd, args) {
  switch (cmd) {
    case "open_folder":
      return { root: "/demo", name: "demo", project: "demo", entries: demoTree() };
    case "read_file":
      return demoFiles[args.path] ?? "";
    case "write_file":
      demoFiles[args.path] = args.text;
      return null;
    case "create_file":
      demoFiles[args.path] = "";
      return args.path;
    case "create_dir":
    case "rename_path":
    case "new_project":
      return args.to || args.path || "/demo";
    case "analyze":
      return { diagnostics: [], hints: [], modules: [], root: "/demo" };
    case "complete":
      return ["prinb", "input", "len", "fn", "var", "while"].map((label) => ({ label, kind: "function" }));
    case "start":
      setTimeout(() => emit("run-output", { stream: "stdout", text: "(demo mode: build the app to run programs)\n" }), 50);
      setTimeout(() => emit("run-exit", 0), 100);
      return null;
    default:
      return null;
  }
}
