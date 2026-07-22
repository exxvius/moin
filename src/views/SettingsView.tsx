import { useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { Select } from "../components/Select";
import { Switch } from "../components/Switch";
import { api } from "../lib/api";
import { subscribeToolProgress } from "../lib/events";
import { formatBytes } from "../lib/format";
import { ACCENTS, type Accent } from "../lib/accent";
import type { BackendInfo, Settings, ToolStatus } from "../lib/types";

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
  const [backends, setBackends] = useState<BackendInfo[]>([]);
  const [tool, setTool] = useState<ToolStatus | null>(null);
  // Download progress while fetching aria2c, or null when idle.
  const [fetching, setFetching] = useState<{ received: number; total: number | null } | null>(
    null,
  );
  const [toolError, setToolError] = useState<string | null>(null);

  useEffect(() => {
    api.getSettings().then(setSettings).catch(() => {});
    api.listBackends().then(setBackends).catch(() => {});
    api.toolStatus().then(setTool).catch(() => {});
  }, []);

  const patch = (change: Partial<Settings>) => {
    setSettings((prev) => {
      if (!prev) return prev;
      const next = { ...prev, ...change };
      api.saveSettings(next).catch(() => {});
      return next;
    });
  };

  const refreshTool = (next: ToolStatus) => {
    setTool(next);
    api.listBackends().then(setBackends).catch(() => {});
  };

  const downloadTool = async () => {
    setToolError(null);
    setFetching({ received: 0, total: null });
    const unlisten = await subscribeToolProgress((p) =>
      setFetching({ received: p.received, total: p.total }),
    );
    try {
      refreshTool(await api.downloadTool());
    } catch (e) {
      setToolError(e instanceof Error ? e.message : String(e));
    } finally {
      unlisten();
      setFetching(null);
    }
  };

  const pickBinary = async () => {
    setToolError(null);
    const picked = await open({
      multiple: false,
      directory: false,
      title: "Select the aria2c binary",
    });
    if (typeof picked !== "string") return;
    try {
      refreshTool(await api.setToolPath(picked));
      patch({ aria2_path: picked });
    } catch (e) {
      setToolError(e instanceof Error ? e.message : String(e));
    }
  };

  const clearBinary = async () => {
    setToolError(null);
    try {
      refreshTool(await api.setToolPath(null));
      patch({ aria2_path: null });
    } catch (e) {
      setToolError(e instanceof Error ? e.message : String(e));
    }
  };

  const httpEngine = settings?.http_backend ?? "embedded";

  // Engine choices: HTTP-capable backends, offering aria2c only once it's usable
  // (but always keeping the current pick so the value resolves).
  const engineOptions = backends
    .filter((b) => b.http && (b.available || b.id === httpEngine))
    .map((b) => ({ value: b.id, label: b.label }));

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
            <div className="setting-label">Download engine</div>
            <div className="dim">
              The built-in engine needs no setup. aria2c is an external
              downloader you can switch to once it's installed below.
            </div>
          </div>
          <Select
            value={httpEngine}
            ariaLabel="Download engine"
            caret
            disabled={!settings || engineOptions.length < 2}
            onChange={(v) => patch({ http_backend: v })}
            options={
              engineOptions.length > 0
                ? engineOptions
                : [{ value: "embedded", label: "Built-in" }]
            }
          />
        </div>

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
              automatically. Set to 1 to always use one. Applies to whichever
              engine is selected.
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
        <div className="card-title">External tools</div>
        <p className="dim">
          Optional binaries moin can use in place of its built-in engines. Set one
          up here, then pick it as an engine where it applies.
        </p>

        <div className="tool-row">
          <div>
            <div className="setting-label">aria2c</div>
            <ToolState tool={tool} fetching={fetching} error={toolError} />
          </div>
          <div className="tool-actions">
            {tool?.can_fetch && (
              <button
                className="dl-btn"
                onClick={downloadTool}
                disabled={fetching !== null}
              >
                {fetching
                  ? "Downloading…"
                  : tool?.source === "managed"
                    ? "Re-download"
                    : "Download"}
              </button>
            )}
            <button
              className="dl-btn"
              onClick={pickBinary}
              disabled={fetching !== null}
            >
              Use my binary…
            </button>
            {tool?.source === "override" && (
              <button
                className="dl-btn danger"
                onClick={clearBinary}
                disabled={fetching !== null}
              >
                Clear
              </button>
            )}
          </div>
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

interface ToolStateProps {
  tool: ToolStatus | null;
  fetching: { received: number; total: number | null } | null;
  error: string | null;
}

/** The status line under the aria2c label: a colored dot plus a plain-English
 * description of where the binary is and what version it is. */
function ToolState({ tool, fetching, error }: ToolStateProps) {
  if (fetching) {
    const suffix = fetching.total
      ? ` of ${formatBytes(fetching.total)}`
      : "";
    return (
      <div className="dim tool-state">
        <span className="tool-dot warn" />
        Downloading aria2c — {formatBytes(fetching.received)}
        {suffix}
      </div>
    );
  }
  if (error) {
    return (
      <div className="dim tool-state">
        <span className="tool-dot bad" />
        {error}
      </div>
    );
  }
  if (!tool || !tool.present) {
    return (
      <div className="dim tool-state">
        <span className="tool-dot bad" />
        Not installed. Download it or point moin at your own copy.
      </div>
    );
  }
  return (
    <div className="dim tool-state">
      <span className="tool-dot ok" />
      {tool.version ? `aria2 ${tool.version}` : "Ready"} · {sourceLabel(tool)}
    </div>
  );
}

function sourceLabel(tool: ToolStatus): string {
  switch (tool.source) {
    case "override":
      return "your binary";
    case "env":
      return "from MOIN_ARIA2";
    case "managed":
      return "managed by moin";
    case "beside":
      return "next to moin";
    case "path":
      return "found on PATH";
    default:
      return tool.path ?? "";
  }
}
