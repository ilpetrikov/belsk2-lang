let editor = null;
let isRunning = false;

// --- File state ---
let currentFolder = null;
let openTabs = [];      // [{ filePath, name, content, modified }]
let activeTabIndex = -1;

const defaultCode = `// Welcome to belsk2 IDE!
// Press Ctrl+S to save, Ctrl+Enter to run

fn fib(n: bel): bel {
    if (n <= 1) {
        return n;
    }
    return fib(n - 1) + fib(n - 2);
}

prinb("belsk2 - fibonacci");

for i in [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10] {
    prinb(fib(i));
}
`;

const belsk2LangDef = {
  keywords: [
    'var', 'idb', 'bel', 'fn', 'if', 'else', 'while', 'for', 'in', 'return',
    'break', 'continue', 'true', 'false', 'null',
  ],
  typeKeywords: ['float', 'string', 'bool', 'any', 'bel', 'ster'],
  builtins: [
    'prinb', 'reab', 'input', 'len', 'str', 'num', 'int', 'float',
    'bool', 'push', 'pop', 'substr', 'type',
  ],
  operators: [
    '=', '==', '!=', '<', '>', '<=', '>=',
    '+', '-', '*', '/', '%',
    '+=', '-=', '&&', '||', '!',
    '->', ':',
  ],
  symbols: /[=><!~?:&|+\-*/^%]+/,
  tokenizer: {
    root: [
      [/\/\/.*$/, 'comment'],
      [/\/\*/, 'comment', '@comment'],
      [/"([^"\\]|\\.)*$/, 'string.invalid'],
      [/'([^'\\]|\\.)*$/, 'string.invalid'],
      [/"/, 'string', '@stringDouble'],
      [/'/, 'string', '@stringSingle'],
      [/\d+(\.\d+)?/, 'number'],
      [/[a-zA-Z_]\w*/, {
        cases: {
          '@keywords': 'keyword',
          '@typeKeywords': 'type',
          '@builtins': 'predefined',
          '@default': 'identifier',
        },
      }],
      [/[{}()[\]]/, '@brackets'],
      [/[;,.]/, 'delimiter'],
      [/[+\-*/%]=?/, 'operator'],
      [/==|!=|<=|>=|<|>/, 'operator'],
      [/\|\||&&|!/, 'operator'],
    ],
    stringDouble: [
      [/[^\\"]+/, 'string'],
      [/\\./, 'string.escape'],
      [/"/, 'string', '@pop'],
    ],
    stringSingle: [
      [/[^\\']+/, 'string'],
      [/\\./, 'string.escape'],
      [/'/, 'string', '@pop'],
    ],
    comment: [
      [/[^\/*]+/, 'comment'],
      [/\*\//, 'comment', '@pop'],
      [/[/*]/, 'comment'],
    ],
  },
};

// ===== Init =====

require.config({
  paths: { vs: 'https://cdn.jsdelivr.net/npm/monaco-editor@0.44.0/min/vs' },
});

require(['vs/editor/editor.main'], function () {
  monaco.languages.register({ id: 'belsk2' });
  monaco.languages.setMonarchTokensProvider('belsk2', belsk2LangDef);

  monaco.editor.defineTheme('belsk2-dark', {
    base: 'vs-dark',
    inherit: true,
    rules: [
      { token: 'comment', foreground: '6c7086', fontStyle: 'italic' },
      { token: 'keyword', foreground: 'cba6f7' },
      { token: 'type', foreground: 'f9e2af' },
      { token: 'predefined', foreground: 'cba6f7' },
      { token: 'string', foreground: 'a6e3a1' },
      { token: 'number', foreground: 'fab387' },
      { token: 'operator', foreground: '89dceb' },
      { token: 'identifier', foreground: 'cdd6f4' },
    ],
    colors: {
      'editor.background': '#1e1e2e',
      'editor.foreground': '#cdd6f4',
      'editorLineNumber.foreground': '#45475a',
      'editorLineNumber.activeForeground': '#89b4fa',
      'editor.lineHighlightBackground': '#252536',
      'editor.selectionBackground': '#45475a',
      'editorCursor.foreground': '#f5e0dc',
      'editorIndentGuide.background': '#313244',
      'editorIndentGuide.activeBackground': '#45475a',
    },
  });

  monaco.editor.defineTheme('belsk2-light', {
    base: 'vs',
    inherit: true,
    rules: [
      { token: 'comment', foreground: '9ca0b0', fontStyle: 'italic' },
      { token: 'keyword', foreground: '8839ef' },
      { token: 'type', foreground: 'df8e1d' },
      { token: 'predefined', foreground: '8839ef' },
      { token: 'string', foreground: '40a02b' },
      { token: 'number', foreground: 'fe640b' },
      { token: 'operator', foreground: '04a5e5' },
      { token: 'identifier', foreground: '4c4f69' },
    ],
    colors: {
      'editor.background': '#eff1f5',
      'editor.foreground': '#4c4f69',
      'editorLineNumber.foreground': '#bcc0cc',
      'editorLineNumber.activeForeground': '#1e66f5',
      'editor.lineHighlightBackground': '#e6e9ef',
      'editor.selectionBackground': '#dce0e8',
      'editorCursor.foreground': '#dc8a78',
      'editorIndentGuide.background': '#dce0e8',
      'editorIndentGuide.activeBackground': '#bcc0cc',
    },
  });

  monaco.editor.defineTheme('belsk2-yellow', {
    base: 'vs-dark',
    inherit: true,
    rules: [
      { token: 'comment', foreground: '92856e', fontStyle: 'italic' },
      { token: 'keyword', foreground: 'e879f9' },
      { token: 'type', foreground: 'facc15' },
      { token: 'predefined', foreground: 'e879f9' },
      { token: 'string', foreground: 'a3e635' },
      { token: 'number', foreground: 'fbbf24' },
      { token: 'operator', foreground: '38bdf8' },
      { token: 'identifier', foreground: 'fde68a' },
    ],
    colors: {
      'editor.background': '#1c1917',
      'editor.foreground': '#fde68a',
      'editorLineNumber.foreground': '#57534e',
      'editorLineNumber.activeForeground': '#60a5fa',
      'editor.lineHighlightBackground': '#292524',
      'editor.selectionBackground': '#44403c',
      'editorCursor.foreground': '#fbbf24',
      'editorIndentGuide.background': '#292524',
      'editorIndentGuide.activeBackground': '#44403c',
    },
  });

  const themeMap = {
    dark: 'belsk2-dark',
    light: 'belsk2-light',
    yellow: 'belsk2-yellow',
  };

  function applyTheme(name) {
    document.body.className = 'theme-' + name;
    if (editor) {
      monaco.editor.setTheme(themeMap[name] || 'belsk2-dark');
    }
    const sel = document.getElementById('theme-select');
    if (sel) sel.value = name;
    localStorage.setItem('belsk2-theme', name);
  }

  const savedTheme = localStorage.getItem('belsk2-theme') || 'dark';

  editor = monaco.editor.create(document.getElementById('editor'), {
    value: defaultCode,
    language: 'belsk2',
    theme: themeMap[savedTheme] || 'belsk2-dark',
    fontSize: 14,
    fontFamily: "'Cascadia Code', 'Fira Code', 'Consolas', monospace",
    minimap: { enabled: true },
    lineNumbers: 'on',
    roundedSelection: false,
    scrollBeyondLastLine: false,
    automaticLayout: true,
    tabSize: 4,
    renderWhitespace: 'selection',
    bracketPairColorization: { enabled: true },
    cursorBlinking: 'smooth',
    smoothScrolling: true,
  });

  // Track editor changes
  editor.onDidChangeModelContent(() => {
    if (activeTabIndex >= 0 && activeTabIndex < openTabs.length) {
      openTabs[activeTabIndex].modified = true;
      renderTabs();
    }
  });

  // Keyboard shortcuts
  editor.addCommand(monaco.KeyMod.CtrlCmd | monaco.KeyCode.KeyS, () => saveCurrentFile());
  editor.addCommand(monaco.KeyMod.CtrlCmd | monaco.KeyCode.Enter, () => runCode());
  editor.addCommand(monaco.KeyMod.CtrlCmd | monaco.KeyCode.Period, () => stopCode());

  // Button handlers
  document.getElementById('btn-run').addEventListener('click', runCode);
  document.getElementById('btn-stop').addEventListener('click', stopCode);
  document.getElementById('btn-clear').addEventListener('click', clearConsole);
  document.getElementById('btn-save').addEventListener('click', saveCurrentFile);
  document.getElementById('btn-open-folder').addEventListener('click', openFolder);
  document.getElementById('btn-open-file').addEventListener('click', openFile);

  document.getElementById('theme-select').addEventListener('change', (e) => {
    applyTheme(e.target.value);
  });

  const consoleInput = document.getElementById('console-input');
  consoleInput.addEventListener('keydown', (e) => {
    if (e.key === 'Enter') {
      e.preventDefault();
      sendConsoleInput();
    }
  });

  applyTheme(savedTheme);

  initSplitters();

  // Default tab
  openTabs.push({ filePath: null, name: 'untitled.belsk2', content: defaultCode, modified: false });
  activeTabIndex = 0;
  renderTabs();
});

// ===== Tabs =====

function renderTabs() {
  const tabsEl = document.getElementById('tabs');
  tabsEl.innerHTML = '';
  openTabs.forEach((tab, i) => {
    const div = document.createElement('div');
    div.className = 'tab' + (i === activeTabIndex ? ' active' : '');

    const name = document.createElement('span');
    name.textContent = tab.name;
    div.appendChild(name);

    if (tab.modified) {
      const dot = document.createElement('span');
      dot.className = 'dirty';
      dot.textContent = '*';
      div.appendChild(dot);
    }

    const close = document.createElement('span');
    close.className = 'close';
    close.textContent = '\u00D7';
    close.addEventListener('click', (e) => {
      e.stopPropagation();
      closeTab(i);
    });
    div.appendChild(close);

    div.addEventListener('click', () => switchTab(i));
    tabsEl.appendChild(div);
  });
}

function switchTab(index) {
  if (index === activeTabIndex) return;
  if (index < 0 || index >= openTabs.length) return;

  // Save current editor content to old tab
  if (activeTabIndex >= 0 && activeTabIndex < openTabs.length) {
    openTabs[activeTabIndex].content = editor.getValue();
  }

  activeTabIndex = index;
  const tab = openTabs[index];
  editor.setValue(tab.content);
  const model = editor.getModel();
  if (tab.filePath && tab.filePath.endsWith('.belsk2')) {
    monaco.editor.setModelLanguage(model, 'belsk2');
  } else {
    monaco.editor.setModelLanguage(model, 'plaintext');
  }
  renderTabs();
}

function closeTab(index) {
  openTabs.splice(index, 1);
  if (openTabs.length === 0) {
    openTabs.push({ filePath: null, name: 'untitled.belsk2', content: '', modified: false });
    activeTabIndex = 0;
    editor.setValue('');
  } else if (activeTabIndex >= openTabs.length) {
    activeTabIndex = openTabs.length - 1;
  } else if (activeTabIndex > index) {
    activeTabIndex--;
  }
  if (activeTabIndex >= 0 && activeTabIndex < openTabs.length) {
    editor.setValue(openTabs[activeTabIndex].content);
  }
  renderTabs();
}

function openNewTab(filePath, name, content) {
  // Check if already open
  const existing = openTabs.findIndex(t => t.filePath === filePath);
  if (existing >= 0) {
    switchTab(existing);
    return;
  }
  openTabs.push({ filePath, name, content, modified: false });
  switchTab(openTabs.length - 1);
}

// ===== File operations =====

async function saveCurrentFile() {
  if (!editor) return;
  const tab = openTabs[activeTabIndex];
  if (!tab) return;

  const content = editor.getValue();
  const result = await window.belsk2.saveFile(tab.filePath, content);
  if (result) {
    tab.filePath = result;
    tab.name = result.split(/[/\\]/).pop();
    tab.content = content;
    tab.modified = false;
    renderTabs();
    setStatus('Saved: ' + tab.name, 'ok');
  }
}

async function openFile() {
  const result = await window.belsk2.openFile();
  if (result) {
    openNewTab(result.filePath, result.filePath.split(/[/\\]/).pop(), result.content);
  }
}

async function openFolder() {
  const folder = await window.belsk2.openFolder();
  if (folder) {
    currentFolder = folder;
    document.getElementById('sidebar-title').textContent = folder.split(/[/\\]/).pop();
    renderFileTree(folder);
  }
}

// ===== File tree =====

async function renderFileTree(dirPath) {
  const container = document.getElementById('file-tree');
  container.innerHTML = '';
  await buildTree(container, dirPath, 0);
}

async function buildTree(container, dirPath, depth) {
  const entries = await window.belsk2.listDirectory(dirPath);
  for (const entry of entries) {
    const item = document.createElement('div');
    item.className = 'tree-item ' + (entry.isDirectory ? 'tree-folder' : 'tree-file');
    item.style.paddingLeft = (12 + depth * 16) + 'px';

    const icon = document.createElement('span');
    icon.className = 'icon';
    icon.textContent = entry.isDirectory ? '\uD83D\uDCC1' : getFileIcon(entry.name);

    const name = document.createElement('span');
    name.className = 'name';
    name.textContent = entry.name;

    item.appendChild(icon);
    item.appendChild(name);
    container.appendChild(item);

    if (entry.isDirectory) {
      let expanded = false;
      let childContainer = null;

      item.addEventListener('click', async () => {
        if (!expanded) {
          childContainer = document.createElement('div');
          childContainer.className = 'tree-children';
          item.after(childContainer);
          await buildTree(childContainer, entry.path, depth + 1);
          icon.textContent = '\uD83D\uDCC0';
          expanded = true;
        } else {
          childContainer.remove();
          childContainer = null;
          icon.textContent = '\uD83D\uDCC1';
          expanded = false;
        }
      });
    } else {
      item.addEventListener('click', async () => {
        const result = await window.belsk2.readFile(entry.path);
        if (result.content !== null) {
          openNewTab(entry.path, entry.name, result.content);
        }
      });
    }
  }
}

function getFileIcon(name) {
  if (name.endsWith('.belsk2')) return '\u{1F4DC}';
  if (name.endsWith('.js') || name.endsWith('.ts')) return '\u{1F4D6}';
  if (name.endsWith('.json')) return '\u{1F4CB}';
  if (name.endsWith('.md')) return '\u{270D}';
  if (name.endsWith('.go')) return '\u{1F40D}';
  if (name.endsWith('.html') || name.endsWith('.css')) return '\u{1F3A8}';
  return '\u{1F4C4}';
}

// ===== Console =====

function setStatus(text, cls) {
  const el = document.getElementById('status');
  el.textContent = text;
  el.className = 'status ' + (cls || '');
}

function clearConsole() {
  document.getElementById('console-output').innerHTML = '';
}

function appendConsole(text, cls) {
  const el = document.getElementById('console-output');
  const line = document.createElement('div');
  line.className = 'console-line ' + (cls || 'stdout');
  line.textContent = text;
  el.appendChild(line);
  el.scrollTop = el.scrollHeight;
}

// ===== Run / Stop =====

function setInputEnabled(enabled) {
  const input = document.getElementById('console-input');
  if (!input) return;
  input.disabled = !enabled;
  if (enabled) input.focus();
}

function sendConsoleInput() {
  const input = document.getElementById('console-input');
  const value = input.value;
  if (!value) return;
  appendConsole('> ' + value, 'info');
  window.belsk2.sendInput(value);
  input.value = '';
  input.focus();
}

async function runCode() {
  if (isRunning || !editor) return;
  isRunning = true;
  const code = editor.getValue();
  clearConsole();
  setStatus('Running...', 'running');
  document.getElementById('btn-run').disabled = true;
  document.getElementById('btn-stop').disabled = false;
  setInputEnabled(true);

  window.belsk2.removeListeners();

  window.belsk2.onOutputChunk((data) => {
    for (const line of data.replace(/\r?\n$/, '').split('\n')) {
      if (line.trim() !== '') appendConsole(line, 'stdout');
    }
  });

  window.belsk2.onErrorChunk((data) => {
    for (const line of data.replace(/\r?\n$/, '').split('\n')) {
      if (line.trim() !== '') appendConsole(line, 'stderr');
    }
  });

  window.belsk2.onProcessExited((code) => {
    isRunning = false;
    setInputEnabled(false);
    document.getElementById('btn-run').disabled = false;
    document.getElementById('btn-stop').disabled = true;
    setStatus(code === 0 ? 'Done' : code !== null ? 'Error (exit ' + code + ')' : 'Stopped', code === 0 ? 'ok' : 'error');
  });

  try {
    await window.belsk2.runCode(code);
  } catch (err) {
    appendConsole('Error: ' + err, 'stderr');
    setStatus('Error', 'error');
    isRunning = false;
    setInputEnabled(false);
    document.getElementById('btn-run').disabled = false;
    document.getElementById('btn-stop').disabled = true;
  }
}

function stopCode() {
  if (!isRunning) return;
  window.belsk2.stopCode();
  setStatus('Stopped', 'error');
  isRunning = false;
  setInputEnabled(false);
  document.getElementById('btn-run').disabled = false;
  document.getElementById('btn-stop').disabled = true;
}

// ===== Splitters =====

function initSplitters() {
  // Sidebar splitter
  const sbSplitter = document.getElementById('sidebar-splitter');
  const sidebar = document.getElementById('sidebar');
  let draggingSidebar = false;

  sbSplitter.addEventListener('mousedown', (e) => { draggingSidebar = true; e.preventDefault(); });
  document.addEventListener('mousemove', (e) => {
    if (!draggingSidebar) return;
    const w = e.clientX;
    if (w > 120 && w < 500) sidebar.style.width = w + 'px';
  });
  document.addEventListener('mouseup', () => { draggingSidebar = false; });

  // Console splitter
  const splitter = document.getElementById('splitter');
  const consoleEl = document.getElementById('console-container');
  let draggingConsole = false;

  splitter.addEventListener('mousedown', (e) => { draggingConsole = true; e.preventDefault(); });
  document.addEventListener('mousemove', (e) => {
    if (!draggingConsole) return;
    const editorArea = document.getElementById('editor-area');
    const rect = editorArea.getBoundingClientRect();
    const newHeight = rect.bottom - e.clientY;
    if (newHeight > 60 && newHeight < rect.height - 200) {
      consoleEl.style.height = newHeight + 'px';
    }
  });
  document.addEventListener('mouseup', () => { draggingConsole = false; });
}
