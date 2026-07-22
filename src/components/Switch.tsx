interface Props {
  checked: boolean;
  onChange: (checked: boolean) => void;
  ariaLabel?: string;
  disabled?: boolean;
}

/** A themed on/off toggle (styled via `.switch` in app.css). */
export function Switch({ checked, onChange, ariaLabel, disabled }: Props) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={ariaLabel}
      className="switch"
      disabled={disabled}
      onClick={() => onChange(!checked)}
    />
  );
}
