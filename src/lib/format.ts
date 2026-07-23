// Human-friendly formatting for sizes, speeds, and time.

const UNITS = ["B", "KB", "MB", "GB", "TB", "PB"];

export function formatBytes(n: number | null | undefined): string {
  if (n == null) return "—";
  let v = n;
  let i = 0;
  while (v >= 1024 && i < UNITS.length - 1) {
    v /= 1024;
    i++;
  }
  const digits = i === 0 ? 0 : v < 10 ? 1 : 0;
  return `${v.toFixed(digits)} ${UNITS[i]}`;
}

export function formatSpeed(bytesPerSec: number): string {
  if (!bytesPerSec || bytesPerSec <= 0) return "—";
  return `${formatBytes(bytesPerSec)}/s`;
}

/** Seconds remaining, given bytes left and current speed. */
export function formatEta(remaining: number | null, speed: number): string {
  if (remaining == null || speed <= 0) return "—";
  let s = Math.round(remaining / speed);
  // Anything beyond ~99 days is meaningless (a trickle at a few B/s) — show ∞.
  if (s > 99 * 86400) return "∞";
  if (s < 60) return `${s}s`;
  const d = Math.floor(s / 86400);
  s -= d * 86400;
  const h = Math.floor(s / 3600);
  s -= h * 3600;
  const m = Math.floor(s / 60);
  s -= m * 60;
  if (d > 0) return `${d}d ${h}h`;
  if (h > 0) return `${h}h ${m}m`;
  return `${m}m ${s}s`;
}

/** 0–100, or null when the total is unknown. */
export function percent(received: number, total: number | null): number | null {
  if (!total || total <= 0) return null;
  return Math.min(100, (received / total) * 100);
}

/** Compact duration from milliseconds, e.g. "2h 15m", "3m 5s", "12s". */
export function formatDuration(ms: number): string {
  if (!ms || ms <= 0) return "—";
  let s = Math.round(ms / 1000);
  const h = Math.floor(s / 3600);
  s -= h * 3600;
  const m = Math.floor(s / 60);
  s -= m * 60;
  if (h > 0) return `${h}h ${m}m`;
  if (m > 0) return `${m}m ${s}s`;
  return `${s}s`;
}

/** Compact local date + time, e.g. "Jul 22, 2:30 PM". */
export function formatDate(ms: number): string {
  const d = new Date(ms);
  const date = d.toLocaleDateString(undefined, {
    month: "short",
    day: "numeric",
  });
  const time = d.toLocaleTimeString(undefined, {
    hour: "numeric",
    minute: "2-digit",
  });
  return `${date}, ${time}`;
}
