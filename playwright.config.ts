import { defineConfig } from "@playwright/test";

export default defineConfig({
  testDir: "tests/visual",
  snapshotPathTemplate: "{testDir}/__snapshots__/{testFileName}-{arg}{ext}",
  use: { viewport: { height: 800, width: 1280 } },
  webServer: { command: "npm run dev", port: 1420, reuseExistingServer: true },
});
