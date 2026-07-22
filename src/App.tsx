import { useEffect, useState } from "react";
import type { ComponentType } from "react";
import { HomeView } from "./views/HomeView";
import { DownloadsView } from "./views/DownloadsView";
import { SettingsView } from "./views/SettingsView";
import {
  AddIcon,
  DownloadsIcon,
  Logo,
  SettingsIcon,
  ThemeToggleIcon,
} from "./components/icons";
import { StoreProvider } from "./lib/store";
import { initCursorFx } from "./lib/cursor";
import { useTheme } from "./lib/theme";
import { useAccent } from "./lib/accent";
import { useReorderAnim } from "./lib/prefs";

type View = "home" | "downloads" | "settings";

const NAV: {
  id: View;
  label: string;
  icon: ComponentType<{ size?: number }>;
}[] = [
  { id: "home", label: "Add", icon: AddIcon },
  { id: "downloads", label: "Downloads", icon: DownloadsIcon },
];

function Shell() {
  const [theme, toggleTheme] = useTheme();
  const [accent, setAccent] = useAccent();
  const [reorderAnim, setReorderAnim] = useReorderAnim();
  const [view, setView] = useState<View>("home");

  // Cursor-proximity border glow on cards + download rows.
  useEffect(() => initCursorFx(), []);

  // Suppress the webview's native right-click menu; we use our own.
  useEffect(() => {
    const block = (e: MouseEvent) => e.preventDefault();
    document.addEventListener("contextmenu", block);
    return () => document.removeEventListener("contextmenu", block);
  }, []);

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
            <ThemeToggleIcon size={20} />
          </button>
          <button
            className="rail-btn"
            aria-current={view === "settings"}
            aria-label="Settings"
            title="Settings"
            onClick={() => setView("settings")}
          >
            <SettingsIcon size={20} />
          </button>
        </div>
      </aside>

      <main className="main">
        {view === "home" && (
          <HomeView onAdded={() => setView("downloads")} />
        )}
        {view === "downloads" && <DownloadsView animateReorder={reorderAnim} />}
        {view === "settings" && (
          <SettingsView
            accent={accent}
            setAccent={setAccent}
            reorderAnim={reorderAnim}
            setReorderAnim={setReorderAnim}
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
