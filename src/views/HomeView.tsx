import { useEffect, useState } from "react";
import { ConfirmModal } from "../components/ConfirmModal";
import { Select } from "../components/Select";
import { useStore } from "../lib/store";
import { api } from "../lib/api";
import { CategoryIcon } from "../components/CategoryIcon";

interface Props {
  onAdded: () => void;
}

export function HomeView({ onAdded }: Props) {
  const store = useStore();
  const [url, setUrl] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  // Set when the entered URL is already in the queue and we need to confirm.
  const [dupUrl, setDupUrl] = useState<string | null>(null);
  // "" = uncategorized. Auto-filled from the URL until the user picks manually.
  const [category, setCategory] = useState("");
  const [touched, setTouched] = useState(false);

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
      setUrl("");
      setCategory("");
      setTouched(false);
      onAdded();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
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

  return (
    <div className="view">
      <div className="view-head">
        <h2>Add a download</h2>
        <p>Paste a direct link. Torrents and media sites are coming next.</p>
      </div>

      <div className="card">
        <div className="add-row">
          <input
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
          <button
            className="btn-primary"
            onClick={submit}
            disabled={busy || url.trim().length === 0}
          >
            {busy ? "Adding…" : "Download"}
          </button>
        </div>
        {error && <p className="dl-error">{error}</p>}
      </div>

      {dupUrl && (
        <ConfirmModal
          title="Already in the queue"
          message={
            <>
              This download is already in your list. Add it again anyway? A
              numbered copy will be saved so it won't overwrite the existing
              file.
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
    </div>
  );
}
