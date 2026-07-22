// The launch-prompt window. Reports the user's choice back to the background
// worker (with this window's id so it can match the right prompt), then lets the
// worker close the window.

const B = globalThis.browser ?? globalThis.chrome;

const params = new URLSearchParams(location.search);
const file = params.get("file");
if (file) document.getElementById("file").textContent = file;

async function decide(launch) {
  const win = await B.windows.getCurrent();
  B.runtime.sendMessage({ type: "moin-launch-decision", launch, windowId: win.id });
}

document.getElementById("launch").addEventListener("click", () => {
  // Trigger the moin:// handler from this real click, then report the decision.
  window.location.href = "moin://launch";
  decide(true);
});
document.getElementById("cancel").addEventListener("click", () => decide(false));
