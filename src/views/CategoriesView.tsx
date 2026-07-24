import {
  useEffect,
  useMemo,
  useState,
  type CSSProperties,
  type PointerEvent as ReactPointerEvent,
} from "react";
import { createPortal } from "react-dom";
import { open } from "@tauri-apps/plugin-dialog";
import { Select } from "../components/Select";
import { Switch } from "../components/Switch";
import { ConfirmModal } from "../components/ConfirmModal";
import { TokenInput } from "../components/TokenInput";
import { AutomationEditor } from "../components/AutomationEditor";
import { GridPicker, type GridItem } from "../components/GridPicker";
import { DragHandleIcon } from "../components/icons";
import { useSortableList } from "../lib/useSortableList";
import { api } from "../lib/api";
import { useStore } from "../lib/store";
import {
  ADD_METHOD_LABEL,
  CATEGORY_COLORS,
  categorySwatch,
  ruleSummary,
} from "../lib/categories";
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
const LIVE_SOURCES: AddMethodKind[] = ["manual-link", "manual-torrent", "browser-capture"];

// The color swatches, shared by the color / icon-color / effects-color grids.
const COLOR_SWATCHES: GridItem[] = CATEGORY_COLORS.map((a) => ({
  value: a.id,
  title: a.label,
  render: <span className="grid-swatch" style={{ background: a.swatch }} />,
}));

// "Match accent color" — follows the app's theme accent. Offered by every color
// picker (category color, icon color, effects color).
const ACCENT_ITEM: GridItem = {
  value: "accent",
  title: "Match accent color",
  render: <span className="grid-swatch" style={{ background: "var(--accent)" }} />,
};

// The icon grid: a leading "no icon" cell, then every curated icon.
const ICON_ITEMS: GridItem[] = [
  // Blank cell = no icon (an empty square, not a glyph of its own).
  { value: "", title: "No icon", render: null },
  ...CATEGORY_ICONS.map(([id, Icon]) => ({
    value: id,
    title: id.replace(/-/g, " "),
    render: <Icon size={18} />,
  })),
];

function blankCategory(): Category {
  return {
    id: "",
    name: "",
    color: "blue",
    icon_color: "",
    effects_color: "",
    icon: null,
    hidden_from_all: false,
    save_dir: null,
    seed_ratio_limit: null,
    seed_time_limit_mins: null,
    incomplete_dir: null,
    torrent_file_dir: null,
    torrent_file_done_dir: null,
    sources: [],
    watch_folders: [],
    capture_torrent_downloads: false,
    triggers: [],
    fallback_download: false,
    order: 0,
    automation: {
      exclude: [],
      layout: "original",
      renames: [],
    },
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
  // How many downloads currently sit in each category (and uncategorized). Archived
  // records aren't in the list, so they don't count.
  const counts = useMemo(() => {
    const byCat = new Map<string, number>();
    let uncat = 0;
    for (const t of store.all) {
      if (t.archived) continue;
      if (t.category) byCat.set(t.category, (byCat.get(t.category) ?? 0) + 1);
      else uncat++;
    }
    return { byCat, uncat };
  }, [store.all]);
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
          className="btn primary"
          onClick={() => setEditing(blankCategory())}
        >
          New category
        </button>
      </div>

      <div className="cat-panel">
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
              {cats.map((c, i) => {
                const count = counts.byCat.get(c.id) ?? 0;
                // Effects (hover glow/border) use the category's chosen effects
                // color, falling back to its main color, then the theme accent.
                const glowSrc = c.effects_color || c.color;
                const style = {
                  "--cat": categorySwatch(c.color),
                  ...(glowSrc ? { "--cat-glow": categorySwatch(glowSrc) } : {}),
                } as CSSProperties;
                return (
                <div
                  className="cat-card"
                  key={c.id}
                  data-id={c.id}
                  style={style}
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
                    <CategoryIcon
                      icon={c.icon}
                      color={c.color}
                      iconColor={c.icon_color}
                      size={22}
                    />
                  </span>
                  <div className="cat-row-main">
                    <div className="cat-name-row">
                      <span className="setting-label">{c.name || "Untitled"}</span>
                      <span
                        className="cat-count"
                        title={`${count} download${count === 1 ? "" : "s"}`}
                      >
                        {count}
                      </span>
                    </div>
                    <div className="dim cat-sub">{ruleSummary(c)}</div>
                    {c.save_dir && (
                      <div className="dim cat-sub path">Saves to {c.save_dir}</div>
                    )}
                  </div>
                  <div className="cat-row-actions">
                    <button className="btn" onClick={() => setEditing(c)}>
                      Edit
                    </button>
                    <button
                      className="btn danger"
                      onClick={() => setConfirmDelete(c)}
                    >
                      Delete
                    </button>
                  </div>
                </div>
                );
              })}
            </div>
          )}
        </div>

        {/* Pinned: where uncategorized downloads land. Only its save folder is
            editable — no name, color, icon, or rules. */}
        <div className="cat-card cat-card-uncat">
          <span className="cat-card-icon">
            <CategoryIcon icon="inbox" color="" size={22} />
          </span>
          <div className="cat-row-main">
            <div className="cat-name-row">
              <span className="setting-label">Uncategorized</span>
              <span
                className="cat-count"
                title={`${counts.uncat} download${counts.uncat === 1 ? "" : "s"}`}
              >
                {counts.uncat}
              </span>
            </div>
            <div className="dim cat-sub path">
              {settings?.download_dir
                ? `Saves to ${settings.download_dir}`
                : "Saves to your default download folder"}
            </div>
          </div>
          <div className="cat-row-actions">
            <button
              className="btn"
              disabled={!settings}
              onClick={pickUncatFolder}
            >
              {settings?.download_dir ? "Change…" : "Choose…"}
            </button>
            {settings?.download_dir && (
              <button
                className="btn danger"
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

/** A save-folder-style row: a path readout with Choose/Change and Clear buttons. */
function FolderRow({
  label,
  desc,
  value,
  onPick,
  onClear,
}: {
  label: string;
  desc: string;
  value: string | null;
  onPick: () => void;
  onClear: () => void;
}) {
  return (
    <div className="setting-row">
      <div>
        <div className="setting-label">{label}</div>
        <div className="dim">{desc}</div>
        {value && <div className="dim path cat-savedir">{value}</div>}
      </div>
      <div className="tool-actions">
        <button className="btn" onClick={onPick}>
          {value ? "Change…" : "Choose…"}
        </button>
        {value && (
          <button className="btn danger" onClick={onClear}>
            Clear
          </button>
        )}
      </div>
    </div>
  );
}

/** A seed-limit field with three modes — inherit the global default, seed
 *  forever, or a custom value (`null` / `0` / `>0` respectively). */
function SeedLimitField({
  label,
  desc,
  unit,
  step,
  value,
  onChange,
}: {
  label: string;
  desc: string;
  unit: string;
  step: number;
  value: number | null;
  onChange: (v: number | null) => void;
}) {
  const mode = value === null ? "inherit" : value === 0 ? "unlimited" : "custom";
  return (
    <div className="setting-row">
      <div>
        <div className="setting-label">{label}</div>
        <div className="dim">{desc}</div>
      </div>
      <div className="seed-limit-control">
        <div className="method-chips">
          <button
            className={`method-chip${mode === "inherit" ? " on" : ""}`}
            aria-pressed={mode === "inherit"}
            onClick={() => onChange(null)}
          >
            Global
          </button>
          <button
            className={`method-chip${mode === "unlimited" ? " on" : ""}`}
            aria-pressed={mode === "unlimited"}
            onClick={() => onChange(0)}
          >
            Forever
          </button>
          <button
            className={`method-chip${mode === "custom" ? " on" : ""}`}
            aria-pressed={mode === "custom"}
            onClick={() => onChange(value && value > 0 ? value : step)}
          >
            Custom
          </button>
        </div>
        {mode === "custom" && (
          <div className="seed-limit-input">
            <input
              className="add-input"
              type="number"
              min={step}
              step={step}
              value={value ?? ""}
              onChange={(e) => {
                const n = parseFloat(e.target.value);
                onChange(Number.isFinite(n) && n > 0 ? n : step);
              }}
            />
            <span className="dim">{unit}</span>
          </div>
        )}
      </div>
    </div>
  );
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

  // Whether the watched-folder source is on (reveals the folder pickers). Seeded
  // from whether the category already watches anything.
  const [watchOn, setWatchOn] = useState(initial.watch_folders.length > 0);

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

  // Pick a folder into any of the category's path fields (incomplete staging,
  // .torrent export). Shares the OS picker with the save-folder button.
  const pickInto = (field: keyof Category, title: string) => async () => {
    const picked = await open({ directory: true, multiple: false, title });
    if (typeof picked === "string")
      patch({ [field]: picked } as Partial<Category>);
  };

  // Watched-folder source: toggling it off clears the folders so the source is
  // genuinely off; a folder is added via the OS picker.
  const toggleWatch = () => {
    if (watchOn) {
      setWatchOn(false);
      patch({ watch_folders: [] });
    } else {
      setWatchOn(true);
    }
  };
  const addWatchFolder = async () => {
    const picked = await open({
      directory: true,
      multiple: false,
      title: "Choose a folder to watch for .torrent files",
    });
    if (typeof picked === "string" && !draft.watch_folders.includes(picked)) {
      patch({ watch_folders: [...draft.watch_folders, picked] });
    }
  };
  const removeWatchFolder = (i: number) =>
    patch({ watch_folders: draft.watch_folders.filter((_, j) => j !== i) });

  // The swatch shown in a color picker's trigger: the chosen color, else a
  // fallback (a category color the picker inherits from), else the no-color ring.
  const swatchNode = (colorId: string, fallback?: string) => {
    const id = colorId || fallback || "";
    return id ? (
      <span className="grid-swatch" style={{ background: categorySwatch(id) }} />
    ) : (
      <span className="grid-swatch no-color" />
    );
  };
  const inheritItem: GridItem = {
    value: "",
    title: "Match category color",
    render: (
      <span
        className="grid-swatch inherit"
        style={{ background: categorySwatch(draft.color) }}
      />
    ),
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
            <GridPicker
              value={draft.color}
              ariaLabel="Category color"
              onChange={(v) => patch({ color: v })}
              trigger={swatchNode(draft.color)}
              items={[
                {
                  value: "",
                  title: "No color",
                  render: <span className="grid-swatch no-color" />,
                },
                ACCENT_ITEM,
                ...COLOR_SWATCHES,
              ]}
            />
          </div>

          <div className="setting-row">
            <div className="setting-label">Icon color</div>
            <GridPicker
              value={draft.icon_color}
              ariaLabel="Icon color"
              onChange={(v) => patch({ icon_color: v })}
              trigger={swatchNode(draft.icon_color, draft.color)}
              items={[inheritItem, ACCENT_ITEM, ...COLOR_SWATCHES]}
            />
          </div>

          <div className="setting-row">
            <div className="setting-label">Effects color</div>
            <GridPicker
              value={draft.effects_color}
              ariaLabel="Effects color"
              onChange={(v) => patch({ effects_color: v })}
              trigger={swatchNode(draft.effects_color, draft.color)}
              items={[inheritItem, ACCENT_ITEM, ...COLOR_SWATCHES]}
            />
          </div>

          <div className="setting-row">
            <div>
              <div className="setting-label">Icon</div>
              <div className="dim">
                Optional — shows on the category in place of the color dot.
              </div>
            </div>
            <GridPicker
              value={draft.icon ?? ""}
              ariaLabel="Category icon"
              onChange={(v) => patch({ icon: v || null })}
              menuColor={categorySwatch(draft.icon_color || draft.color)}
              trigger={
                draft.icon ? (
                  // Match the 20px color swatch so the trigger box is the same size.
                  <CategoryIcon
                    icon={draft.icon}
                    color={draft.color}
                    iconColor={draft.icon_color}
                    size={20}
                  />
                ) : (
                  // No icon selected: an empty square (matching the grid's "no icon"
                  // cell), not the color dot.
                  <span className="grid-swatch no-color" />
                )
              }
              items={ICON_ITEMS}
            />
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
              <button className="btn" onClick={pickFolder}>
                {draft.save_dir ? "Change…" : "Choose…"}
              </button>
              {draft.save_dir && (
                <button
                  className="btn danger"
                  onClick={() => patch({ save_dir: null })}
                >
                  Clear
                </button>
              )}
            </div>
          </div>

          <FolderRow
            label="Incomplete folder"
            desc="Stage in-progress downloads here, then move the finished content into the save folder on completion. Leave unset to download in place."
            value={draft.incomplete_dir}
            onPick={pickInto("incomplete_dir", "Choose an incomplete folder")}
            onClear={() => patch({ incomplete_dir: null })}
          />

          <div className="setting-row">
            <div>
              <div className="setting-label">Hide from All filter</div>
              <div className="dim">
                Keep this category's downloads out of the “All categories” filter —
                they show only when you pick this category. Like archived tasks
                staying out of the “All” status filter.
              </div>
            </div>
            <Switch
              checked={draft.hidden_from_all}
              ariaLabel="Hide from the All categories filter"
              onChange={(v) => patch({ hidden_from_all: v })}
            />
          </div>

          <div className="cat-section">
            <div className="setting-label">Sources</div>
            <div className="dim">
              How this category takes in downloads. The first three filter which
              manual and captured adds it claims (leave all off to match any); the
              last two feed it automatically.
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
              <button
                className={`method-chip${watchOn ? " on" : ""}`}
                aria-pressed={watchOn}
                onClick={toggleWatch}
              >
                {ADD_METHOD_LABEL["watch-folder"]}
              </button>
              <button
                className={`method-chip${draft.capture_torrent_downloads ? " on" : ""}`}
                aria-pressed={draft.capture_torrent_downloads}
                onClick={() =>
                  patch({
                    capture_torrent_downloads: !draft.capture_torrent_downloads,
                  })
                }
              >
                Downloaded torrent
              </button>
            </div>

            {watchOn && (
              <div className="watch-config">
                <div className="dim">
                  Drop a .torrent into one of these folders and it's added under
                  this category automatically. Handled files are renamed so they
                  aren't added twice.
                </div>
                <div className="watch-folders">
                  {draft.watch_folders.map((folder, i) => (
                    <div className="watch-folder-row" key={folder}>
                      <span
                        className="path selectable watch-folder-path"
                        title={folder}
                      >
                        {folder}
                      </span>
                      <button
                        className="btn danger"
                        aria-label="Remove folder"
                        onClick={() => removeWatchFolder(i)}
                      >
                        Remove
                      </button>
                    </div>
                  ))}
                </div>
                <button className="btn add-trigger" onClick={addWatchFolder}>
                  Add folder
                </button>
              </div>
            )}

            {draft.capture_torrent_downloads && (
              <div className="dim watch-config-note">
                A .torrent you download through moin that files into this category
                is re-added as a torrent (into this category's save folder), rather
                than left as a file. Uncategorized downloads are untouched.
              </div>
            )}
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
            <button className="btn add-trigger" onClick={addTrigger}>
              Add trigger
            </button>
          </div>

          <AutomationEditor
            automation={draft.automation}
            onChange={(automation) => patch({ automation })}
          />

          <div className="cat-section">
            <div className="setting-label">Seeding &amp; torrent files</div>
            <div className="dim">
              Torrent-only. Seeding limits stop uploading once a torrent filed here
              hits a ratio or time; “Global” follows the app-wide setting.
            </div>
            <SeedLimitField
              label="Seed ratio limit"
              desc="Stop seeding once uploaded ÷ downloaded reaches this."
              unit="ratio"
              step={0.1}
              value={draft.seed_ratio_limit}
              onChange={(v) => patch({ seed_ratio_limit: v })}
            />
            <SeedLimitField
              label="Seed time limit"
              desc="Stop seeding this long after the torrent finishes."
              unit="minutes"
              step={1}
              value={draft.seed_time_limit_mins}
              onChange={(v) => patch({ seed_time_limit_mins: v })}
            />
            <FolderRow
              label="Save .torrent files to"
              desc="Keep a copy of each added torrent's .torrent file in this folder."
              value={draft.torrent_file_dir}
              onPick={pickInto("torrent_file_dir", "Choose a .torrent folder")}
              onClear={() => patch({ torrent_file_dir: null })}
            />
            <FolderRow
              label="Completed .torrent files to"
              desc="When a torrent finishes, move its .torrent copy here — a separate home for completed torrents."
              value={draft.torrent_file_done_dir}
              onPick={pickInto(
                "torrent_file_done_dir",
                "Choose a folder for completed .torrent files",
              )}
              onClear={() => patch({ torrent_file_done_dir: null })}
            />
          </div>

          <div className="setting-row">
            <div>
              <div className="setting-label">Download even if triggers fail</div>
              <div className="dim">
                For watched folders: grab a dropped torrent that doesn't match
                the triggers above anyway, uncategorized, instead of skipping it.
                No effect on manual adds.
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
          <button className="btn ghost" onClick={onCancel}>
            Cancel
          </button>
          <button
            className="btn primary"
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
      <button className="btn danger" aria-label="Remove trigger" onClick={onRemove}>
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
