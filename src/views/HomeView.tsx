import { useState } from "react";
import { useStore } from "../lib/store";

interface Props {
  onAdded: () => void;
}

export function HomeView({ onAdded }: Props) {
  const store = useStore();
  const [url, setUrl] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const submit = async () => {
    const value = url.trim();
    if (!value || busy) return;
    setBusy(true);
    setError(null);
    try {
      await store.add(value);
      setUrl("");
      onAdded();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
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
    </div>
  );
}
