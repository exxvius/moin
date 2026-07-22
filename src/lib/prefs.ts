// Small UI preferences persisted in localStorage (client-only, not engine
// settings). Each returns a [value, setValue] pair like useState.

import { useEffect, useState } from "react";

const REORDER_KEY = "moin-reorder-anim";

/** Whether the downloads list animates rows sliding when the sort reorders. */
export function useReorderAnim(): [boolean, (v: boolean) => void] {
  const [on, setOn] = useState<boolean>(
    () => localStorage.getItem(REORDER_KEY) !== "0", // default on
  );
  useEffect(() => {
    localStorage.setItem(REORDER_KEY, on ? "1" : "0");
  }, [on]);
  return [on, setOn];
}
