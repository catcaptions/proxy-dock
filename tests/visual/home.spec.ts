import { expect, test } from "@playwright/test";

test("home overview states", async ({ page }) => {
  await page.goto("/");
  await expect(page.getByTestId("page-home")).toBeVisible();
  await expect(page.getByTestId("home-empty")).toBeVisible();
  await expect(page.getByTestId("page-home")).toHaveScreenshot("home.png");
});
