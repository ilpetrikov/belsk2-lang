const { app, BrowserWindow, ipcMain, dialog } = require('electron');
const path = require('path');
const fs = require('fs');
const os = require('os');
const { spawn } = require('child_process');

let mainWindow;
let runProcess = null;

// The `belsk2` CLI: a bundled copy in ide/bin, otherwise the cargo build output.
function getCliPath() {
  const exe = process.platform === 'win32' ? 'belsk2.exe' : 'belsk2';
  const candidates = [
    path.join(__dirname, 'bin', exe),
    path.join(__dirname, '..', 'target', 'release', exe),
    path.join(__dirname, '..', 'target', 'debug', exe),
  ];
  return candidates.find((p) => fs.existsSync(p)) || candidates[0];
}

function createWindow() {
  mainWindow = new BrowserWindow({
    width: 1100,
    height: 750,
    minWidth: 700,
    minHeight: 450,
    title: 'belsk2 IDE',
    backgroundColor: '#1e1e2e',
    webPreferences: {
      preload: path.join(__dirname, 'preload.js'),
      contextIsolation: true,
      nodeIntegration: false,
    },
  });

  mainWindow.loadFile(path.join(__dirname, 'renderer', 'index.html'));

  mainWindow.on('closed', () => {
    stopProcess();
    mainWindow = null;
  });
}

function stopProcess() {
  if (runProcess) {
    runProcess.kill();
    runProcess = null;
  }
}

// --- File operations ---

ipcMain.handle('open-folder', async () => {
  const result = await dialog.showOpenDialog(mainWindow, {
    properties: ['openDirectory'],
  });
  if (result.canceled || result.filePaths.length === 0) return null;
  return result.filePaths[0];
});

ipcMain.handle('open-file', async () => {
  const result = await dialog.showOpenDialog(mainWindow, {
    properties: ['openFile'],
    filters: [
      { name: 'belsk2', extensions: ['belsk2'] },
      { name: 'All Files', extensions: ['*'] },
    ],
  });
  if (result.canceled || result.filePaths.length === 0) return null;
  const filePath = result.filePaths[0];
  const content = fs.readFileSync(filePath, 'utf-8');
  return { filePath, content };
});

ipcMain.handle('save-file', async (event, filePath, content) => {
  if (!filePath) {
    const result = await dialog.showSaveDialog(mainWindow, {
      filters: [
        { name: 'belsk2', extensions: ['belsk2'] },
        { name: 'All Files', extensions: ['*'] },
      ],
    });
    if (result.canceled) return null;
    filePath = result.filePath;
  }
  fs.writeFileSync(filePath, content, 'utf-8');
  return filePath;
});

ipcMain.handle('read-file', async (event, filePath) => {
  try {
    const content = fs.readFileSync(filePath, 'utf-8');
    return { content, error: null };
  } catch (err) {
    return { content: null, error: err.message };
  }
});

ipcMain.handle('list-directory', async (event, dirPath) => {
  try {
    const entries = fs.readdirSync(dirPath, { withFileTypes: true });
    const items = [];
    for (const entry of entries) {
      if (entry.name.startsWith('.')) continue;
      const fullPath = path.join(dirPath, entry.name);
      items.push({
        name: entry.name,
        path: fullPath,
        isDirectory: entry.isDirectory(),
      });
    }
    items.sort((a, b) => {
      if (a.isDirectory !== b.isDirectory) return a.isDirectory ? -1 : 1;
      return a.name.localeCompare(b.name);
    });
    return items;
  } catch (err) {
    return [];
  }
});

// --- Run/Stop code ---

ipcMain.handle('run-code', async (event, code) => {
  return new Promise((resolve) => {
    stopProcess();

    const binaryPath = getCliPath();

    if (!fs.existsSync(binaryPath)) {
      resolve({ output: '', error: `belsk2 CLI not found: ${binaryPath}\nRun: cargo build --release -p belsk2-cli` });
      return;
    }

    const tmpFile = path.join(os.tmpdir(), `belsk2_${Date.now()}.belsk2`);
    try {
      fs.writeFileSync(tmpFile, code, 'utf-8');
    } catch (err) {
      resolve({ output: '', error: 'Failed to write temp file: ' + err.message, exitCode: -1 });
      return;
    }

    // Run from a temp file so stdin stays open for `reab`/`input`.
    runProcess = spawn(binaryPath, [tmpFile], {
      stdio: ['pipe', 'pipe', 'pipe'],
    });

    let stdout = '';
    let stderr = '';

    runProcess.stdout.on('data', (data) => {
      stdout += data.toString();
      if (mainWindow && !mainWindow.isDestroyed()) {
        mainWindow.webContents.send('output-chunk', data.toString());
      }
    });

    runProcess.stderr.on('data', (data) => {
      stderr += data.toString();
      if (mainWindow && !mainWindow.isDestroyed()) {
        mainWindow.webContents.send('error-chunk', data.toString());
      }
    });

    runProcess.on('close', (code) => {
      runProcess = null;
      try { fs.unlinkSync(tmpFile); } catch (e) { /* ignore */ }
      if (mainWindow && !mainWindow.isDestroyed()) {
        mainWindow.webContents.send('process-exited', code);
      }
      resolve({ output: stdout, error: stderr, exitCode: code });
    });

    runProcess.on('error', (err) => {
      runProcess = null;
      try { fs.unlinkSync(tmpFile); } catch (e) { /* ignore */ }
      if (mainWindow && !mainWindow.isDestroyed()) {
        mainWindow.webContents.send('error-chunk', err.message);
        mainWindow.webContents.send('process-exited', -1);
      }
      resolve({ output: '', error: err.message, exitCode: -1 });
    });
  });
});

ipcMain.on('send-input', (event, line) => {
  if (runProcess && runProcess.stdin && runProcess.stdin.writable) {
    runProcess.stdin.write(line + '\n');
  }
});

ipcMain.handle('stop-code', async () => {
  stopProcess();
  return { stopped: true };
});

app.whenReady().then(createWindow);

app.on('window-all-closed', () => {
  stopProcess();
  app.quit();
});

app.on('activate', () => {
  if (BrowserWindow.getAllWindows().length === 0) {
    createWindow();
  }
});
