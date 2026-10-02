import type { Route } from "./App";
import { PROVIDERS } from "./data";

type Props = {
  route: Route;
  onNavigate: (route: Route) => void;
};

export default function Sidebar({ route, onNavigate }: Props) {
  return (
    <nav className="dock-rail" aria-label="Proxy Dock navigation" data-testid="dock-rail">
      <div className="rail-scroll">
        <button
          className="tile"
          data-testid="tile-home"
          aria-label="Home"
          title="Home"
          aria-current={route.name === "home" ? "page" : undefined}
          onClick={() => onNavigate({ name: "home" })}
        >
          <img src="icons/home.svg" alt="" aria-hidden="true" className="tile-icon" />
        </button>
        <div className="tile-gap" aria-hidden="true" />
        {PROVIDERS.map((provider) => (
          <button
            key={provider.slug}
            className="tile provider-tile"
            data-testid={`tile-${provider.slug}`}
            aria-label={provider.label}
            title={provider.label}
            aria-current={route.name === "provider" && route.slug === provider.slug ? "page" : undefined}
            onClick={() => onNavigate({ name: "provider", slug: provider.slug })}
          >
            <img src={provider.icon} alt="" aria-hidden="true" className="tile-icon" onError={(event) => { event.currentTarget.hidden = true; }} />
            <span aria-hidden="true" className="tile-fallback">{provider.monogram}</span>
          </button>
        ))}
      </div>
      <div className="rail-foot">
        <div className="rail-divider" aria-hidden="true" />
        <button
          className="tile"
          data-testid="tile-settings"
          aria-label="Settings"
          title="Settings"
          aria-current={route.name === "settings" ? "page" : undefined}
          onClick={() => onNavigate({ name: "settings" })}
        >
          <img src="icons/settings.svg" alt="" aria-hidden="true" className="tile-icon" />
        </button>
      </div>
    </nav>
  );
}
