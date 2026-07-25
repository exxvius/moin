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
import { StoreProvider, useStore } from "./lib/store";
import { initCursorFx } from "./lib/cursor";
import { usePerfMode } from "./lib/perfMode";
import {
  subscribeConfirmQuit,
  subscribeSettings,
  subscribeTaskAdded,
} from "./lib/events";
import { api } from "./lib/api";
import type { Task } from "./lib/types";
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

// What "quitting would interrupt something" means: a transfer the engine is
// actively running. Mirrors the engine's own has_active_transfers() — paused and
// queued items survive a restart untouched, so they don't warrant a prompt.
const RUNNING: ReadonlySet<Task["status"]> = new Set<Task["status"]>([
  "connecting",
  "downloading",
  "checking",
  "seeding",
  "moving",
]);

function Shell() {
  const [theme, toggleTheme] = useTheme();
  const [accent, setAccent] = useAccent();
  const perf = usePerfMode();
  const store = useStore();
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

  // Cursor-proximity border glow on cards + download rows. Performance mode skips
  // the whole thing — the listener, the rAF loop and the per-card style writes —
  // rather than just hiding the result in CSS.
  useEffect(() => {
    if (perf) return;
    return initCursorFx();
  }, [perf]);

  // The shell asks for confirmation when the window is closed with transfers
  // running and "minimize to tray" off — show the quit prompt.
  useEffect(() => {
    const un = subscribeConfirmQuit(() => setQuitPrompt(true));
    return () => {
      un.then((u) => u());
    };
  }, []);

  // The close button is handled natively and can't wait on the engine, so it
  // reads a cached copy of the only two things it needs. We own keeping that
  // current: we already hold the task list, and we follow the setting (which
  // another window may have changed).
  const [closeToTray, setCloseToTray] = useState(true);
  useEffect(() => {
    api
      .getSettings()
      .then((s) => setCloseToTray(s.close_to_tray))
      .catch(() => {});
    return subscribeSettings((s) => setCloseToTray(s.close_to_tray));
  }, []);

  const anyRunning = store.all.some((t) => RUNNING.has(t.status));
  useEffect(() => {
    api.setQuitPolicy(closeToTray, anyRunning).catch(() => {});
  }, [closeToTray, anyRunning]);

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
      {/* Ambient background blobs. Each is a full-viewport compositor layer while
          it drifts, so performance mode drops them from the tree entirely rather
          than hiding them — a hidden layer can still be resident. */}
      {!perf && (
        <div className="lava" aria-hidden="true">
          <i />
          <i />
          <i />
          <i />
        </div>
      )}

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
