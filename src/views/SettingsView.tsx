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

// Smallest piece worth its own connection, in bytes. 1 MiB is aria2's floor, so
// the options start there. Files below the chosen size download in one stream.
const MIB = 1024 * 1024;
const MIN_SPLIT_OPTIONS = [1, 2, 4, 8, 16, 32].map((m) => m * MIB);

// Seconds of no data before a download is marked Stalled; 0 = wait forever.
const STALL_OPTIONS = [30, 60, 120, 300, 0];
// Seconds to wait to establish a connection; 0 = the OS default.
const CONNECT_OPTIONS = [5, 10, 15, 30, 60, 0];
// Seed until this upload/download ratio, then stop; 0 = seed forever.
const SEED_RATIO_OPTIONS = [0, 0.5, 1, 1.5, 2, 3, 5];
// Minutes to keep seeding after finishing, then stop; 0 = no time limit.
const SEED_TIME_OPTIONS = [0, 30, 60, 120, 360, 720, 1440];

/** "30 minutes" / "6 hours" / "1 day" for a whole number of minutes. */
function minutesLabel(n: number): string {
  if (n % 1440 === 0) {
    const d = n / 1440;
    return d === 1 ? "1 day" : `${d} days`;
  }
  if (n % 60 === 0) {
    const h = n / 60;
    return h === 1 ? "1 hour" : `${h} hours`;
  }
  return `${n} minutes`;
}

/** "45 seconds" / "2 minutes" for a whole number of seconds. */
function secondsLabel(n: number): string {
  if (n % 60 === 0) {
    const m = n / 60;
    return m === 1 ? "1 minute" : `${m} minutes`;
  }
  return `${n} seconds`;
}

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
  // Local draft for the RPC port so we commit on blur, not on every keystroke.
  const [portDraft, setPortDraft] = useState("");
  const [tokenCopied, setTokenCopied] = useState(false);

  useEffect(() => {
    api.getSettings().then(setSettings).catch(() => {});
    api.listBackends().then(setBackends).catch(() => {});
    api.toolStatus().then(setTool).catch(() => {});
  }, []);

  // Keep the port field in step with the loaded/committed value.
  useEffect(() => {
    if (settings) setPortDraft(String(settings.rpc_port));
  }, [settings?.rpc_port]);

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

  // Commit the port only if it's a valid TCP port; otherwise snap back to the
  // saved value so the field never holds something the server can't bind.
  const commitPort = () => {
    const n = Number(portDraft);
    if (Number.isInteger(n) && n >= 1 && n <= 65535) {
      if (n !== settings?.rpc_port) patch({ rpc_port: n });
    } else {
      setPortDraft(String(settings?.rpc_port ?? 47653));
    }
  };

  const regenerateToken = async () => {
    try {
      const token = await api.regenerateRpcToken();
      // The backend already persisted it; just mirror it locally (no re-save).
      setSettings((prev) => (prev ? { ...prev, rpc_token: token } : prev));
    } catch {
      /* leave the old token in place on failure */
    }
  };

  const copyToken = async () => {
    if (!settings?.rpc_token) return;
    try {
      await navigator.clipboard.writeText(settings.rpc_token);
      setTokenCopied(true);
      setTimeout(() => setTokenCopied(false), 1500);
    } catch {
      /* clipboard may be unavailable; the field is selectable as a fallback */
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

        <div className="setting-row">
          <div>
            <div className="setting-label">Minimum split size</div>
            <div className="dim">
              A file smaller than this downloads in a single stream; larger ones
              split into parallel pieces no smaller than this. Lower it to split
              more eagerly.
            </div>
          </div>
          <Select
            value={String(settings?.min_split_size ?? MIB)}
            ariaLabel="Minimum split size"
            caret
            disabled={!settings}
            onChange={(v) => patch({ min_split_size: Number(v) })}
            options={MIN_SPLIT_OPTIONS.map((n) => ({
              value: String(n),
              label: formatBytes(n),
            }))}
          />
        </div>

        <div className="setting-row">
          <div>
            <div className="setting-label">Hide partial downloads</div>
            <div className="dim">
              Keep the in-progress <code>.part</code> file hidden while it
              downloads; the finished file appears when it's done. Works where the
              OS supports it (Windows).
            </div>
          </div>
          <Switch
            checked={settings?.hide_part_files ?? false}
            ariaLabel="Hide partial downloads"
            disabled={!settings}
            onChange={(v) => patch({ hide_part_files: v })}
          />
        </div>
      </div>

      <div className="card">
        <div className="card-title">Behavior</div>

        <div className="setting-row">
          <div>
            <div className="setting-label">Changing a download's category</div>
            <div className="dim">
              What happens when you move a download to another category. Move the
              file and moin relocates it into that category's folder, showing a
              Moving status until it lands, then resumes or marks it done.
            </div>
          </div>
          <Select
            value={settings?.category_change ?? "change-only"}
            ariaLabel="Category change behavior"
            caret
            disabled={!settings}
            onChange={(v) =>
              patch({ category_change: v as Settings["category_change"] })
            }
            options={[
              { value: "change-only", label: "Just change the category" },
              { value: "move-file", label: "Move file to category folder" },
            ]}
          />
        </div>
      </div>

      <div className="card">
        <div className="card-title">Advanced</div>

        <div className="setting-row">
          <div>
            <div className="setting-label">Stall timeout</div>
            <div className="dim">
              How long a download may go without receiving any data before it's
              marked Stalled. A stalled download keeps its progress — retry it
              from the right-click menu. Set to Never to keep waiting.
            </div>
          </div>
          <Select
            value={String(settings?.stall_timeout_secs ?? 60)}
            ariaLabel="Stall timeout"
            caret
            disabled={!settings}
            onChange={(v) => patch({ stall_timeout_secs: Number(v) })}
            options={STALL_OPTIONS.map((n) => ({
              value: String(n),
              label: n === 0 ? "Never" : secondsLabel(n),
            }))}
          />
        </div>

        <div className="setting-row">
          <div>
            <div className="setting-label">Connection timeout</div>
            <div className="dim">
              How long to wait to reach a server before giving up on the
              connection. Applies to the built-in engine.
            </div>
          </div>
          <Select
            value={String(settings?.connect_timeout_secs ?? 30)}
            ariaLabel="Connection timeout"
            caret
            disabled={!settings}
            onChange={(v) => patch({ connect_timeout_secs: Number(v) })}
            options={CONNECT_OPTIONS.map((n) => ({
              value: String(n),
              label: n === 0 ? "No limit" : secondsLabel(n),
            }))}
          />
        </div>
      </div>

      <div className="card">
        <div className="card-title">Torrents</div>

        <div className="setting-row">
          <div>
            <div className="setting-label">Seed ratio limit</div>
            <div className="dim">
              Stop seeding a torrent once it has uploaded this much relative to
              what it downloaded. Set to Unlimited to keep seeding until you stop
              it by hand. You can always resume seeding a finished torrent from
              the right-click menu.
            </div>
          </div>
          <Select
            value={String(settings?.seed_ratio_limit ?? 0)}
            ariaLabel="Seed ratio limit"
            caret
            disabled={!settings}
            onChange={(v) => patch({ seed_ratio_limit: Number(v) })}
            options={SEED_RATIO_OPTIONS.map((n) => ({
              value: String(n),
              label: n === 0 ? "Unlimited" : n.toFixed(1),
            }))}
          />
        </div>

        <div className="setting-row">
          <div>
            <div className="setting-label">Seed time limit</div>
            <div className="dim">
              Stop seeding this long after a torrent finishes downloading.
              Whichever of the ratio or time limit is reached first stops it.
            </div>
          </div>
          <Select
            value={String(settings?.seed_time_limit_mins ?? 0)}
            ariaLabel="Seed time limit"
            caret
            disabled={!settings}
            onChange={(v) => patch({ seed_time_limit_mins: Number(v) })}
            options={SEED_TIME_OPTIONS.map((n) => ({
              value: String(n),
              label: n === 0 ? "No limit" : minutesLabel(n),
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
        <div className="card-title">Browser integration</div>
        <p className="dim">
          Send downloads straight from your browser to moin with the companion
          extension. It talks to moin over a local endpoint bound to{" "}
          <code>127.0.0.1</code> — this machine only, never the network. The
          toggle and port take effect after you restart moin.
        </p>

        <div className="setting-row">
          <div>
            <div className="setting-label">Enable browser integration</div>
            <div className="dim">
              Lets the extension hand downloads to moin. Applies after a restart.
            </div>
          </div>
          <Switch
            checked={settings?.rpc_enabled ?? false}
            ariaLabel="Enable browser integration"
            disabled={!settings}
            onChange={(v) => patch({ rpc_enabled: v })}
          />
        </div>

        <div className="setting-row">
          <div>
            <div className="setting-label">Port</div>
            <div className="dim">
              The port the extension connects to. Change it only if another app
              already uses it. Applies after a restart.
            </div>
          </div>
          <input
            className="add-input selectable port-input"
            type="number"
            min={1}
            max={65535}
            value={portDraft}
            aria-label="Browser integration port"
            disabled={!settings}
            onChange={(e) => setPortDraft(e.target.value)}
            onBlur={commitPort}
            onKeyDown={(e) => {
              if (e.key === "Enter") e.currentTarget.blur();
            }}
          />
        </div>

        <div className="setting-row">
          <div>
            <div className="setting-label">Access token</div>
            <div className="dim">
              Paste this into the extension to pair it with moin. Regenerate it to
              cut off a paired browser — you'll need to pair again afterwards.
            </div>
          </div>
          <div className="token-actions">
            <input
              className="add-input selectable token-readout"
              type="text"
              readOnly
              value={settings?.rpc_token ?? ""}
              aria-label="Browser integration access token"
              onFocus={(e) => e.currentTarget.select()}
            />
            <button
              className="dl-btn"
              onClick={copyToken}
              disabled={!settings?.rpc_token}
            >
              {tokenCopied ? "Copied" : "Copy"}
            </button>
            <button className="dl-btn" onClick={regenerateToken} disabled={!settings}>
              Regenerate
            </button>
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
