// Transport to the engine daemon.
//
// The engine runs in its own process; the shell tells us once where it is and how
// to authenticate, and everything after that is plain HTTP to loopback. Commands
// are POSTs named exactly as the old Tauri commands were, so `api.ts` reads much
// the same as it always did.
//
// Events come over one shared SSE connection rather than one per subscriber:
// every connection is a client as far as the daemon's lifecycle is concerned, and
// each gets its own snapshot, so opening several would be wasteful and confusing.
//
// EventSource can't set an Authorization header, so the stream is read by hand off
// fetch — which is a few lines of framing and keeps the token out of the URL.

import { invoke } from "@tauri-apps/api/core";

interface Endpoint {
  port: number;
  token: string;
}

let endpointPromise: Promise<Endpoint> | null = null;

/** Asked for once; the shell has already proved the engine is up by this point. */
function endpoint(): Promise<Endpoint> {
  endpointPromise ??= invoke<Endpoint>("engine_endpoint");
  return endpointPromise;
}

/** Call an engine command. Rejects with the engine's own message, like `invoke`. */
export async function call<T>(
  name: string,
  args: Record<string, unknown> = {},
): Promise<T> {
  const { port, token } = await endpoint();
  const response = await fetch(`http://127.0.0.1:${port}/api/${name}`, {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
      Authorization: `Bearer ${token}`,
    },
    body: JSON.stringify(args),
  });

  if (!response.ok) {
    throw new Error(await errorMessage(response, name));
  }
  const text = await response.text();
  return (text ? JSON.parse(text) : undefined) as T;
}

/** Engine failures arrive as `{"error": "…"}`; fall back to the status if not. */
async function errorMessage(response: Response, name: string): Promise<string> {
  try {
    const body = await response.json();
    if (body && typeof body.error === "string") return body.error;
  } catch {
    // not JSON — fall through to the generic message
  }
  return `${name} failed (${response.status})`;
}

// ---- Events -----------------------------------------------------------------

type Handler = (payload: never) => void;

const handlers = new Map<string, Set<Handler>>();
let streaming = false;

/** Subscribe to one engine event. Returns an unsubscribe function. */
export function onEngineEvent<T>(
  name: string,
  handler: (payload: T) => void,
): () => void {
  let group = handlers.get(name);
  if (!group) {
    group = new Set();
    handlers.set(name, group);
  }
  const entry = handler as Handler;
  group.add(entry);
  void startStream();
  return () => {
    group.delete(entry);
  };
}

/** Reconnect backoff, in ms. The daemon only goes away if it crashed or if it
 *  decided every client had left — either way, don't hammer it. */
const RETRY_MS = [250, 500, 1000, 2000, 5000];

async function startStream(): Promise<void> {
  if (streaming) return;
  streaming = true;

  let attempt = 0;
  for (;;) {
    try {
      await readStream();
      // A clean end means the daemon closed the stream; reconnect like any drop.
      attempt = 0;
    } catch (error) {
      console.error("engine event stream dropped", error);
    }
    const wait = RETRY_MS[Math.min(attempt, RETRY_MS.length - 1)];
    attempt += 1;
    await new Promise((resolve) => setTimeout(resolve, wait));
  }
}

async function readStream(): Promise<void> {
  const { port, token } = await endpoint();
  const response = await fetch(`http://127.0.0.1:${port}/events`, {
    headers: { Authorization: `Bearer ${token}` },
  });
  if (!response.ok || !response.body) {
    throw new Error(`event stream refused (${response.status})`);
  }

  const reader = response.body.getReader();
  const decoder = new TextDecoder();
  let buffer = "";

  for (;;) {
    const { done, value } = await reader.read();
    if (done) return;
    // Normalize line endings so the frame split doesn't depend on them.
    buffer += decoder.decode(value, { stream: true }).replace(/\r\n/g, "\n");
    // A blank line terminates an SSE frame.
    let split = buffer.indexOf("\n\n");
    while (split !== -1) {
      dispatch(buffer.slice(0, split));
      buffer = buffer.slice(split + 2);
      split = buffer.indexOf("\n\n");
    }
  }
}

function dispatch(frame: string): void {
  let name = "";
  const data: string[] = [];
  for (const line of frame.split("\n")) {
    // Comment lines are the keep-alive ping; nothing to do with them.
    if (line.startsWith(":")) continue;
    if (line.startsWith("event:")) name = line.slice(6).trim();
    else if (line.startsWith("data:")) data.push(line.slice(5).trimStart());
  }
  const group = name && handlers.get(name);
  if (!group || group.size === 0) return;

  let payload: unknown;
  try {
    payload = JSON.parse(data.join("\n"));
  } catch {
    console.error(`couldn't parse the payload of ${name}`);
    return;
  }
  for (const handler of group) (handler as (p: unknown) => void)(payload);
}
