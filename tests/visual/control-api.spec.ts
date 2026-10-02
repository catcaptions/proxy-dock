import { expect, test } from "@playwright/test";

// Structural specs for the Control API panels (plan §3F). No screenshots —
// these assert empty/error states render honestly without a gateway running,
// so they need no baseline updates.

test("provider page exposes models section", async ({ page }) => {
  await page.addInitScript(() => {
    const account = {
      provider: "commandcode",
      accountId: "user@example.com",
      email: "user@example.com",
      keySuffix: "xyz",
      verifiedAt: "2026-09-27T00:00:00.000Z",
      modelCount: null,
      plan: "Pro",
      authKind: "key",
      quota: [],
    };
    localStorage.setItem("proxydock-account:commandcode:user@example.com", JSON.stringify(account));
    localStorage.setItem("proxydock-accounts:commandcode", JSON.stringify(["user@example.com"]));
    localStorage.setItem("proxydock-token:commandcode:user@example.com", "sk-test-key-value");
  });
  await page.goto("/");
  await page.getByTestId("tile-commandcode").click();
  await expect(page.getByTestId("page-provider-commandcode")).toBeVisible();
  await expect(page.getByTestId("models-commandcode")).toBeVisible();
  await expect(page.getByTestId("qp-account-commandcode-user@example.com")).toBeVisible();
  await expect(page.getByTestId("qp-refresh-commandcode-user@example.com")).toBeVisible();
});

test("claude provider page exposes sign-in and models section", async ({ page }) => {
  await page.goto("/");
  await page.getByTestId("tile-claude").click();
  await expect(page.getByTestId("page-provider-claude")).toBeVisible();
  await expect(page.getByTestId("signin-claude")).toBeVisible();
  await expect(page.getByTestId("models-claude")).toBeVisible();
  await expect(page.getByTestId("no-accounts-claude")).toBeVisible();
});

test("settings pricing data section offers a sync button", async ({ page }) => {
  await page.goto("/");
  await page.getByTestId("tile-settings").click();
  await expect(page.getByTestId("page-settings")).toBeVisible();
  await expect(page.getByTestId("pricing-sync")).toBeVisible();
});

test("settings gateway URL is read-only and tray is marked unimplemented", async ({ page }) => {
  await page.goto("/");
  await page.getByTestId("tile-settings").click();
  await expect(page.getByTestId("page-settings")).toBeVisible();
  await expect(page.getByTestId("gateway-url")).toBeVisible();
  await expect(page.getByTestId("gateway-url")).toHaveAttribute("readonly", "");
  await expect(page.getByTestId("import-file")).toBeVisible();
});

test("home shows range tabs and breakdown once signed in (seeded)", async ({ page }) => {
  await page.addInitScript(() => {
    const account = {
      provider: "opencode",
      accountId: "key-abc123",
      email: null,
      keySuffix: "xyz",
      verifiedAt: "2026-09-27T00:00:00.000Z",
      modelCount: 3,
      plan: "Go",
      authKind: "key",
      quota: [],
    };
    localStorage.setItem("proxydock-account:opencode:key-abc123", JSON.stringify(account));
    localStorage.setItem("proxydock-accounts:opencode", JSON.stringify(["key-abc123"]));
    localStorage.setItem("proxydock-token:opencode:key-abc123", "sk-test-key-value");
  });
  await page.goto("/");
  await expect(page.getByTestId("page-home")).toBeVisible();
  await expect(page.getByTestId("range-7d")).toBeVisible();
  // Without a gateway running, grouped usage errors honestly (no fabrication).
  // With a live gateway holding real usage, the ready table renders instead.
  await expect(
    page
      .getByTestId("breakdown-empty")
      .or(page.getByTestId("breakdown-error"))
      .or(page.getByTestId("breakdown-model"))
      .first(),
  ).toBeVisible();
});

test("wrong gateway key shows one terse state", async ({ page }) => {
  await page.addInitScript(() => {
    const account = {
      provider: "opencode",
      accountId: "key-abc123",
      email: null,
      keySuffix: "xyz",
      verifiedAt: "2026-09-27T00:00:00.000Z",
      modelCount: 3,
      plan: "Go",
      authKind: "key",
      quota: [],
    };
    localStorage.setItem("proxydock-account:opencode:key-abc123", JSON.stringify(account));
    localStorage.setItem("proxydock-accounts:opencode", JSON.stringify(["key-abc123"]));
    localStorage.setItem("proxydock-token:opencode:key-abc123", "sk-test-key-value");
  });
  // Mocked §0 bearer gate: every usage call 401s with the canonical envelope.
  await page.route("**/api/usage/**", (route) =>
    route.fulfill({
      status: 401,
      contentType: "application/json",
      body: JSON.stringify({ error: { message: "wrong or missing gateway key", type: "proxy_dock_unauthorized", code: 401 } }),
    }),
  );
  await page.goto("/");
  await expect(page.getByTestId("home-usage-error")).toContainText("Wrong key");
});

test("home never shows raw proxy 500s", async ({ page }) => {
  await page.goto("/");
  await expect(page.getByTestId("page-home")).toBeVisible();
  // Holds with or without a gateway: reachable → content loads; unreachable
  // → one friendly banner. Raw vite-proxy 500 text must never render.
  await expect(page.locator("text=Request failed (500)").first()).toHaveCount(0);
});

test("offline banner explains an unreachable gateway", async ({ page }) => {
  // Deterministic offline simulation: block all /api traffic.
  await page.route("**/api/**", (route) => route.abort());
  await page.goto("/");
  await expect(page.getByTestId("page-home")).toBeVisible();
  await expect(page.getByTestId("gateway-offline")).toBeVisible();
  await expect(page.getByTestId("gateway-offline")).toContainText("Gateway unreachable");
});

test("testids are namespaced per provider", async ({ page }) => {
  await page.addInitScript(() => {
    for (const provider of ["commandcode", "opencode"]) {
      const account = {
        provider,
        accountId: "user@example.com",
        email: "user@example.com",
        keySuffix: "xyz",
        verifiedAt: "2026-09-27T00:00:00.000Z",
        modelCount: null,
        plan: "Go",
        authKind: "key",
        quota: [{ id: "window-fiveHour", label: "5-hour", remainingPct: 80, resetText: "", resetInText: "", urgent: false }],
      };
      localStorage.setItem(`proxydock-account:${provider}:user@example.com`, JSON.stringify(account));
      localStorage.setItem(`proxydock-accounts:${provider}`, JSON.stringify(["user@example.com"]));
      localStorage.setItem(`proxydock-token:${provider}:user@example.com`, "sk-test-key-value");
    }
  });
  await page.goto("/");
  await expect(page.getByTestId("account-commandcode-user@example.com")).toBeVisible();
  await expect(page.getByTestId("account-opencode-user@example.com")).toBeVisible();
  await expect(page.getByTestId("refresh-commandcode-user@example.com")).toBeVisible();
  await expect(page.getByTestId("refresh-opencode-user@example.com")).toBeVisible();
  await expect(page.getByTestId("quota-commandcode-user@example.com-window-fiveHour")).toBeVisible();
  await expect(page.getByTestId("quota-opencode-user@example.com-window-fiveHour")).toBeVisible();
});

test("provider pages share the offline banner", async ({ page }) => {
  await page.route("**/api/**", (route) => route.abort());
  await page.goto("/");
  await page.getByTestId("tile-commandcode").click();
  await expect(page.getByTestId("page-provider-commandcode")).toBeVisible();
  await expect(page.getByTestId("gateway-offline")).toBeVisible();
  await expect(page.getByTestId("gateway-offline")).toContainText("Gateway unreachable");
});

test("routing editor shows enable toggle without ordering buttons", async ({ page }) => {
  await page.addInitScript(() => {
    const account = {
      provider: "commandcode",
      accountId: "user@example.com",
      email: "user@example.com",
      keySuffix: "xyz",
      verifiedAt: "2026-09-27T00:00:00.000Z",
      modelCount: null,
      plan: "Pro",
      authKind: "key",
      quota: [],
    };
    localStorage.setItem("proxydock-account:commandcode:user@example.com", JSON.stringify(account));
    localStorage.setItem("proxydock-accounts:commandcode", JSON.stringify(["user@example.com"]));
    localStorage.setItem("proxydock-token:commandcode:user@example.com", "sk-test-key-value");
  });
  await page.route("**/api/routing/accounts**", (route) =>
    route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        provider: "commandcode",
        accounts: [
          { accountId: "user@example.com", label: "main", priority: 0, enabled: true, plan: "Pro", hasToken: true, isDefault: true },
        ],
      }),
    }),
  );
  await page.goto("/");
  await page.getByTestId("tile-commandcode").click();
  await expect(page.getByTestId("page-provider-commandcode")).toBeVisible();
  await expect(page.getByTestId("routing-default-commandcode-user@example.com")).toBeVisible();
  await expect(page.getByTestId("routing-enable-commandcode-user@example.com")).toBeVisible();
  // No ordering buttons, no pN numbering, no drag handle on a single account.
  await expect(page.getByTestId("routing-commandcode-user@example.com")).toHaveCount(0);
  await expect(page.getByTestId("qp-drag-commandcode-user@example.com")).toHaveCount(0);
  await expect(page.locator('[data-testid^="routing-priority-"]')).toHaveCount(0);
});

test("drag reorder persists visual order via priority PATCH", async ({ page }) => {
  await page.addInitScript(() => {
    const ids = ["a@example.com", "b@example.com"];
    for (const id of ids) {
      const account = {
        provider: "commandcode",
        accountId: id,
        email: id,
        keySuffix: "xyz",
        verifiedAt: "2026-09-27T00:00:00.000Z",
        modelCount: null,
        plan: "Pro",
        authKind: "key",
        quota: [],
      };
      localStorage.setItem(`proxydock-account:commandcode:${id}`, JSON.stringify(account));
      localStorage.setItem(`proxydock-token:commandcode:${id}`, "sk-test-key-value");
    }
    localStorage.setItem("proxydock-accounts:commandcode", JSON.stringify(ids));
  });
  const rows = [
    { accountId: "a@example.com", label: "", priority: 0, enabled: true, plan: "Pro", hasToken: true, isDefault: true },
    { accountId: "b@example.com", label: "", priority: 1, enabled: true, plan: "Pro", hasToken: true, isDefault: false },
  ];
  const patches: { id: string; priority: number }[] = [];
  await page.route("**/api/routing/accounts**", (route) =>
    route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ provider: "commandcode", accounts: rows }),
    }),
  );
  await page.route("**/api/accounts/commandcode/*", (route) => {
    const id = decodeURIComponent(route.request().url().split("/").pop() ?? "");
    const body = route.request().postDataJSON() as { priority?: number };
    const row = rows.find((r) => r.accountId === id);
    if (row && typeof body?.priority === "number") {
      row.priority = body.priority;
      patches.push({ id, priority: body.priority });
      rows.sort((x, y) => x.priority - y.priority);
      rows.forEach((r, i) => {
        r.isDefault = i === 0;
      });
    }
    route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify(row ?? {}),
    });
  });
  await page.goto("/");
  await page.getByTestId("tile-commandcode").click();
  await expect(page.getByTestId("page-provider-commandcode")).toBeVisible();
  // Two accounts → drag handles render, no pN numbering anywhere.
  await expect(page.getByTestId("qp-drag-commandcode-a@example.com")).toBeVisible();
  await expect(page.getByTestId("qp-drag-commandcode-b@example.com")).toBeVisible();
  await expect(page.locator('[data-testid^="routing-priority-"]')).toHaveCount(0);
  // Keyboard move (deterministic): b above a.
  await page.getByTestId("qp-drag-commandcode-b@example.com").focus();
  await page.keyboard.press("ArrowUp");
  await expect
    .poll(async () => patches.map((p) => `${p.id}=${p.priority}`).sort().join(","), { timeout: 5000 })
    .toBe("a@example.com=1,b@example.com=0");
  // Visual order follows the new priorities.
  const first = page.getByTestId(/qp-account-commandcode-.*/).first();
  await expect(first).toHaveAttribute("data-testid", "qp-account-commandcode-b@example.com");
});
