# Belsk2 for VS Code

Language support for [Belsk2](https://github.com/ilpetrikov/belsk2-lang):

- syntax highlighting and snippets (`main`, `fn`, `for`, ...)
- errors as you type, for single files and whole projects (`belsk2.toml`)
- hover types, completion, go to definition, find references, rename
- inline type and parameter hints
- **Run** (F5 or the ▶ button) and **Check** commands

## Requirements

The `belsk2` executable must be installed (the extension starts `belsk2 lsp`).
Build it from the repository with `cargo install --path cli`, or point the
`belsk2.path` setting at it.

## Building the extension

```bash
cd editors/vscode
npm install
npx vsce package
code --install-extension belsk2-0.4.0.vsix
```

## Settings

| Setting | Default | |
|---|---|---|
| `belsk2.path` | `belsk2` | Path to the belsk2 executable |
| `belsk2.inlayHints` | `true` | Show inferred types and parameter names |

Any other LSP-capable editor (Zed, Neovim, Helix, ...) can use the same
server: run `belsk2 lsp` for files with the `.belsk2` extension.
