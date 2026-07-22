import { ACCENTS, type Accent } from "../lib/accent";
import type { Theme } from "../lib/theme";

interface Props {
  theme: Theme;
  toggleTheme: () => void;
  accent: Accent;
  setAccent: (a: Accent) => void;
}

export function SettingsView({ theme, toggleTheme, accent, setAccent }: Props) {
  return (
    <div className="view">
      <div className="view-head">
        <h2>Settings</h2>
        <p>Make moin yours.</p>
      </div>

      <div className="card">
        <div className="card-title">Appearance</div>

        <div className="setting-row">
          <div>
            <div className="setting-label">Theme</div>
            <div className="dim">Dark is the primary look; light stays clean.</div>
          </div>
          <button className="foot-btn" onClick={toggleTheme}>
            <span>{theme === "dark" ? "Dark" : "Light"}</span>
          </button>
        </div>

        <div className="setting-row">
          <div>
            <div className="setting-label">Accent</div>
            <div className="dim">
              Recolors the whole app. More can be added later.
            </div>
          </div>
        </div>

        <div className="accent-grid">
          {ACCENTS.map((a) => (
            <button
              key={a.id}
              className={`accent-swatch${a.id === accent ? " on" : ""}`}
              style={{ ["--sw" as string]: a.swatch }}
              onClick={() => setAccent(a.id)}
              title={a.label}
              aria-pressed={a.id === accent}
            >
              <span className="dot" />
              <span>{a.label}</span>
            </button>
          ))}
        </div>
      </div>
    </div>
  );
}
