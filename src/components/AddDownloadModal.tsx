import { useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { ConfirmModal } from "./ConfirmModal";
import { Select } from "./Select";
import { CategoryIcon } from "./CategoryIcon";
import { useStore } from "../lib/store";
import { api } from "../lib/api";

interface Props {
  onClose: () => void;
}

/** The add-a-download form as a modal, opened from the Downloads header. Pastes a
 *  direct link, optionally files it under a category (auto-suggested from the
 *  URL), and closes once the download is queued. */
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
  // clearing the box re-enables auto-suggest.
  useEffect(() => {
    const value = url.trim();
    if (!value) {
      setTouched(false);
      setCategory("");
      return;
    }
    if (touched) return;
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
      await store.add(value, category || null);
      onClose();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
      setBusy(false);
    }
  };

  const submit = async () => {
    const value = url.trim();
    if (!value || busy) return;
    // Same URL already in the active list? Ask before adding a second copy.
    // Archived downloads are effectively gone, so they don't count.
    if (store.all.some((t) => t.url === value && !t.archived)) {
      setDupUrl(value);
      return;
    }
    await doAdd(value);
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
          type="url"
          inputMode="url"
          placeholder="https://example.com/file.zip"
          value={url}
          spellCheck={false}
          onChange={(e) => setUrl(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") submit();
          }}
        />
        <p className="dim add-modal-hint">
          Paste a direct link. Torrents and media sites are coming next.
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
