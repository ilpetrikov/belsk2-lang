const { contextBridge, ipcRenderer } = require('electron');

const CHANNELS = ['output-chunk', 'error-chunk', 'process-exited'];

contextBridge.exposeInMainWorld('belsk2', {
  runCode: (code) => ipcRenderer.invoke('run-code', code),
  stopCode: () => ipcRenderer.invoke('stop-code'),
  sendInput: (line) => ipcRenderer.send('send-input', line),
  removeListeners: () => CHANNELS.forEach(ch => ipcRenderer.removeAllListeners(ch)),
  onOutputChunk: (callback) => ipcRenderer.on('output-chunk', (event, data) => callback(data)),
  onErrorChunk: (callback) => ipcRenderer.on('error-chunk', (event, data) => callback(data)),
  onProcessExited: (callback) => ipcRenderer.on('process-exited', (event, code) => callback(code)),

  openFolder: () => ipcRenderer.invoke('open-folder'),
  openFile: () => ipcRenderer.invoke('open-file'),
  saveFile: (filePath, content) => ipcRenderer.invoke('save-file', filePath, content),
  readFile: (filePath) => ipcRenderer.invoke('read-file', filePath),
  listDirectory: (dirPath) => ipcRenderer.invoke('list-directory', dirPath),
});
