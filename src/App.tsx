import { useEffect, useState } from "react";
import type { ComponentType } from "react";
import { DownloadsView } from "./views/DownloadsView";
import { CategoriesView } from "./views/CategoriesView";
import { SettingsView } from "./views/SettingsView";
import {
  CategoriesIcon,
  DownloadsIcon,
  Logo,
  SettingsIcon,
  ThemeToggleIcon,
} from "./components/icons";
import { StoreProvider } from "./lib/store";
import { initCursorFx } from "./lib/cursor";
import { useTheme } from "./lib/theme";
import { useAccent } from "./lib/accent";

type View = "downloads" | "categories" | "settings";

const NAV: {
  id: View;
  label: string;
  icon: ComponentType<{ size?: number }>;
}[] = [
  { id: "downloads", label: "Downloads", icon: DownloadsIcon },
  { id: "categories", label: "Categories", icon: CategoriesIcon },
];

function Shell() {
  const [theme, toggleTheme] = useTheme();
  const [accent, setAccent] = useAccent();
  const [view, setView] = useState<View>("downloads");

  // Cursor-proximity border glow on cards + download rows.
  useEffect(() => initCursorFx(), []);

  // Halt looping animations while the window is hidden/minimized/occluded, so the
  // compositor isn't repainting the lava blobs and live progress bars for a
  // window nobody can see (and their promoted layers can be freed).
  useEffect(() => {
    const sync = () =>
      document.documentElement.toggleAttribute(
        "data-anim-paused",
        document.hidden,
      );
    sync();
    document.addEventListener("visibilitychange", sync);
    return () => document.removeEventListener("visibilitychange", sync);
  }, []);

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
        {view === "downloads" && <DownloadsView />}
        {view === "categories" && <CategoriesView />}
        {view === "settings" && (
          <SettingsView accent={accent} setAccent={setAccent} />
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
