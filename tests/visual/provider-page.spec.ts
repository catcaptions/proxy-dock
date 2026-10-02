import { expect, test } from "@playwright/test";

test("provider page sections", async ({ page }) => {
  await page.goto("/");
  await page.getByTestId("tile-commandcode").click();
  await expect(page.getByTestId("page-provider-commandcode")).toBeVisible();
  await expect(page.getByTestId("no-accounts-commandcode")).toBeVisible();
  await page.getByTestId("signin-commandcode").click();
  await expect(page.getByTestId("signin-panel-commandcode")).toBeVisible();
  await expect(page.getByTestId("signin-token-commandcode")).toBeVisible();
  await expect(page.getByTestId("page-provider-commandcode")).toHaveScreenshot("provider-commandcode.png");
});

test("keyboard focus stays visible without permanent tile borders", async ({ page }) => {
  await page.goto("/");
  await page.getByTestId("tile-home").focus();
  await expect(page.getByTestId("tile-home")).toBeFocused();
  await expect(page.getByTestId("dock-rail")).toHaveScreenshot("dock-rail-focus.png");
});
