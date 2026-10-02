export type ProviderSlug = "commandcode" | "opencode" | "chatgpt" | "antigravity" | "claude";

export type SignInMethod = "api-key" | "cli-oauth" | "api-key-or-oauth";

export type Provider = {
  slug: ProviderSlug;
  label: string;
  monogram: string;
  icon: string;
  authFlow: string;
  signInMethod: SignInMethod;
  signInTitle: string;
  signInSteps: string[];
  signInNote: string;
};
export function compact(value: number): string {
  if (value >= 1_000_000) return `${(value / 1_000_000).toFixed(1)}M`;
  if (value >= 1_000) return `${(value / 1_000).toFixed(1)}K`;
  return `${value}`;
}

/**
 * Chart series color per provider slug (t3code usage palette mapped onto
 * our slugs). Shared by the Home daily-cost chart and the provider dots on
 * Home account blocks so the two always agree.
 */
export const PROVIDER_SERIES_COLORS: Record<ProviderSlug, string> = {
  commandcode: "#d97757",
  opencode: "#5b9bbd",
  chatgpt: "#0f172a",
  antigravity: "#8c7bd1",
  claude: "#2f9e8f",
};

/** `2026-09-20` -> `Sep 20` for human window/axis labels (uppercased at render). */
export function formatDayShort(iso: string): string {
  const months = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun",
    "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
  ];
  const [, m, d] = iso.split("-").map((part) => Number(part));
  if (m == null || d == null || Number.isNaN(m) || Number.isNaN(d)) return iso;
  return `${months[m - 1] ?? ""} ${d}`;
}

// NOTE (honesty cuts, plan §0): the mock usage pipeline is deleted —
// QuotaGroup/ModelUsage/MockAccount, homeTotals/HomeTotals, TREND_POINTS,
// MOCK_PROVIDERS, GATEWAY_BASE, and the Provider.accounts field were all
// removed. Usage/cost/totals come only from GET /api/usage/* (see api.ts);
// the gateway base URL comes from the gateway_info Tauri command with a
// same-origin fallback (see Settings.tsx). Static PROVIDERS below carry
// provider metadata + sign-in copy only, never numbers.

export const PROVIDERS: Provider[] = [
  {
    slug: "commandcode",
    label: "Command Code",
    monogram: "CG",
    icon: "icons/commandcode-go.svg",
    authFlow: "Studio browser sign-in with automatic key capture.",
    signInMethod: "api-key",
    signInTitle: "Sign in with Command Code",
    signInSteps: [
      "Click “Sign in with Command Code in browser” — Proxy Dock opens the official Studio login and captures the issued key automatically.",
      "If the automatic capture does not complete, Studio shows “Copy your API key” — paste it below and Verify.",
      "The key is tested against Command Code and stored in your OS secret store. Proxy Dock never reads ~/.commandcode/auth.json for you.",
    ],
    signInNote:
      "Sign-in uses Command Code's own Studio login page. Go-plan keys route through the CLI-style integration, which Command Code treats as subscription-specific — check the terms for your use.",
  },
  {
    slug: "opencode",
    label: "OpenCode",
    monogram: "OC",
    icon: "icons/opencode-apple-180.png",
    authFlow: "Subscription API key via Zen.",
    signInMethod: "api-key",
    signInTitle: "Connect your OpenCode key",
    signInSteps: [
      "Sign in at OpenCode Zen and subscribe to Go.",
      "Copy your Go API key.",
      "Paste the key here — it stays in your OS secret store and is sent only to opencode.ai.",
    ],
    signInNote: "Usage limits are dollar-based per model: 5-hour is 20%, weekly is 50%, monthly is 100% of the cap.",
  },
  {
    slug: "chatgpt",
    label: "ChatGPT Codex",
    monogram: "CX",
    icon: "icons/chatgpt-blossom.svg",
    authFlow: "Browser OAuth or API key.",
    signInMethod: "api-key-or-oauth",
    signInTitle: "Sign in to Codex",
    signInSteps: [
      "Click “Sign in with ChatGPT in browser” — Proxy Dock opens the official ChatGPT login (the same flow as `codex login`).",
      "Approve in the browser tab and return here — the redirect is captured automatically, nothing to copy.",
      "Or paste an access token / API key directly — it stays in your OS secret store.",
    ],
    signInNote: "ChatGPT-plan OAuth and Platform API keys bill differently. Proxy Dock never copies ~/.codex/auth.json for you.",
  },
  {
    slug: "antigravity",
    label: "Antigravity",
    monogram: "AG",
    icon: "icons/antigravity.png",
    authFlow: "Google browser sign-in.",
    signInMethod: "api-key-or-oauth",
    signInTitle: "Sign in to Antigravity",
    signInSteps: [
      "Click “Sign in with Google in browser” — Proxy Dock opens the official Google sign-in (the same flow as the `agy` CLI).",
      "Approve in the browser tab and return here — the redirect is captured automatically, nothing to copy.",
      "Or paste an existing token directly — it stays in your OS secret store.",
    ],
    signInNote: "Proxy Dock never reads ~/.gemini/antigravity-cli/ or your Keychain for you.",
  },
  {
    slug: "claude",
    label: "Claude",
    monogram: "CL",
    icon: "icons/claude.svg",
    authFlow: "Browser OAuth (subscription) or Anthropic API key.",
    signInMethod: "api-key-or-oauth",
    signInTitle: "Sign in to Claude",
    signInSteps: [
      "Click “Sign in with Claude in browser” — Proxy Dock opens the official Claude login (the same flow as `claude setup-token`).",
      "Approve in the browser tab and return here — the redirect is captured automatically, nothing to copy.",
      "Or paste an Anthropic API key (sk-ant-…) directly — it is verified against api.anthropic.com and stays in your OS secret store.",
    ],
    signInNote: "Subscription OAuth and Console API keys bill differently. Proxy Dock never reads ~/.claude/.credentials.json for you.",
  },
];
