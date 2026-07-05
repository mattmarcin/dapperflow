// Tauri host detection and the native folder picker. In the packaged app these
// bridge to the Tauri webview APIs; in a plain browser (used for visual
// development) isTauri() is false and callers fall back gracefully - the
// add-project Browse button hides, manual path entry stays.

// True when the React app is running inside the Tauri webview (not a plain
// browser dev session). The globals are injected by the Tauri runtime.
export function isTauri(): boolean {
  return (
    typeof window !== "undefined" &&
    ("__TAURI_INTERNALS__" in window || "__TAURI__" in window)
  );
}

// Open the native directory picker, seeded at the user's home directory, and
// return the chosen absolute path - or null if the user cancelled. Only call
// when isTauri() is true; in a plain browser the plugin import is unavailable.
export async function pickDirectory(): Promise<string | null> {
  const { open } = await import("@tauri-apps/plugin-dialog");

  // Seed at the home directory so the picker opens somewhere useful. If the path
  // API is unavailable the picker still opens at the OS default.
  let defaultPath: string | undefined;
  try {
    const { homeDir } = await import("@tauri-apps/api/path");
    defaultPath = await homeDir();
  } catch {
    defaultPath = undefined;
  }

  const selected = await open({
    directory: true,
    multiple: false,
    title: "Choose a project folder",
    defaultPath,
  });

  // With multiple:false the plugin resolves to a single path or null.
  return typeof selected === "string" ? selected : null;
}

// Reveal a project folder in the OS file manager. In the packaged app this runs the
// app-defined `reveal_in_explorer` Tauri command; in a plain browser it returns false
// so the caller can fall back (e.g. copy the path).
export async function revealInExplorer(path: string): Promise<boolean> {
  if (!isTauri()) return false;
  try {
    const { invoke } = await import("@tauri-apps/api/core");
    await invoke("reveal_in_explorer", { path });
    return true;
  } catch {
    return false;
  }
}
