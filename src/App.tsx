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
import { QuitConfirmModal } from "./components/QuitConfirmModal";
import { RailAccentPicker } from "./components/RailAccentPicker";
import { StoreProvider } from "./lib/store";
import { initCursorFx } from "./lib/cursor";
import { subscribeConfirmQuit, subscribeTaskAdded } from "./lib/events";
import { api } from "./lib/api";
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
  const [quitPrompt, setQuitPrompt] = useState(false);
  // Which rail icon last got clicked + a bump counter, so re-clicking replays its
  // one-shot icon animation (the counter re-keys the icon, remounting it).
  const [pulse, setPulse] = useState<{ id: View | ""; n: number }>({
    id: "",
    n: 0,
  });
  const goto = (id: View) => {
    setView(id);
    setPulse((p) => ({ id, n: p.n + 1 }));
  };
  // Class + remount key for a rail button's icon: it animates only after a click
  // (never on first load), and replays on every click.
  const railClass = (id: View) => `rail-btn${pulse.id === id ? " clicked" : ""}`;
  const iconKey = (id: View) => (pulse.id === id ? pulse.n : `s-${id}`);

  // Cursor-proximity border glow on cards + download rows.
  useEffect(() => initCursorFx(), []);

  // The backend asks for confirmation when the window is closed with transfers
  // running and "minimize to tray" off — show the quit prompt.
  useEffect(() => {
    const un = subscribeConfirmQuit(() => setQuitPrompt(true));
    return () => {
      un.then((u) => u());
    };
  }, []);

  // Easter egg: whenever a download is added, drop the arrow into the tray on the
  // Downloads rail icon — even when you're on another page.
  useEffect(() => {
    const un = subscribeTaskAdded(() =>
      setPulse((p) => ({ id: "downloads", n: p.n + 1 })),
    );
    return () => {
      un.then((u) => u());
    };
  }, []);

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
                className={railClass(n.id)}
                aria-current={view === n.id}
                aria-label={n.label}
                title={n.label}
                onClick={() => goto(n.id)}
              >
                <Icon key={iconKey(n.id)} size={20} />
              </button>
            );
          })}
        </nav>

        <div className="rail-foot">
          <RailAccentPicker accent={accent} setAccent={setAccent} />
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
            className={railClass("settings")}
            aria-current={view === "settings"}
            aria-label="Settings"
            title="Settings"
            onClick={() => goto("settings")}
          >
            <SettingsIcon key={iconKey("settings")} size={20} />
          </button>
        </div>
      </aside>

      <main className="main">
        {view === "downloads" && <DownloadsView />}
        {view === "categories" && <CategoriesView />}
        {view === "settings" && <SettingsView />}
      </main>

      {quitPrompt && (
        <QuitConfirmModal
          onCancel={() => setQuitPrompt(false)}
          onMinimize={() => {
            setQuitPrompt(false);
            api.hideWindow();
          }}
          onQuit={() => api.quitApp()}
        />
      )}
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
