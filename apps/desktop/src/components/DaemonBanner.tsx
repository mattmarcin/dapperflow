import { useEffect, useRef, useState } from "react";
import { useStore } from "../state/store";

// A slim banner when the daemon is unreachable. Board data keeps working on
// fixtures; the banner is honest about what is degraded (live terminals).
//
// It is LATCHED with hysteresis so it never flickers: the reconnect loop rapidly
// oscillates the connection between "disconnected" (waiting to retry) and
// "connecting" (mid-retry), and the old banner, which rendered only for
// "disconnected", blinked off on every "connecting". Now any non-connected state
// counts as degraded, the banner appears only after a short grace (so a momentary
// blip does not flash), holds steady through the whole retry cycle, and clears
// cleanly only once the daemon is truly connected again.
export function DaemonBanner() {
  const { daemon, fixtureMode } = useStore();
  const [visible, setVisible] = useState(false);
  const graceTimer = useRef<number>();

  useEffect(() => {
    if (daemon === "connected") {
      // Truly reconnected: drop the banner immediately and cancel any pending show.
      if (graceTimer.current !== undefined) {
        window.clearTimeout(graceTimer.current);
        graceTimer.current = undefined;
      }
      setVisible(false);
      return;
    }
    // Degraded: connecting | disconnected | absent. Latch on after a short grace and
    // stay steady. Do not re-arm the timer on every oscillation (that is the flicker),
    // and once shown, keep it shown until "connected" flips it off above.
    if (!visible && graceTimer.current === undefined) {
      graceTimer.current = window.setTimeout(() => {
        graceTimer.current = undefined;
        setVisible(true);
      }, 600);
    }
  }, [daemon, visible]);

  useEffect(
    () => () => {
      if (graceTimer.current !== undefined) window.clearTimeout(graceTimer.current);
    },
    [],
  );

  if (!visible) return null;

  // "absent" is the daemon we could not reach at all (start it); every other degraded
  // state is a live daemon we lost and are reconnecting to.
  const offline = daemon === "absent";
  return (
    <div className={`daemon-banner${offline ? "" : " is-reconnecting"}`} role="status" aria-live="polite">
      <span className="daemon-banner-dot" aria-hidden />
      <span className="daemon-banner-text">
        {offline ? (
          <>
            Daemon offline. Live terminals are unavailable
            {fixtureMode ? "; the board is running on dev fixtures." : "."} Start{" "}
            <code>dflowd</code> to open real sessions.
          </>
        ) : (
          <>Daemon unavailable. Reconnecting…</>
        )}
      </span>
    </div>
  );
}
