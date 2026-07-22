// Options page logic. Reuses the globals from shared.js (loaded first): `B`,
// `loadConfig`, `saveConfig`, `verifyMoin`. Settings save as they change.

const portEl = document.getElementById("port");
const tokenEl = document.getElementById("token");
const enabledEl = document.getElementById("enabled");
const autoEl = document.getElementById("autoCapture");
const testEl = document.getElementById("test");
const statusEl = document.getElementById("status");

async function init() {
  const cfg = await loadConfig();
  portEl.value = String(cfg.port);
  tokenEl.value = cfg.token;
  enabledEl.checked = cfg.enabled;
  autoEl.checked = cfg.autoCapture;
}

function setStatus(text, kind) {
  statusEl.textContent = text;
  statusEl.className = `status ${kind ?? "dim"}`;
}

// Persist the port only when it's a valid TCP port; otherwise snap it back.
async function commitPort() {
  const n = Number(portEl.value);
  if (Number.isInteger(n) && n >= 1 && n <= 65535) {
    await saveConfig({ port: n });
  } else {
    const cfg = await loadConfig();
    portEl.value = String(cfg.port);
  }
}

portEl.addEventListener("change", commitPort);
tokenEl.addEventListener("change", () => saveConfig({ token: tokenEl.value.trim() }));
enabledEl.addEventListener("change", () => saveConfig({ enabled: enabledEl.checked }));
autoEl.addEventListener("change", () => saveConfig({ autoCapture: autoEl.checked }));

testEl.addEventListener("click", async () => {
  await commitPort();
  const cfg = { port: Number(portEl.value), token: tokenEl.value.trim() };
  if (!cfg.token) {
    setStatus("Paste the access token first", "bad");
    return;
  }
  setStatus("Testing…", "dim");
  const result = await verifyMoin(cfg);
  if (result === "ok") setStatus("Connected to moin ✓", "ok");
  else if (result === "bad-token") setStatus("Reached moin, but the token is wrong", "bad");
  else setStatus("Couldn't reach moin — is it running?", "bad");
});

init();
