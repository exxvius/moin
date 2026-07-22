import { categoryIcon } from "../lib/categoryIcons";
import { categorySwatch } from "../lib/categories";

interface Props {
  icon: string | null;
  color: string;
  size?: number;
}

/** A category's chosen icon tinted with its color, or the plain color dot when
 *  no icon is set (or the id is unknown). */
export function CategoryIcon({ icon, color, size = 16 }: Props) {
  const swatch = categorySwatch(color);
  const Icon = categoryIcon(icon);
  if (Icon) {
    return (
      <Icon size={size} style={{ color: swatch, flex: "none" }} aria-hidden />
    );
  }
  return (
    <span
      className="accent-dot"
      style={{ background: swatch }}
      aria-hidden
    />
  );
}
