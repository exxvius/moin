import { useState } from "react";
import type { ComponentType } from "react";
import { HomeView } from "./views/HomeView";
import { DownloadsView } from "./views/DownloadsView";
import { CompletedView } from "./views/CompletedView";
import { SettingsView } from "./views/SettingsView";
import { useTheme } from "./lib/theme";
import { useAccent } from "./lib/accent";

type View = "home" | "downloads" | "completed" | "settings";

const NAV: { id: View; label: string }[] = [
  { id: "home", label: "Add" },
  { id: "downloads", label: "Downloads" },
  { id: "completed", label: "Completed" },
  { id: "settings", label: "Settings" },
];

const VIEWS: Record<View, ComponentType> = {
  home: HomeView,
  downloads: DownloadsView,
  completed: CompletedView,
  settings: () => null, // rendered explicitly below (needs theme/accent props)
};

export default function App() {
  const [theme, toggleTheme] = useTheme();
  const [accent, setAccent] = useAccent();
  const [view, setView] = useState<View>("home");

  const Body = VIEWS[view];

  return (
    <div className="app">
      <div className="lava" aria-hidden="true">
        <i />
        <i />
        <i />
        <i />
      </div>

      <aside className="sidebar">
        <div className="brand">
          {/* TODO(icons): brand logo mark — awaiting SVG from user */}
          <span className="wordmark">moin</span>
        </div>

        <nav aria-label="Main">
          {NAV.map((n) => (
            <button
              key={n.id}
              className="nav-item"
              aria-current={view === n.id}
              onClick={() => setView(n.id)}
            >
              {/* TODO(icons): nav glyph — awaiting SVG from user */}
              <span className="grow">{n.label}</span>
            </button>
          ))}
        </nav>

        <div className="sidebar-foot">
          <button className="foot-btn" onClick={toggleTheme}>
            {/* TODO(icons): moon/sun glyph — awaiting SVG from user */}
            <span>{theme === "dark" ? "Dark" : "Light"}</span>
          </button>
        </div>
      </aside>

      <main className="main">
        {view === "settings" ? (
          <SettingsView
            theme={theme}
            toggleTheme={toggleTheme}
            accent={accent}
            setAccent={setAccent}
          />
        ) : (
          <Body />
        )}
      </main>
    </div>
  );
}
