import { useEffect, useState } from "react";
import { Select } from "../components/Select";
import { Switch } from "../components/Switch";
import { api } from "../lib/api";
import { ACCENTS, type Accent } from "../lib/accent";
import type { Settings } from "../lib/types";

interface Props {
  accent: Accent;
  setAccent: (a: Accent) => void;
  reorderAnim: boolean;
  setReorderAnim: (v: boolean) => void;
}

// 0 = unlimited; the rest are sensible concurrency caps.
const CONCURRENCY_OPTIONS = [0, 1, 2, 3, 4, 5, 6, 8, 10, 16];

// Parallel connections per download. 1 = a single stream (no splitting).
const CONNECTION_OPTIONS = [1, 2, 4, 6, 8, 12, 16];

export function SettingsView({
  accent,
  setAccent,
  reorderAnim,
  setReorderAnim,
}: Props) {
  const [settings, setSettings] = useState<Settings | null>(null);

  useEffect(() => {
    api.getSettings().then(setSettings).catch(() => {});
  }, []);

  const patch = (change: Partial<Settings>) => {
    setSettings((prev) => {
      if (!prev) return prev;
      const next = { ...prev, ...change };
      api.saveSettings(next).catch(() => {});
      return next;
    });
  };

  return (
    <div className="view">
      <div className="view-head">
        <h2>Settings</h2>
        <p>Make moin yours.</p>
      </div>

      <div className="card">
        <div className="card-title">Downloads</div>

        <div className="setting-row">
          <div>
            <div className="setting-label">Concurrent downloads</div>
            <div className="dim">
              How many downloads run at once. The rest wait in the queue. Set to
              Unlimited to run every download immediately.
            </div>
          </div>
          <Select
            value={String(settings?.max_concurrent ?? 4)}
            ariaLabel="Concurrent downloads"
            caret
            disabled={!settings}
            onChange={(v) => patch({ max_concurrent: Number(v) })}
            options={CONCURRENCY_OPTIONS.map((n) => ({
              value: String(n),
              label: n === 0 ? "Unlimited" : String(n),
            }))}
          />
        </div>

        <div className="setting-row">
          <div>
            <div className="setting-label">Connections per download</div>
            <div className="dim">
              Split each file into parallel streams for faster downloads.
              Sources that don't support it fall back to a single stream
              automatically. Set to 1 to always use one.
            </div>
          </div>
          <Select
            value={String(settings?.connections ?? 8)}
            ariaLabel="Connections per download"
            caret
            disabled={!settings}
            onChange={(v) => patch({ connections: Number(v) })}
            options={CONNECTION_OPTIONS.map((n) => ({
              value: String(n),
              label: n === 1 ? "Single stream" : String(n),
            }))}
          />
        </div>
      </div>

      <div className="card">
        <div className="card-title">Appearance</div>

        <div className="setting-row">
          <div>
            <div className="setting-label">Accent color</div>
            <div className="dim">
              Recolors backgrounds, buttons, and progress. Light and dark is in
              the sidebar.
            </div>
          </div>
          <Select
            value={accent}
            ariaLabel="Accent color"
            caret
            onChange={(v) => setAccent(v as Accent)}
            options={ACCENTS.map((a) => ({
              value: a.id,
              label: (
                <span className="accent-option">
                  <span
                    className="accent-dot"
                    style={{ background: a.swatch }}
                  />
                  {a.label}
                </span>
              ),
            }))}
          />
        </div>

        <div className="setting-row">
          <div>
            <div className="setting-label">Reorder animation</div>
            <div className="dim">
              Slide rows into place when the sort order changes. Turn off if
              live-sorted downloads shuffle too much.
            </div>
          </div>
          <Switch
            checked={reorderAnim}
            ariaLabel="Reorder animation"
            onChange={setReorderAnim}
          />
        </div>
      </div>
    </div>
  );
}
