import { defineConfig } from "vite";

// Tauri expects a fixed port and a build in `dist`.
export default defineConfig({
  clearScreen: false,
  server: { port: 1420, strictPort: true },
  build: { target: "es2022", outDir: "dist", chunkSizeWarningLimit: 6000 },
});
