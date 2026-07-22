import {
  useEffect,
  useState,
  type CSSProperties,
  type KeyboardEvent as ReactKeyboardEvent,
  type PointerEvent as ReactPointerEvent,
} from "react";
import { createPortal } from "react-dom";
import { open } from "@tauri-apps/plugin-dialog";
import { Select } from "../components/Select";
import { Switch } from "../components/Switch";
import { ConfirmModal } from "../components/ConfirmModal";
import { DragHandleIcon } from "../components/icons";
import { useSortableList } from "../lib/useSortableList";
import { api } from "../lib/api";
import { useStore } from "../lib/store";
import { ACCENTS } from "../lib/accent";
import { ADD_METHOD_LABEL, categorySwatch, ruleSummary } from "../lib/categories";
import { CATEGORY_ICONS } from "../lib/categoryIcons";
import { CategoryIcon } from "../components/CategoryIcon";
import type {
  AddMethodKind,
  Category,
  Settings,
  Trigger,
  TriggerType,
} from "../lib/types";

const MIB = 1024 * 1024;

// Content triggers offered in the builder, with friendly labels. (How a download
// arrived is a separate axis — see the Sources section.)
const TRIGGER_TYPES: { value: TriggerType; label: string }[] = [
  { value: "extension", label: "File type" },
  { value: "size", label: "File size" },
  { value: "url-pattern", label: "URL pattern" },
  { value: "name-pattern", label: "Filename pattern" },
];

// Add-methods that exist today; watch methods arrive with automation.
const LIVE_SOURCES: AddMethodKind[] = ["manual-link", "manual-torrent"];

function blankCategory(): Category {
  return {
    id: "",
    name: "",
    color: "blue",
    icon: null,
    save_dir: null,
    sources: [],
    triggers: [],
    fallback_download: false,
    order: 0,
  };
}

function defaultTrigger(type: TriggerType): Trigger {
  switch (type) {
    case "extension":
      return { type, exts: [] };
    case "size":
      return { type, min: null, max: null };
    case "url-pattern":
      return { type, patterns: [] };
    case "name-pattern":
      return { type, patterns: [] };
  }
}

/** Move an array item from one index to another, returning a new array. */
function reorder<T>(arr: T[], from: number, to: number): T[] {
  const next = [...arr];
  const [moved] = next.splice(from, 1);
  next.splice(to, 0, moved);
  return next;
}

export function CategoriesView() {
  const store = useStore();
  const cats = store.categories;
  const [editing, setEditing] = useState<Category | null>(null);
  const [confirmDelete, setConfirmDelete] = useState<Category | null>(null);
  // App settings, for the uncategorized default save folder (download_dir).
  const [settings, setSettings] = useState<Settings | null>(null);

  useEffect(() => {
    api.getSettings().then(setSettings).catch(() => {});
  }, []);

  const setUncatFolder = async (dir: string | null) => {
    if (!settings) return;
    const next = { ...settings, download_dir: dir };
    setSettings(next);
    api.saveSettings(next).catch(() => {});
  };

  const pickUncatFolder = async () => {
    const picked = await open({
      directory: true,
      multiple: false,
      title: "Default folder for uncategorized downloads",
    });
    if (typeof picked === "string") setUncatFolder(picked);
  };

  const save = async (draft: Category) => {
    const list = draft.id
      ? await api.updateCategory(draft)
      : await api.createCategory(draft);
    store.setCategories(list);
    setEditing(null);
  };

  const remove = async (c: Category) => {
    store.setCategories(await api.deleteCategory(c.id));
    setConfirmDelete(null);
  };

  // Drag-to-reorder the category cards: the grabbed card follows the cursor
  // while its neighbors slide aside; the new order is optimistic during the drag
  // and persisted on drop. Cards carry stable ids, so React reuses their nodes.
  const { containerRef, startDrag } = useSortableList<HTMLDivElement>({
    onReorder: (from, to) =>
      store.setCategories((prev) => reorder(prev, from, to)),
    onDrop: () => {
      const el = containerRef.current;
      if (!el) return;
      const ids = (Array.from(el.children) as HTMLElement[])
        .map((c) => c.dataset.id ?? "")
        .filter(Boolean);
      if (ids.length) {
        api.reorderCategories(ids).then(store.setCategories).catch(() => {});
      }
    },
  });

  return (
    <div className="view categories">
      <div className="view-head cat-head">
        <div>
          <h2>Categories</h2>
          <p>
            File downloads into buckets by rule. A download joins a category when
            all of its triggers match; when several match, the one higher in this
            list wins.
          </p>
        </div>
        <button
          className="btn-primary"
          onClick={() => setEditing(blankCategory())}
        >
          New category
        </button>
      </div>

      <div className="card cat-panel">
        <div className="cat-scroll">
          {cats.length === 0 ? (
            <div className="dl-empty">
              <div className="card-title">No categories yet</div>
              <p className="dim">
                Create one to auto-file downloads by type, size, URL, and more.
              </p>
            </div>
          ) : (
            <div className="cat-list" ref={containerRef}>
              {cats.map((c, i) => (
                <div
                  className="cat-card"
                  key={c.id}
                  data-id={c.id}
                  style={{ "--cat": categorySwatch(c.color) } as CSSProperties}
                  onPointerDown={(e) => {
                    // Drag from anywhere on the card, but let the action buttons
                    // (Edit/Delete) still click normally.
                    if ((e.target as HTMLElement).closest("button")) return;
                    startDrag(i, e);
                  }}
                >
                  <span className="drag-handle" aria-hidden>
                    <DragHandleIcon size={18} />
                  </span>
                  <span className="cat-card-icon">
                    <CategoryIcon icon={c.icon} color={c.color} size={20} />
                  </span>
                  <div className="cat-row-main">
                    <div className="setting-label">{c.name || "Untitled"}</div>
                    <div className="dim cat-sub">{ruleSummary(c)}</div>
                    {c.save_dir && (
                      <div className="dim cat-sub path">Saves to {c.save_dir}</div>
                    )}
                  </div>
                  <div className="cat-row-actions">
                    <button className="dl-btn" onClick={() => setEditing(c)}>
                      Edit
                    </button>
                    <button
                      className="dl-btn danger"
                      onClick={() => setConfirmDelete(c)}
                    >
                      Delete
                    </button>
                  </div>
                </div>
              ))}
            </div>
          )}
        </div>

        {/* Pinned: where uncategorized downloads land. Only its save folder is
            editable — no name, color, icon, or rules. */}
        <div className="cat-card cat-card-uncat">
          <span className="cat-card-icon">
            <CategoryIcon icon="inbox" color="" size={20} />
          </span>
          <div className="cat-row-main">
            <div className="setting-label">Uncategorized</div>
            <div className="dim cat-sub path">
              {settings?.download_dir
                ? `Saves to ${settings.download_dir}`
                : "Saves to your default download folder"}
            </div>
          </div>
          <div className="cat-row-actions">
            <button
              className="dl-btn"
              disabled={!settings}
              onClick={pickUncatFolder}
            >
              {settings?.download_dir ? "Change…" : "Choose…"}
            </button>
            {settings?.download_dir && (
              <button
                className="dl-btn danger"
                onClick={() => setUncatFolder(null)}
              >
                Clear
              </button>
            )}
          </div>
        </div>
      </div>

      {confirmDelete && (
        <ConfirmModal
          title="Delete category"
          message={
            <>
              Delete “{confirmDelete.name || "Untitled"}”? Its downloads stay in
              your list — they just become uncategorized.
            </>
          }
          confirmLabel="Delete"
          onConfirm={() => remove(confirmDelete)}
          onCancel={() => setConfirmDelete(null)}
        />
      )}

      {editing && (
        <CategoryEditor
          initial={editing}
          onSave={save}
          onCancel={() => setEditing(null)}
        />
      )}
    </div>
  );
}

interface EditorProps {
  initial: Category;
  onSave: (c: Category) => void;
  onCancel: () => void;
}

function CategoryEditor({ initial, onSave, onCancel }: EditorProps) {
  const [draft, setDraft] = useState<Category>(initial);
  const patch = (change: Partial<Category>) =>
    setDraft((d) => ({ ...d, ...change }));

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onCancel();
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [onCancel]);

  // Stable keys parallel to the triggers, so rows keep their identity across a
  // reorder (the Rust model has no per-trigger id).
  const [keys, setKeys] = useState<string[]>(() =>
    initial.triggers.map(() => crypto.randomUUID()),
  );

  const setTrigger = (idx: number, t: Trigger) =>
    patch({ triggers: draft.triggers.map((cur, i) => (i === idx ? t : cur)) });
  const removeTrigger = (idx: number) => {
    patch({ triggers: draft.triggers.filter((_, i) => i !== idx) });
    setKeys((k) => k.filter((_, i) => i !== idx));
  };
  const addTrigger = () => {
    patch({ triggers: [...draft.triggers, defaultTrigger("extension")] });
    setKeys((k) => [...k, crypto.randomUUID()]);
  };

  // Drag-to-reorder triggers, same behavior as the category cards.
  const triggerSort = useSortableList<HTMLDivElement>({
    onReorder: (from, to) => {
      setDraft((d) => ({ ...d, triggers: reorder(d.triggers, from, to) }));
      setKeys((k) => reorder(k, from, to));
    },
  });

  const pickFolder = async () => {
    const picked = await open({
      directory: true,
      multiple: false,
      title: "Choose a save folder",
    });
    if (typeof picked === "string") patch({ save_dir: picked });
  };

  const canSave = draft.name.trim().length > 0;

  return createPortal(
    <div className="modal-backdrop" onClick={onCancel}>
      <div
        className="modal modal-lg"
        role="dialog"
        aria-modal="true"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="modal-title">
          {draft.id ? "Edit category" : "New category"}
        </div>

        <div className="modal-scroll">
          <div className="setting-row">
            <div className="setting-label">Name</div>
            <input
              className="add-input selectable cat-name"
              type="text"
              placeholder="Movies, Shows, Music…"
              value={draft.name}
              spellCheck={false}
              onChange={(e) => patch({ name: e.target.value })}
            />
          </div>

          <div className="setting-row">
            <div className="setting-label">Color</div>
            <Select
              value={draft.color}
              ariaLabel="Category color"
              caret
              onChange={(v) => patch({ color: v })}
              options={[
                {
                  value: "",
                  label: (
                    <span className="accent-option">
                      <span className="accent-dot no-color" />
                      No color
                    </span>
                  ),
                },
                ...ACCENTS.map((a) => ({
                  value: a.id,
                  label: (
                    <span className="accent-option">
                      <span
                        className="accent-dot"
                        style={{ background: a.swatch }}
                      />
                      {a.label}
                    </span>
                  ),
                })),
              ]}
            />
          </div>

          <div className="cat-section">
            <div className="icon-head">
              <div className="setting-label">Icon</div>
              <button
                className="dl-btn"
                disabled={!draft.icon}
                onClick={() => patch({ icon: null })}
              >
                No icon
              </button>
            </div>
            <div className="dim">
              Optional — shows on the category in place of the color dot.
            </div>
            <div className="icon-grid" style={{ color: categorySwatch(draft.color) }}>
              {CATEGORY_ICONS.map(([id, Icon]) => (
                <button
                  key={id}
                  className={`icon-swatch${draft.icon === id ? " on" : ""}`}
                  title={id.replace(/-/g, " ")}
                  aria-label={id}
                  aria-pressed={draft.icon === id}
                  onClick={() => patch({ icon: id })}
                >
                  <Icon size={18} />
                </button>
              ))}
            </div>
          </div>

          <div className="setting-row">
            <div>
              <div className="setting-label">Save folder</div>
              <div className="dim">
                Where matching downloads land. Leave unset for the default folder.
              </div>
              {draft.save_dir && (
                <div className="dim path cat-savedir">{draft.save_dir}</div>
              )}
            </div>
            <div className="tool-actions">
              <button className="dl-btn" onClick={pickFolder}>
                {draft.save_dir ? "Change…" : "Choose…"}
              </button>
              {draft.save_dir && (
                <button
                  className="dl-btn danger"
                  onClick={() => patch({ save_dir: null })}
                >
                  Clear
                </button>
              )}
            </div>
          </div>

          <div className="cat-section">
            <div className="setting-label">Sources</div>
            <div className="dim">
              How the download arrived. Pick which to accept, or leave all off to
              match any source.
            </div>
            <div className="method-chips cat-sources">
              {LIVE_SOURCES.map((m) => {
                const on = draft.sources.includes(m);
                return (
                  <button
                    key={m}
                    className={`method-chip${on ? " on" : ""}`}
                    aria-pressed={on}
                    onClick={() =>
                      patch({
                        sources: on
                          ? draft.sources.filter((x) => x !== m)
                          : [...draft.sources, m],
                      })
                    }
                  >
                    {ADD_METHOD_LABEL[m]}
                  </button>
                );
              })}
            </div>
          </div>

          <div className="cat-section">
            <div className="setting-label">Triggers</div>
            <div className="dim">
              Conditions on the file itself. All must match. Type a value and
              press space to turn it into a chip; backspace edits the last one.
            </div>
            <div className="trigger-list" ref={triggerSort.containerRef}>
              {draft.triggers.map((t, i) => (
                <TriggerRow
                  key={keys[i]}
                  rowKey={keys[i]}
                  trigger={t}
                  onChange={(nt) => setTrigger(i, nt)}
                  onRemove={() => removeTrigger(i)}
                  onHandleDown={(e) => triggerSort.startDrag(i, e)}
                />
              ))}
            </div>
            <button className="dl-btn add-trigger" onClick={addTrigger}>
              Add trigger
            </button>
          </div>

          <div className="setting-row">
            <div>
              <div className="setting-label">Download even if triggers fail</div>
              <div className="dim">
                For automated sources (watched folders / URL lists, coming
                later): download non-matching items anyway, uncategorized,
                instead of skipping them. No effect on manual adds.
              </div>
            </div>
            <Switch
              checked={draft.fallback_download}
              ariaLabel="Download even if triggers fail"
              onChange={(v) => patch({ fallback_download: v })}
            />
          </div>
        </div>

        <div className="modal-actions">
          <button className="dl-btn" onClick={onCancel}>
            Cancel
          </button>
          <button
            className="btn-primary"
            disabled={!canSave}
            onClick={() => onSave(draft)}
          >
            {draft.id ? "Save" : "Create"}
          </button>
        </div>
      </div>
    </div>,
    document.body,
  );
}

interface TriggerRowProps {
  rowKey: string;
  trigger: Trigger;
  onChange: (t: Trigger) => void;
  onRemove: () => void;
  onHandleDown: (e: ReactPointerEvent) => void;
}

function TriggerRow({
  rowKey,
  trigger,
  onChange,
  onRemove,
  onHandleDown,
}: TriggerRowProps) {
  return (
    <div className="trigger-row" data-key={rowKey}>
      <span
        className="drag-handle"
        role="button"
        aria-label="Drag to reorder"
        onPointerDown={onHandleDown}
      >
        <DragHandleIcon size={18} />
      </span>
      <Select
        value={trigger.type}
        ariaLabel="Trigger type"
        caret
        onChange={(v) => onChange(defaultTrigger(v as TriggerType))}
        options={TRIGGER_TYPES}
      />
      <div className="trigger-fields">
        <TriggerFields trigger={trigger} onChange={onChange} />
      </div>
      <button className="dl-btn danger" aria-label="Remove trigger" onClick={onRemove}>
        Remove
      </button>
    </div>
  );
}

interface FieldsProps {
  trigger: Trigger;
  onChange: (t: Trigger) => void;
}

function TriggerFields({ trigger, onChange }: FieldsProps) {
  switch (trigger.type) {
    case "extension":
      return (
        <TokenInput
          values={trigger.exts}
          placeholder="pdf, epub, zip…"
          normalize={(s) => s.trim().replace(/^\./, "").toLowerCase()}
          onChange={(exts) => onChange({ type: "extension", exts })}
        />
      );
    case "url-pattern":
      return (
        <TokenInput
          values={trigger.patterns}
          placeholder="arxiv.org, *.pdf…"
          onChange={(patterns) => onChange({ type: "url-pattern", patterns })}
        />
      );
    case "name-pattern":
      return (
        <TokenInput
          values={trigger.patterns}
          placeholder="*.mkv, invoice*…"
          onChange={(patterns) => onChange({ type: "name-pattern", patterns })}
        />
      );
    case "size":
      return (
        <div className="size-fields">
          <input
            className="add-input selectable size-input"
            type="number"
            min={0}
            placeholder="min MB"
            value={trigger.min != null ? trigger.min / MIB : ""}
            onChange={(e) =>
              onChange({
                ...trigger,
                min: e.target.value === "" ? null : Number(e.target.value) * MIB,
              })
            }
          />
          <span className="dim">to</span>
          <input
            className="add-input selectable size-input"
            type="number"
            min={0}
            placeholder="max MB"
            value={trigger.max != null ? trigger.max / MIB : ""}
            onChange={(e) =>
              onChange({
                ...trigger,
                max: e.target.value === "" ? null : Number(e.target.value) * MIB,
              })
            }
          />
        </div>
      );
  }
}

interface TokenInputProps {
  values: string[];
  placeholder?: string;
  /** Clean a raw entry before it becomes a chip; a falsy result is dropped. */
  normalize?: (s: string) => string;
  onChange: (values: string[]) => void;
}

/** A chip/token field: type a value and press space (or Enter/comma) to turn it
 *  into a chip; backspace on an empty field pops the last chip back to text for
 *  editing; clicking a chip pops it back too. */
function TokenInput({ values, placeholder, normalize, onChange }: TokenInputProps) {
  const [text, setText] = useState("");
  const clean = (s: string) => (normalize ? normalize(s) : s.trim());

  const commit = () => {
    const v = clean(text);
    setText("");
    if (v && !values.includes(v)) onChange([...values, v]);
  };

  const editLast = () => {
    if (values.length === 0) return;
    const last = values[values.length - 1];
    onChange(values.slice(0, -1));
    setText(last);
  };

  const onKeyDown = (e: ReactKeyboardEvent<HTMLInputElement>) => {
    if (e.key === " " || e.key === "Enter" || e.key === ",") {
      e.preventDefault();
      commit();
    } else if (e.key === "Backspace" && text === "") {
      e.preventDefault();
      editLast();
    }
  };

  const editChip = (i: number) => {
    // Stash any half-typed value as a chip first, then pull chip i into the field.
    const pending = clean(text);
    const withPending =
      pending && !values.includes(pending) ? [...values, pending] : values;
    const target = withPending[i];
    onChange(withPending.filter((_, j) => j !== i));
    setText(target);
  };

  return (
    <div className="token-input">
      {values.map((v, i) => (
        <button
          key={i}
          className="token"
          onClick={() => editChip(i)}
          title="Click to edit"
        >
          {v}
        </button>
      ))}
      <input
        className="token-text selectable"
        type="text"
        value={text}
        placeholder={values.length === 0 ? placeholder : ""}
        spellCheck={false}
        onChange={(e) => setText(e.target.value)}
        onKeyDown={onKeyDown}
        onBlur={commit}
      />
    </div>
  );
}
