import { PROVIDERS } from "./data";
import { migrateCommandCodeSlug, migrateProxyHubKeys } from "./auth";
import Home from "./pages/Home";
import ProviderPage from "./pages/ProviderPage";
import Settings from "./pages/Settings";
import Sidebar from "./Sidebar";
import { useEffect, useState } from "react";

export type Route = { name: "home" } | { name: "provider"; slug: string } | { name: "settings" };

export default function App() {
  const [route, setRoute] = useState<Route>({ name: "home" });
  useEffect(() => {
    migrateProxyHubKeys();
    migrateCommandCodeSlug();
  }, []);

  return (
    <div className="shell">
      <Sidebar route={route} onNavigate={setRoute} />
      <main className={route.name === "provider" ? "workspace workspace-provider" : "workspace"} aria-live="polite">
        <header className="topbar">
          <span className="destination" data-testid="destination">
            {route.name === "home" ? "Home" : route.name === "settings" ? "Settings" : `Provider · ${route.slug}`}
          </span>
        </header>
        {route.name === "home" && <Home onOpenProvider={(slug) => setRoute({ name: "provider", slug })} />}
        {route.name === "provider" && (
          <ProviderPage
            key={route.slug}
            slug={route.slug}
            provider={PROVIDERS.find((p) => p.slug === route.slug)}
          />
        )}
        {route.name === "settings" && <Settings />}
      </main>
    </div>
  );
}
