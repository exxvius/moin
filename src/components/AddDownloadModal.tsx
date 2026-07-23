import { useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { open } from "@tauri-apps/plugin-dialog";
import { ConfirmModal } from "./ConfirmModal";
import { Select } from "./Select";
import { CategoryIcon } from "./CategoryIcon";
import { useStore } from "../lib/store";
import { api } from "../lib/api";

interface Props {
  onClose: () => void;
}

/** A magnet link — routed to the torrent engine rather than the HTTP one. */
function isMagnet(value: string): boolean {
  return value.trim().toLowerCase().startsWith("magnet:");
}

/** The add-a-download form as a modal, opened from the Downloads header. Takes a
 *  direct link, a magnet URI, or a picked `.torrent` file, optionally files it
 *  under a category (auto-suggested), and closes once the download is queued. */
export function AddDownloadModal({ onClose }: Props) {
  const store = useStore();
  const inputRef = useRef<HTMLInputElement>(null);
  const [url, setUrl] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  // Set when the entered URL is already in the queue and we need to confirm.
  const [dupUrl, setDupUrl] = useState<string | null>(null);
  // "" = uncategorized. Auto-filled from the URL until the user picks manually.
  const [category, setCategory] = useState("");
  const [touched, setTouched] = useState(false);

  // Land the cursor in the URL box on open.
  useEffect(() => inputRef.current?.focus(), []);

  // Esc closes the form (unless the duplicate confirm is up — it owns Esc then).
  useEffect(() => {
    if (dupUrl) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [dupUrl, onClose]);

  // Suggest a category from the URL (debounced). A manual pick (touched) sticks;
  // clearing the box re-enables auto-suggest. Magnets have no URL to read, so the
  // engine auto-files them by torrent name at add time instead.
  useEffect(() => {
    const value = url.trim();
    if (!value) {
      setTouched(false);
      setCategory("");
      return;
    }
    if (touched || isMagnet(value)) return;
    const t = setTimeout(() => {
      api
        .suggestCategory(value)
        .then((id) => {
          if (!touched) setCategory(id ?? "");
        })
        .catch(() => {});
    }, 300);
    return () => clearTimeout(t);
  }, [url, touched]);

  const doAdd = async (value: string) => {
    setBusy(true);
    setError(null);
    try {
      if (isMagnet(value)) await store.addTorrent(value, category || null);
      else await store.add(value, category || null);
      onClose();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
      setBusy(false);
    }
  };

  const submit = async () => {
    const value = url.trim();
    if (!value || busy) return;
    // Same source already in the active list? Ask before adding a second copy.
    // Archived downloads are effectively gone, so they don't count.
    if (store.all.some((t) => t.url === value && !t.archived)) {
      setDupUrl(value);
      return;
    }
    await doAdd(value);
  };

  // Pick a local .torrent file via the OS dialog and queue it straight away.
  const pickTorrent = async () => {
    if (busy) return;
    const picked = await open({
      multiple: false,
      directory: false,
      title: "Choose a .torrent file",
      filters: [{ name: "Torrent", extensions: ["torrent"] }],
    });
    if (typeof picked !== "string") return;
    setBusy(true);
    setError(null);
    try {
      await store.addTorrent(picked, category || null);
      onClose();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
      setBusy(false);
    }
  };

  return createPortal(
    <div className="modal-backdrop" onClick={onClose}>
      <div
        className="modal add-modal"
        role="dialog"
        aria-modal="true"
        aria-label="Add a download"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="modal-title">Add a download</div>

        <input
          ref={inputRef}
          className="add-input selectable"
          type="text"
          inputMode="url"
          placeholder="Paste a link or magnet…"
          value={url}
          spellCheck={false}
          onChange={(e) => setUrl(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") submit();
          }}
        />
        <p className="dim add-modal-hint">
          Paste a direct link or a magnet, or{" "}
          <button
            type="button"
            className="link-btn"
            onClick={pickTorrent}
            disabled={busy}
          >
            choose a .torrent file
          </button>
          . Media sites are coming next.
        </p>
        {error && <p className="dl-error">{error}</p>}

        <div className="add-modal-foot">
          {store.categories.length > 0 && (
            <Select
              value={category}
              ariaLabel="Category"
              caret
              onChange={(v) => {
                setCategory(v);
                setTouched(true);
              }}
              options={[
                { value: "", label: "Uncategorized" },
                ...store.categories.map((c) => ({
                  value: c.id,
                  label: (
                    <span className="accent-option">
                      <CategoryIcon icon={c.icon} color={c.color} size={16} />
                      {c.name}
                    </span>
                  ),
                })),
              ]}
            />
          )}
          <div className="add-modal-actions">
            <button className="dl-btn" onClick={onClose}>
              Cancel
            </button>
            <button
              className="btn-primary"
              onClick={submit}
              disabled={busy || url.trim().length === 0}
            >
              {busy ? "Adding…" : "Download"}
            </button>
          </div>
        </div>
      </div>

      {dupUrl && (
        <ConfirmModal
          title="Already in the queue"
          message={
            <>
              This download is already in your list. Add it again anyway? A
              numbered copy will be saved so it won't overwrite the existing file.
            </>
          }
          confirmLabel="Add anyway"
          cancelLabel="Cancel"
          onCancel={() => setDupUrl(null)}
          onConfirm={() => {
            const value = dupUrl;
            setDupUrl(null);
            doAdd(value);
          }}
        />
      )}
    </div>,
    document.body,
  );
}
