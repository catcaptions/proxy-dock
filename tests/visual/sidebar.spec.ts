import { expect, test } from "@playwright/test";

test("dock rail layout and tiles", async ({ page }) => {
  await page.goto("/");
  const rail = page.getByTestId("dock-rail");
  await expect(rail).toBeVisible();
  await expect(page.getByTestId("tile-home")).toBeVisible();
  await expect(page.getByTestId("tile-commandcode")).toHaveAttribute("aria-label", "Command Code");
  await expect(page.getByTestId("tile-opencode")).toHaveAttribute("aria-label", "OpenCode");
  await expect(page.getByTestId("tile-chatgpt")).toHaveAttribute("aria-label", "ChatGPT Codex");
  await expect(page.getByTestId("tile-antigravity")).toHaveAttribute("aria-label", "Antigravity");
  await expect(page.getByTestId("tile-claude")).toHaveAttribute("aria-label", "Claude");
  await expect(rail).toHaveScreenshot("dock-rail.png");
});

test("narrow rail keeps destination understandable", async ({ page }) => {
  await page.setViewportSize({ height: 800, width: 700 });
  await page.goto("/");
  await expect(page.getByTestId("destination")).toHaveText("Home");
  await expect(page.getByTestId("dock-rail")).toHaveScreenshot("dock-rail-narrow.png");
});
