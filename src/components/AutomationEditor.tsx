import { Select } from "./Select";
import { Switch } from "./Switch";
import { TokenInput } from "./TokenInput";
import type {
  Automation,
  FileMatch,
  FileMatchType,
  LayoutMode,
  PresetKind,
  RenameRule,
  RenameRuleType,
} from "../lib/types";

const MIB = 1024 * 1024;

const LAYOUT_OPTIONS: { value: LayoutMode; label: string }[] = [
  { value: "original", label: "Original" },
  { value: "subfolder", label: "Create subfolder" },
  { value: "flat", label: "Don't create subfolder" },
];

const EXCLUDE_TYPES: { value: FileMatchType; label: string }[] = [
  { value: "extension", label: "File type" },
  { value: "name-pattern", label: "Filename pattern" },
  { value: "size-under", label: "Smaller than" },
  { value: "size-over", label: "Larger than" },
];

const RENAME_TYPES: { value: RenameRuleType; label: string }[] = [
  { value: "preset", label: "Preset" },
  { value: "replace", label: "Find & replace" },
];

const PRESET_OPTIONS: { value: PresetKind; label: string }[] = [
  { value: "dots-to-spaces", label: "Dots → spaces" },
  { value: "underscores-to-spaces", label: "Underscores → spaces" },
  { value: "lowercase", label: "Lowercase" },
  { value: "strip-tags", label: "Strip [tags]" },
];

function defaultExclusion(type: FileMatchType): FileMatch {
  switch (type) {
    case "extension":
      return { type, exts: [] };
    case "name-pattern":
      return { type, patterns: [] };
    case "size-under":
      return { type, bytes: 0 };
    case "size-over":
      return { type, bytes: 0 };
  }
}

function defaultRename(type: RenameRuleType): RenameRule {
  return type === "preset"
    ? { type, kind: "dots-to-spaces" }
    : { type, find: "", with: "", regex: false };
}

interface Props {
  automation: Automation;
  onChange: (a: Automation) => void;
}

/** The torrent-automation editor: watched folders, file exclusions, content
 *  layout, and rename rules. Lives inside the category editor modal. */
export function AutomationEditor({ automation, onChange }: Props) {
  const patch = (change: Partial<Automation>) =>
    onChange({ ...automation, ...change });

  // --- exclusions ---
  const setExclusion = (i: number, rule: FileMatch) =>
    patch({ exclude: automation.exclude.map((r, j) => (j === i ? rule : r)) });
  const removeExclusion = (i: number) =>
    patch({ exclude: automation.exclude.filter((_, j) => j !== i) });
  const addExclusion = () =>
    patch({ exclude: [...automation.exclude, defaultExclusion("extension")] });

  // --- renames ---
  const setRename = (i: number, rule: RenameRule) =>
    patch({ renames: automation.renames.map((r, j) => (j === i ? rule : r)) });
  const removeRename = (i: number) =>
    patch({ renames: automation.renames.filter((_, j) => j !== i) });
  const addRename = () =>
    patch({ renames: [...automation.renames, defaultRename("preset")] });

  return (
    <>
      <div className="cat-section-head">
        <div className="setting-label">Automation</div>
        <div className="dim">
          How torrents filed into this category are fetched and organized.
        </div>
      </div>

      <div className="setting-row">
        <div className="setting-label">Content layout</div>
        <Select
          value={automation.layout}
          ariaLabel="Content layout"
          caret
          onChange={(v) => patch({ layout: v as LayoutMode })}
          options={LAYOUT_OPTIONS}
        />
      </div>

      <div className="cat-section">
        <div className="setting-label">Exclude files</div>
        <div className="dim">
          A file matching any rule is skipped — good for `.nfo`, samples, and
          tiny junk. A rule set that would drop every file is ignored.
        </div>
        <div className="trigger-list">
          {automation.exclude.map((rule, i) => (
            <div className="trigger-row" key={i}>
              <Select
                value={rule.type}
                ariaLabel="Exclusion type"
                caret
                onChange={(v) => setExclusion(i, defaultExclusion(v as FileMatchType))}
                options={EXCLUDE_TYPES}
              />
              <div className="trigger-fields">
                <ExclusionFields rule={rule} onChange={(r) => setExclusion(i, r)} />
              </div>
              <button
                className="dl-btn danger"
                aria-label="Remove exclusion"
                onClick={() => removeExclusion(i)}
              >
                Remove
              </button>
            </div>
          ))}
        </div>
        <button className="dl-btn add-trigger" onClick={addExclusion}>
          Add exclusion
        </button>
      </div>

      <div className="cat-section">
        <div className="setting-label">Rename files</div>
        <div className="dim">
          Applied in order to each file's name (its folder is kept). Regex
          replacements can reference groups with <code>{"${1}"}</code>.
        </div>
        <div className="trigger-list">
          {automation.renames.map((rule, i) => (
            <div className="trigger-row" key={i}>
              <Select
                value={rule.type}
                ariaLabel="Rename type"
                caret
                onChange={(v) => setRename(i, defaultRename(v as RenameRuleType))}
                options={RENAME_TYPES}
              />
              <div className="trigger-fields">
                <RenameFields rule={rule} onChange={(r) => setRename(i, r)} />
              </div>
              <button
                className="dl-btn danger"
                aria-label="Remove rename rule"
                onClick={() => removeRename(i)}
              >
                Remove
              </button>
            </div>
          ))}
        </div>
        <button className="dl-btn add-trigger" onClick={addRename}>
          Add rename rule
        </button>
      </div>
    </>
  );
}

interface ExclusionFieldsProps {
  rule: FileMatch;
  onChange: (r: FileMatch) => void;
}

function ExclusionFields({ rule, onChange }: ExclusionFieldsProps) {
  switch (rule.type) {
    case "extension":
      return (
        <TokenInput
          values={rule.exts}
          placeholder="nfo, txt, exe…"
          normalize={(s) => s.trim().replace(/^\./, "").toLowerCase()}
          onChange={(exts) => onChange({ type: "extension", exts })}
        />
      );
    case "name-pattern":
      return (
        <TokenInput
          values={rule.patterns}
          placeholder="*sample*, *.r0?…"
          onChange={(patterns) => onChange({ type: "name-pattern", patterns })}
        />
      );
    case "size-under":
    case "size-over":
      return (
        <div className="size-fields">
          <input
            className="add-input selectable size-input"
            type="number"
            min={0}
            placeholder="MB"
            value={rule.bytes ? rule.bytes / MIB : ""}
            onChange={(e) =>
              onChange({
                ...rule,
                bytes: e.target.value === "" ? 0 : Number(e.target.value) * MIB,
              })
            }
          />
          <span className="dim">MB</span>
        </div>
      );
  }
}

interface RenameFieldsProps {
  rule: RenameRule;
  onChange: (r: RenameRule) => void;
}

function RenameFields({ rule, onChange }: RenameFieldsProps) {
  if (rule.type === "preset") {
    return (
      <Select
        value={rule.kind}
        ariaLabel="Preset transform"
        caret
        onChange={(v) => onChange({ type: "preset", kind: v as PresetKind })}
        options={PRESET_OPTIONS}
      />
    );
  }
  return (
    <div className="rename-fields">
      <input
        className="add-input selectable rename-input"
        type="text"
        placeholder={rule.regex ? "pattern" : "find"}
        spellCheck={false}
        value={rule.find}
        onChange={(e) => onChange({ ...rule, find: e.target.value })}
      />
      <span className="dim">→</span>
      <input
        className="add-input selectable rename-input"
        type="text"
        placeholder="replace with"
        spellCheck={false}
        value={rule.with}
        onChange={(e) => onChange({ ...rule, with: e.target.value })}
      />
      <label className="rename-regex">
        <Switch
          checked={rule.regex}
          ariaLabel="Treat find as a regular expression"
          onChange={(v) => onChange({ ...rule, regex: v })}
        />
        <span className="dim">regex</span>
      </label>
    </div>
  );
}
