import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { formatBytes, formatEta, formatSpeed, percent } from "../lib/format";
import type { Task, TaskStatus } from "../lib/types";

const STATUS_LABEL: Record<TaskStatus, string> = {
  queued: "Queued",
  connecting: "Connecting",
  downloading: "Downloading",
  paused: "Paused",
  completed: "Done",
  failed: "Failed",
  canceled: "Canceled",
};

const STATUS_CLASS: Record<TaskStatus, string> = {
  queued: "dim",
  connecting: "accent",
  downloading: "accent",
  paused: "warn",
  completed: "ok",
  failed: "bad",
  canceled: "faint",
};

interface Props {
  task: Task;
  speed?: number;
  onPause: (id: string) => void;
  onResume: (id: string) => void;
  onCancel: (id: string) => void;
  onRemove: (id: string) => void;
}

export function DownloadItem({
  task,
  speed = 0,
  onPause,
  onResume,
  onCancel,
  onRemove,
}: Props) {
  const pct = percent(task.received, task.total);
  const remaining = task.total != null ? task.total - task.received : null;
  const busy = task.status === "downloading" || task.status === "connecting";
  const indeterminate =
    task.status === "queued" ||
    task.status === "connecting" ||
    (task.status === "downloading" && pct == null);

  return (
    <div className="dl-item">
      <div className="dl-head">
        <span className="dl-name" title={task.dest}>
          {task.filename}
        </span>
        <span className={`dl-badge ${STATUS_CLASS[task.status]}`}>
          {STATUS_LABEL[task.status]}
        </span>
      </div>

      {task.status !== "completed" && (
        <div className={`dl-bar${indeterminate ? " indeterminate" : ""}`}>
          <i style={pct != null ? { width: `${pct}%` } : undefined} />
        </div>
      )}

      <div className="dl-meta">
        <span className="dl-size">
          {formatBytes(task.received)}
          {task.total != null ? ` / ${formatBytes(task.total)}` : ""}
        </span>
        {task.status === "downloading" && (
          <>
            <span>{formatSpeed(speed)}</span>
            <span>{formatEta(remaining, speed)} left</span>
          </>
        )}
        {task.error && <span className="dl-error">{task.error}</span>}

        <span className="dl-actions">
          {busy && (
            <button className="dl-btn" onClick={() => onPause(task.id)}>
              Pause
            </button>
          )}
          {task.status === "queued" && (
            <button className="dl-btn" onClick={() => onPause(task.id)}>
              Hold
            </button>
          )}
          {(task.status === "paused" ||
            task.status === "failed" ||
            task.status === "canceled") && (
            <button className="dl-btn" onClick={() => onResume(task.id)}>
              {task.status === "paused" ? "Resume" : "Retry"}
            </button>
          )}
          {task.status === "completed" && (
            <button
              className="dl-btn"
              onClick={() => revealItemInDir(task.dest).catch(() => {})}
            >
              Show in folder
            </button>
          )}
          {task.status !== "completed" ? (
            <button
              className="dl-btn danger"
              onClick={() => onCancel(task.id)}
            >
              Cancel
            </button>
          ) : (
            <button
              className="dl-btn danger"
              onClick={() => onRemove(task.id)}
            >
              Remove
            </button>
          )}
        </span>
      </div>
    </div>
  );
}
