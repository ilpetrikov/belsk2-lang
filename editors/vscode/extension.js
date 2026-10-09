// Belsk2 for VS Code: starts `belsk2 lsp` and adds Run / Check commands.
const path = require("path");
const fs = require("fs");
const vscode = require("vscode");
const { LanguageClient } = require("vscode-languageclient/node");

let client;

function exe() {
  return vscode.workspace.getConfiguration("belsk2").get("path") || "belsk2";
}

async function startServer() {
  client = new LanguageClient(
    "belsk2",
    "Belsk2",
    { command: exe(), args: ["lsp"] },
    {
      documentSelector: [{ scheme: "file", language: "belsk2" }],
      middleware: {
        provideInlayHints: (doc, range, token, next) =>
          vscode.workspace.getConfiguration("belsk2").get("inlayHints")
            ? next(doc, range, token)
            : [],
      },
    }
  );
  try {
    await client.start();
  } catch (e) {
    vscode.window.showErrorMessage(
      `Belsk2: could not start "${exe()} lsp". Install belsk2 or set "belsk2.path". (${e.message || e})`
    );
  }
}

/** The project root (directory with belsk2.toml) above `file`, if any. */
function projectRoot(file) {
  let dir = path.dirname(file);
  for (;;) {
    if (fs.existsSync(path.join(dir, "belsk2.toml"))) return dir;
    const parent = path.dirname(dir);
    if (parent === dir) return undefined;
    dir = parent;
  }
}

function quote(s) {
  return /[\s"']/.test(s) ? `"${s.replace(/"/g, '\\"')}"` : s;
}

/** Runs `belsk2 <sub>` for the current file or its project in a terminal. */
async function inTerminal(sub) {
  const editor = vscode.window.activeTextEditor;
  if (!editor || editor.document.languageId !== "belsk2") {
    vscode.window.showWarningMessage("Belsk2: open a .belsk2 file first.");
    return;
  }
  await editor.document.save();
  const file = editor.document.fileName;
  const root = projectRoot(file);
  let term = vscode.window.terminals.find((t) => t.name === "Belsk2");
  if (!term) term = vscode.window.createTerminal({ name: "Belsk2", cwd: root });
  term.show(true);
  term.sendText(
    root
      ? `cd ${quote(root)}; ${quote(exe())} ${sub}`
      : `${quote(exe())} ${sub} ${quote(file)}`
  );
}

async function activate(context) {
  context.subscriptions.push(
    vscode.commands.registerCommand("belsk2.run", () => inTerminal("run")),
    vscode.commands.registerCommand("belsk2.check", () => inTerminal("check")),
    vscode.commands.registerCommand("belsk2.restartServer", async () => {
      if (client) await client.stop();
      await startServer();
    }),
    vscode.workspace.onDidChangeConfiguration(async (e) => {
      if (e.affectsConfiguration("belsk2.path")) {
        if (client) await client.stop();
        await startServer();
      }
    })
  );
  await startServer();
}

function deactivate() {
  return client ? client.stop() : undefined;
}

module.exports = { activate, deactivate };
