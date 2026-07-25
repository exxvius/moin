import { useEffect, useState } from "react";
import { Heart } from "lucide-react";
import { open } from "@tauri-apps/plugin-dialog";
import { openUrl } from "@tauri-apps/plugin-opener";
import { Select } from "../components/Select";
import { Switch } from "../components/Switch";
import { api } from "../lib/api";
import { subscribeToolProgress } from "../lib/events";
import { setPerfMode, usePerfMode } from "../lib/perfMode";
import { formatBytes } from "../lib/format";
import type { BackendInfo, Settings, ToolStatus, UpdateInfo } from "../lib/types";

/** The author's profile, credited at the bottom of the settings view. */
const AUTHOR_URL = "https://github.com/exxvius";
/** Upstream of the built-in BitTorrent engine, credited in the About card. */
const RQBIT_URL = "https://github.com/ikatson/rqbit";

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
// Torrent up/download rate caps in bytes/sec; 0 = unlimited.
const RATE_LIMIT_OPTIONS = [0, 64, 128, 256, 512, 1024, 2048, 5120, 10240, 20480].map(
  (kib) => kib * 1024,
);
// Seconds to wait for a magnet's metadata before giving up.
const MAGNET_TIMEOUT_OPTIONS = [30, 60, 120, 180, 300, 600];
// Seconds between tracker scrapes for seeder/leecher counts.
const SCRAPE_INTERVAL_OPTIONS = [30, 60, 90, 120, 300, 600];
// Seconds to wait on a peer's handshake before dropping it.
const PEER_CONNECT_OPTIONS = [3, 5, 10, 15, 30];
// How many ports past the listen port to try if it's busy.
const PORT_SPAN_OPTIONS = [0, 5, 10, 20, 50, 100];

// How often batched progress reaches the UI, in ms. Every active transfer's
// readings collapse into one event per flush, so this is a direct multiplier on
// how much work the app does per second — the labels name the trade rather than
// the number, since the number means nothing without it. Independent of
// performance mode: a slower cadence is just as useful with the full look on.
// Must stay inside the backend's clamp (100–5000 ms).
const FLUSH_OPTIONS: { ms: number; label: string }[] = [
  { ms: 250, label: "Smooth — 4× a second" },
  { ms: 500, label: "Balanced — 2× a second" },
  { ms: 1000, label: "Light — once a second" },
  { ms: 2000, label: "Minimal — every 2 seconds" },
];

// The tabs of the combined settings card: General (behavior + advanced), HTTP
// (direct-download tuning), Torrent (torrent tuning).
type SettingsTab = "general" | "http" | "torrent";
const SETTINGS_TABS: { id: SettingsTab; label: string }[] = [
  { id: "general", label: "General" },
  { id: "http", label: "HTTP" },
  { id: "torrent", label: "Torrent" },
];

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

export function SettingsView() {
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
  // Same, for the torrent listen port.
  const [torrentPortDraft, setTorrentPortDraft] = useState("");
  const [tokenCopied, setTokenCopied] = useState(false);
  // Which tab of the combined General/HTTP/Torrent settings card is showing.
  const [settingsTab, setSettingsTab] = useState<SettingsTab>("general");
  // A display preference, kept locally rather than in the settings file — so it's
  // available before the backend answers and never flashes the full look.
  const perf = usePerfMode();

  useEffect(() => {
    api.getSettings().then(setSettings).catch(() => {});
    api.listBackends().then(setBackends).catch(() => {});
    api.toolStatus().then(setTool).catch(() => {});
  }, []);

  // Keep the port field in step with the loaded/committed value.
  useEffect(() => {
    if (settings) setPortDraft(String(settings.rpc_port));
  }, [settings?.rpc_port]);

  useEffect(() => {
    if (settings) setTorrentPortDraft(String(settings.torrent_listen_port));
  }, [settings?.torrent_listen_port]);

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

  // Same as the RPC port: only commit a valid TCP port, else snap back.
  const commitTorrentPort = () => {
    const n = Number(torrentPortDraft);
    if (Number.isInteger(n) && n >= 1 && n <= 65535) {
      if (n !== settings?.torrent_listen_port) patch({ torrent_listen_port: n });
    } else {
      setTorrentPortDraft(String(settings?.torrent_listen_port ?? 4240));
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
  const torrentEngine = settings?.torrent_backend ?? "embedded";

  // Engine choices: HTTP-capable backends, offering aria2c only once it's usable
  // (but always keeping the current pick so the value resolves).
  const engineOptions = backends
    .filter((b) => b.http && (b.available || b.id === httpEngine))
    .map((b) => ({ value: b.id, label: b.label }));

  // Same shape for torrent-capable backends (aria2c joins once it's installed).
  const torrentEngineOptions = backends
    .filter((b) => b.torrent && (b.available || b.id === torrentEngine))
    .map((b) => ({ value: b.id, label: b.label }));

  return (
    <div className="view">
      <div className="view-head">
        <h2>Settings</h2>
        <p>Make moin yours.</p>
      </div>

      <div className="card">
        <div className="settings-tabs" role="tablist">
          {SETTINGS_TABS.map(({ id, label }) => (
            <button
              key={id}
              role="tab"
              aria-selected={settingsTab === id}
              className={`settings-tab${settingsTab === id ? " active" : ""}`}
              onClick={() => setSettingsTab(id)}
            >
              {label}
            </button>
          ))}
        </div>

        {settingsTab === "general" && (
          <>
            <div className="setting-row">
              <div>
                <div className="setting-label">Concurrent downloads</div>
                <div className="dim">
                  How many downloads run at once; the rest wait in the queue.
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
                <div className="setting-label">Changing a download's category</div>
                <div className="dim">
                  What happens to the file when you move a download to another
                  category.
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

            <div className="setting-row">
              <div>
                <div className="setting-label">Stall timeout</div>
                <div className="dim">
                  How long a download may receive no data before it's marked
                  Stalled.
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
                <div className="setting-label">Progress update rate</div>
                <div className="dim">
                  How often the list refreshes its speeds, percentages and totals.
                </div>
              </div>
              <Select
                value={String(settings?.progress_flush_ms ?? 250)}
                ariaLabel="Progress update rate"
                caret
                disabled={!settings}
                onChange={(v) => patch({ progress_flush_ms: Number(v) })}
                options={FLUSH_OPTIONS.map(({ ms, label }) => ({
                  value: String(ms),
                  label,
                }))}
              />
            </div>

            <div className="setting-row">
              <div>
                <div className="setting-label">Close button minimizes to tray</div>
                <div className="dim">
                  Closing the window hides moin to the tray instead of quitting.
                </div>
              </div>
              <Switch
                checked={settings?.close_to_tray ?? true}
                ariaLabel="Close button minimizes to tray"
                disabled={!settings}
                onChange={(v) => patch({ close_to_tray: v })}
              />
            </div>

            <div className="setting-row">
              <div>
                <div className="setting-label">Notify when downloads finish</div>
                <div className="dim">
                  Show a system notification when a download completes.
                </div>
              </div>
              <Switch
                checked={settings?.notify_on_complete ?? true}
                ariaLabel="Notify when downloads finish"
                disabled={!settings}
                onChange={(v) => patch({ notify_on_complete: v })}
              />
            </div>

            <div className="setting-row">
              <div>
                <div className="setting-label">Add downloads paused</div>
                <div className="dim">
                  New downloads wait paused instead of starting right away.
                </div>
              </div>
              <Switch
                checked={settings?.add_paused ?? false}
                ariaLabel="Add downloads paused"
                disabled={!settings}
                onChange={(v) => patch({ add_paused: v })}
              />
            </div>

            <div className="setting-row">
              <div>
                <div className="setting-label">Performance mode</div>
                <div className="dim">
                  Strip the interface back to flat colour — no animations, blur or
                  effects — for very large lists.
                </div>
              </div>
              <Switch
                checked={perf}
                ariaLabel="Performance mode"
                onChange={setPerfMode}
              />
            </div>

          </>
        )}

        {settingsTab === "http" && (
          <>
        <div className="setting-row">
          <div>
            <div className="setting-label">Download engine</div>
            <div className="dim">Which engine handles direct downloads.</div>
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
            <div className="setting-label">Connection timeout</div>
            <div className="dim">
              How long to wait to reach a server before giving up.
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

        <div className="setting-row">
          <div>
            <div className="setting-label">Connections per download</div>
            <div className="dim">
              Split each file into parallel streams for faster downloads.
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
              Files smaller than this download in a single stream.
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
              Keep the in-progress <code>.part</code> file hidden until it
              finishes.
            </div>
          </div>
          <Switch
            checked={settings?.hide_part_files ?? false}
            ariaLabel="Hide partial downloads"
            disabled={!settings}
            onChange={(v) => patch({ hide_part_files: v })}
          />
        </div>
          </>
        )}

        {settingsTab === "torrent" && (
          <>
        <div className="setting-row">
          <div>
            <div className="setting-label">Torrent engine</div>
            <div className="dim">Which engine handles torrents.</div>
          </div>
          <Select
            value={torrentEngine}
            ariaLabel="Torrent engine"
            caret
            disabled={!settings || torrentEngineOptions.length < 2}
            onChange={(v) => patch({ torrent_backend: v })}
            options={
              torrentEngineOptions.length > 0
                ? torrentEngineOptions
                : [{ value: "embedded", label: "Built-in" }]
            }
          />
        </div>

        <div className="setting-row">
          <div>
            <div className="setting-label">Download limit</div>
            <div className="dim">
              Cap how fast torrents download, across all of them.
            </div>
          </div>
          <Select
            value={String(settings?.torrent_download_limit ?? 0)}
            ariaLabel="Torrent download limit"
            caret
            disabled={!settings}
            onChange={(v) => patch({ torrent_download_limit: Number(v) })}
            options={RATE_LIMIT_OPTIONS.map((n) => ({
              value: String(n),
              label: n === 0 ? "Unlimited" : `${formatBytes(n)}/s`,
            }))}
          />
        </div>

        <div className="setting-row">
          <div>
            <div className="setting-label">Upload limit</div>
            <div className="dim">Cap how fast torrents upload to peers.</div>
          </div>
          <Select
            value={String(settings?.torrent_upload_limit ?? 0)}
            ariaLabel="Torrent upload limit"
            caret
            disabled={!settings}
            onChange={(v) => patch({ torrent_upload_limit: Number(v) })}
            options={RATE_LIMIT_OPTIONS.map((n) => ({
              value: String(n),
              label: n === 0 ? "Unlimited" : `${formatBytes(n)}/s`,
            }))}
          />
        </div>

        <div className="setting-row">
          <div>
            <div className="setting-label">Listen port</div>
            <div className="dim">
              The port moin accepts incoming peer connections on. Applies after a
              restart.
            </div>
          </div>
          <input
            className="add-input selectable port-input"
            type="number"
            min={1}
            max={65535}
            value={torrentPortDraft}
            aria-label="Torrent listen port"
            disabled={!settings}
            onChange={(e) => setTorrentPortDraft(e.target.value)}
            onBlur={commitTorrentPort}
            onKeyDown={(e) => {
              if (e.key === "Enter") e.currentTarget.blur();
            }}
          />
        </div>

        <div className="setting-row">
          <div>
            <div className="setting-label">Peer discovery (DHT)</div>
            <div className="dim">
              Find peers without a tracker. Applies after a restart.
            </div>
          </div>
          <Switch
            checked={settings?.torrent_dht ?? true}
            ariaLabel="Peer discovery (DHT)"
            disabled={!settings}
            onChange={(v) => patch({ torrent_dht: v })}
          />
        </div>

        <div className="setting-row">
          <div>
            <div className="setting-label">Forward my port (UPnP)</div>
            <div className="dim">
              Ask your router to open the listen port. Applies after a restart.
            </div>
          </div>
          <Switch
            checked={settings?.torrent_upnp ?? true}
            ariaLabel="Forward my port (UPnP)"
            disabled={!settings}
            onChange={(v) => patch({ torrent_upnp: v })}
          />
        </div>

        <div className="setting-row">
          <div>
            <div className="setting-label">Seed ratio limit</div>
            <div className="dim">
              Stop seeding once a torrent has uploaded this much of what it
              downloaded.
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

        <div className="setting-row">
          <div>
            <div className="setting-label">Magnet timeout</div>
            <div className="dim">
              How long to wait for a magnet's metadata before giving up.
            </div>
          </div>
          <Select
            value={String(settings?.magnet_timeout_secs ?? 180)}
            ariaLabel="Magnet timeout"
            caret
            disabled={!settings}
            onChange={(v) => patch({ magnet_timeout_secs: Number(v) })}
            options={MAGNET_TIMEOUT_OPTIONS.map((n) => ({
              value: String(n),
              label: secondsLabel(n),
            }))}
          />
        </div>

        <div className="setting-row">
          <div>
            <div className="setting-label">Tracker scrape interval</div>
            <div className="dim">
              How often to re-check trackers for seeder and leecher counts.
            </div>
          </div>
          <Select
            value={String(settings?.scrape_interval_secs ?? 90)}
            ariaLabel="Tracker scrape interval"
            caret
            disabled={!settings}
            onChange={(v) => patch({ scrape_interval_secs: Number(v) })}
            options={SCRAPE_INTERVAL_OPTIONS.map((n) => ({
              value: String(n),
              label: secondsLabel(n),
            }))}
          />
        </div>

        <div className="setting-row">
          <div>
            <div className="setting-label">Peer connection timeout</div>
            <div className="dim">
              How long to wait on a peer's handshake before dropping it.
            </div>
          </div>
          <Select
            value={String(settings?.peer_connect_timeout_secs ?? 10)}
            ariaLabel="Peer connection timeout"
            caret
            disabled={!settings}
            onChange={(v) => patch({ peer_connect_timeout_secs: Number(v) })}
            options={PEER_CONNECT_OPTIONS.map((n) => ({
              value: String(n),
              label: secondsLabel(n),
            }))}
          />
        </div>

        <div className="setting-row">
          <div>
            <div className="setting-label">Listen port fallback range</div>
            <div className="dim">
              How many ports past the listen port to try if it's busy.
            </div>
          </div>
          <Select
            value={String(settings?.torrent_port_span ?? 20)}
            ariaLabel="Listen port fallback range"
            caret
            disabled={!settings}
            onChange={(v) => patch({ torrent_port_span: Number(v) })}
            options={PORT_SPAN_OPTIONS.map((n) => ({
              value: String(n),
              label: n === 0 ? "Just the one port" : `+${n} ports`,
            }))}
          />
        </div>
          </>
        )}
      </div>

      <div className="card">
        <div className="card-title">Browser integration</div>
        <p className="dim">
          Send downloads straight from your browser with the companion extension,
          over a local endpoint bound to <code>127.0.0.1</code>.
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
              The port the extension connects to. Applies after a restart.
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
              Paste this into the extension to pair it with moin.
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
              className="btn"
              onClick={copyToken}
              disabled={!settings?.rpc_token}
            >
              {tokenCopied ? "Copied" : "Copy"}
            </button>
            <button className="btn" onClick={regenerateToken} disabled={!settings}>
              Regenerate
            </button>
          </div>
        </div>
      </div>

      <div className="card">
        <div className="card-title">External tools</div>
        <p className="dim">
          Optional binaries moin can use in place of its built-in engines.
        </p>

        <div className="tool-row">
          <div>
            <div className="setting-label">aria2c</div>
            <ToolState tool={tool} fetching={fetching} error={toolError} />
          </div>
          <div className="tool-actions">
            {tool?.can_fetch && (
              <button
                className="btn"
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
              className="btn"
              onClick={pickBinary}
              disabled={fetching !== null}
            >
              Use my binary…
            </button>
            {tool?.source === "override" && (
              <button
                className="btn danger"
                onClick={clearBinary}
                disabled={fetching !== null}
              >
                Clear
              </button>
            )}
          </div>
        </div>
      </div>

      <AboutCard />

      <div className="settings-credit dim">
        Developed by{" "}
        <button
          className="link-btn"
          onClick={() => openUrl(AUTHOR_URL).catch(() => {})}
        >
          Exxvius
        </button>
        <Heart className="credit-heart" size={13} strokeWidth={2.25} />
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

/** The About card: an update check against GitHub plus a short credit + license
 *  line. "Get the update" opens the installer (or the release page) — a running app
 *  can't overwrite itself, so the actual swap is the installer's job. */
function AboutCard() {
  const [version, setVersion] = useState("");
  const [update, setUpdate] = useState<UpdateInfo | null>(null);
  const [checking, setChecking] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    api
      .appInfo()
      .then((i) => setVersion(i.version))
      .catch(() => {});
  }, []);

  const check = async () => {
    setChecking(true);
    setError(null);
    try {
      setUpdate(await api.checkUpdate());
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setChecking(false);
    }
  };

  const status = error
    ? error
    : !update
      ? "Check GitHub for a newer version."
      : update.available
        ? `Version ${update.latest} is available.`
        : "You're on the latest version.";

  return (
    <div className="card">
      <div className="card-title">About</div>

      <div className="setting-row">
        <div>
          <div className="setting-label">moin{version ? ` ${version}` : ""}</div>
          <div className={`dim${error ? " update-error" : ""}`}>{status}</div>
        </div>
        {update?.available ? (
          <button
            className="btn"
            onClick={() => openUrl(update.asset_url ?? update.page_url).catch(() => {})}
          >
            Get the update
          </button>
        ) : (
          <button className="btn" onClick={check} disabled={checking}>
            {checking ? "Checking…" : "Check for updates"}
          </button>
        )}
      </div>

      {update?.available && update.notes && (
        <pre className="update-notes">{update.notes}</pre>
      )}

      <p className="dim about-note">
        moin is free and open source under the MIT License. Its built-in
        BitTorrent engine is powered by{" "}
        <button className="link-btn" onClick={() => openUrl(RQBIT_URL).catch(() => {})}>
          librqbit
        </button>
        . External tools — aria2c, yt-dlp, and FFmpeg — aren't bundled: moin
        fetches them on demand, and each stays under its own license.
      </p>
    </div>
  );
}
