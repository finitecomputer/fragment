import { defineConfig } from "vite";
import solid from "@solidjs/vite-plugin";
import { resolve } from "node:path";

const repo =
  process.env.FRAGMENT_BASELINE_ROOT ?? resolve(import.meta.dirname, "../..");
export default defineConfig({
  plugins: [solid()],
  resolve: {
    alias: {
      "./agent-color.js": resolve(repo, "templates/agent/site/agent-color.js"),
    },
  },
  server: {
    port: 5180,
    host: "127.0.0.1",
    proxy: {
      "/api": {
        target: "http://127.0.0.1:8790",
        changeOrigin: true,
        ws: true,
        configure(proxy) {
          // A local-only engineering preview. The Worker still receives its
          // configured platform origin; no production proxy is introduced.
          proxy.on("proxyReq", (request) =>
            request.setHeader("origin", "http://127.0.0.1:8790"),
          );
          proxy.on("proxyReqWs", (request) =>
            request.setHeader("origin", "http://127.0.0.1:8790"),
          );
        },
      },
      "/__shell": "http://127.0.0.1:8790",
      "/auth": "http://127.0.0.1:8790",
      "/share": "http://127.0.0.1:8790",
    },
  },
  preview: { port: 5180, host: "127.0.0.1" },
  build: {
    manifest: true,
    target: "es2022",
    rollupOptions: {
      input: {
        solid: resolve(import.meta.dirname, "index.html"),
        benchmark: resolve(import.meta.dirname, "benchmark.html"),
        vanillaApps: resolve(repo, "cell/shell/apps-page.js"),
        vanillaShell: resolve(repo, "cell/shell/shell.js"),
      },
    },
  },
});
