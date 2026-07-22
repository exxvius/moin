import { useState } from "react";
import type { ComponentType } from "react";
import { HomeView } from "./views/HomeView";
import { DownloadsView } from "./views/DownloadsView";
import { CompletedView } from "./views/CompletedView";
import { SettingsView } from "./views/SettingsView";
import {
  AddIcon,
  CompletedIcon,
  DownloadsIcon,
  Logo,
  MoonIcon,
  SettingsIcon,
  SunIcon,
} from "./components/icons";
import { StoreProvider } from "./lib/store";
import { useTheme } from "./lib/theme";
import { useAccent } from "./lib/accent";

type View = "home" | "downloads" | "completed" | "settings";

const NAV: {
  id: View;
  label: string;
  icon: ComponentType<{ size?: number }>;
}[] = [
  { id: "home", label: "Add", icon: AddIcon },
  { id: "downloads", label: "Downloads", icon: DownloadsIcon },
  { id: "completed", label: "Completed", icon: CompletedIcon },
  { id: "settings", label: "Settings", icon: SettingsIcon },
];

function Shell() {
  const [theme, toggleTheme] = useTheme();
  const [accent, setAccent] = useAccent();
  const [view, setView] = useState<View>("home");

  return (
    <div className="app">
      <div className="lava" aria-hidden="true">
        <i />
        <i />
        <i />
        <i />
      </div>

      <aside className="rail">
        <div className="rail-brand" title="moin">
          <Logo size={40} />
        </div>

        <nav className="rail-nav" aria-label="Main">
          {NAV.map((n) => {
            const Icon = n.icon;
            return (
              <button
                key={n.id}
                className="rail-btn"
                aria-current={view === n.id}
                aria-label={n.label}
                title={n.label}
                onClick={() => setView(n.id)}
              >
                <Icon size={20} />
              </button>
            );
          })}
        </nav>

        <div className="rail-foot">
          <button
            className="rail-btn"
            onClick={toggleTheme}
            aria-label={
              theme === "dark" ? "Switch to light theme" : "Switch to dark theme"
            }
            title={theme === "dark" ? "Light theme" : "Dark theme"}
          >
            {theme === "dark" ? <MoonIcon size={19} /> : <SunIcon size={19} />}
          </button>
        </div>
      </aside>

      <main className="main">
        {view === "home" && (
          <HomeView onAdded={() => setView("downloads")} />
        )}
        {view === "downloads" && <DownloadsView />}
        {view === "completed" && <CompletedView />}
        {view === "settings" && (
          <SettingsView
            theme={theme}
            toggleTheme={toggleTheme}
            accent={accent}
            setAccent={setAccent}
          />
        )}
      </main>
    </div>
  );
}

export default function App() {
  return (
    <StoreProvider>
      <Shell />
    </StoreProvider>
  );
}
