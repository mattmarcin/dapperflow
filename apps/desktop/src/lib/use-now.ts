import { useEffect, useState } from "react";

// A shared ticking clock so every live "elapsed-in-state" readout advances
// together. One interval per mounting component; cheap at board scale.
export function useNow(intervalMs = 1000): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const t = window.setInterval(() => setNow(Date.now()), intervalMs);
    return () => window.clearInterval(t);
  }, [intervalMs]);
  return now;
}
