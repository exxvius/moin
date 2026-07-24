import { useEffect } from "react";
import { createPortal } from "react-dom";

interface Props {
  onMinimize: () => void;
  onQuit: () => void;
  onCancel: () => void;
}

/** Shown when the user tries to close moin while transfers are still running and
 *  the "minimize to tray on close" setting is off. Offers to keep everything
 *  running in the tray instead of stopping it. Esc / backdrop cancels. */
export function QuitConfirmModal({ onMinimize, onQuit, onCancel }: Props) {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onCancel();
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [onCancel]);

  return createPortal(
    <div className="modal-backdrop" onClick={onCancel}>
      <div
        className="modal quit-confirm-modal"
        role="dialog"
        aria-modal="true"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="modal-title">Downloads are still running</div>
        <div className="modal-body quit-confirm-body">
          Quitting will stop your active downloads and seeding.
        </div>
        <div className="modal-actions">
          <button className="qc-btn ghost" onClick={onCancel}>
            Cancel
          </button>
          <button className="qc-btn" onClick={onMinimize} autoFocus>
            Minimize to tray
          </button>
          <button className="qc-btn danger" onClick={onQuit}>
            Quit anyway
          </button>
        </div>
      </div>
    </div>,
    document.body,
  );
}
