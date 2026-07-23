import { useState, type KeyboardEvent as ReactKeyboardEvent } from "react";

interface TokenInputProps {
  values: string[];
  placeholder?: string;
  /** Clean a raw entry before it becomes a chip; a falsy result is dropped. */
  normalize?: (s: string) => string;
  onChange: (values: string[]) => void;
}

/** A chip/token field: type a value and press space (or Enter/comma) to turn it
 *  into a chip; backspace on an empty field pops the last chip back to text for
 *  editing; clicking a chip pops it back too. Shared by the category triggers and
 *  the automation exclude rules. */
export function TokenInput({
  values,
  placeholder,
  normalize,
  onChange,
}: TokenInputProps) {
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
