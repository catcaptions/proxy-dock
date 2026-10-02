import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";
import { proxyDockDevOAuth } from "./dev-oauth";

export default defineConfig({
  build: { target: "es2021" },
  clearScreen: false,
  plugins: [react(), proxyDockDevOAuth()],
  server: {
    // IPv4 loopback explicitly: the default `localhost` bind lands on ::1
    // only, but the Command Code Studio callback is sent (and fetched) as
    // http://127.0.0.1:{port}/callback — unreachable on a ::1-only socket.
    host: "127.0.0.1",
    port: 1420,
    strictPort: true,
    // Dev-only CORS bypass so the browser preview can actually test an
    // OpenCode Go key. opencode.ai has no CORS headers, so a direct fetch
    // from the page is blocked. The desktop (Tauri) app verifies via Rust
    // with no CORS involved; this proxy is only for `npm run dev`.
    proxy: {
      // Same-origin gate decision (§0): the preview reaches the Control API
      // through this proxy, so no CORS opt-in is needed on the Axum side.
      "/api": {
        target: "http://127.0.0.1:11434",
        changeOrigin: true,
      },
      "/opencode-usage": {
        target: "https://opencode.ai/zen/go/v1",
        changeOrigin: true,
        rewrite: () => "/usage",
      },
      "/opencode-models": {
        target: "https://opencode.ai/zen/go/v1",
        changeOrigin: true,
        rewrite: () => "/models",
      },
      // Anthropic has no CORS headers, so the browser preview verifies a
      // Claude API key through this same-origin proxy (x-api-key and
      // anthropic-version pass through from the page fetch).
      "/claude-api": {
        target: "https://api.anthropic.com",
        changeOrigin: true,
        rewrite: (path) => path.replace(/^\/claude-api/, ""),
      },
      "/commandcode-alpha": {
        target: "https://api.commandcode.ai/alpha",
        changeOrigin: true,
        rewrite: (path) => path.replace(/^\/commandcode-alpha/, ""),
      },
      // ChatGPT wham/usage needs chatgpt.com Origin/Referer, which a page
      // fetch cannot spoof — the proxy sets them (documented recipe).
      "/chatgpt-wham": {
        target: "https://chatgpt.com/backend-api",
        changeOrigin: true,
        rewrite: (path) => path.replace(/^\/chatgpt-wham/, ""),
        headers: {
          Origin: "https://chatgpt.com",
          Referer: "https://chatgpt.com/",
          Accept: "application/json",
        },
      },
    },
  },
});
