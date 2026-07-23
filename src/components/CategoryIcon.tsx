import { categoryIcon } from "../lib/categoryIcons";
import { categorySwatch } from "../lib/categories";

interface Props {
  icon: string | null;
  color: string;
  /** Separate icon color; empty/undefined inherits `color`. */
  iconColor?: string;
  size?: number;
}

/** A category's chosen icon tinted with its icon color (falling back to the
 *  category color), or the plain color dot when no icon is set (or unknown). */
export function CategoryIcon({ icon, color, iconColor, size = 16 }: Props) {
  const glyph = categorySwatch(iconColor ? iconColor : color);
  const dot = categorySwatch(color);
  const Icon = categoryIcon(icon);
  if (Icon) {
    return (
      <Icon size={size} style={{ color: glyph, flex: "none" }} aria-hidden />
    );
  }
  return <span className="accent-dot" style={{ background: dot }} aria-hidden />;
}
