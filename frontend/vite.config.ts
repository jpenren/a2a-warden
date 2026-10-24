import { defineConfig } from "vite";

// The console is embedded in the warden binary, so the build output must be
// self contained and reference its assets relatively (no CDN, no absolute
// paths that would break behind the /ui prefix).
export default defineConfig({
  base: "./",
  build: {
    outDir: "dist",
    emptyOutDir: true,
    assetsDir: "assets",
    target: "es2020",
  },
  server: {
    port: 5173,
    proxy: {
      "/api": "http://127.0.0.1:8080",
    },
  },
});
