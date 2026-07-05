import { useState } from "react";
import { useStore } from "../state/store";
import { Project } from "../model";
import { isTauri, pickDirectory } from "../lib/tauri";

interface Props {
  onAdded: (project: Project) => void;
  onCancel: () => void;
  autoFocus?: boolean;
}

// The add-project flow, shared by the sidebar Projects tree and the card modal's
// inline "Add a project" action. A path input plus a native Browse button (folder
// picker, seeded at home); manual entry still works for pasted paths. The Browse
// button hides in a plain browser where the native dialog is unavailable.
export function AddProjectForm({ onAdded, onCancel, autoFocus = true }: Props) {
  const store = useStore();
  const [path, setPath] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const canBrowse = isTauri();

  const submit = async () => {
    if (!path.trim() || busy) return;
    setBusy(true);
    setError(null);
    const res = await store.addProject(path);
    setBusy(false);
    if (res.ok && res.project) {
      store.flash(`Registered ${res.project.name}.`);
      onAdded(res.project);
    } else {
      setError(res.error ?? "Could not add that project.");
    }
  };

  const browse = async () => {
    try {
      const dir = await pickDirectory();
      if (dir) {
        setPath(dir);
        setError(null);
      }
    } catch {
      setError("Could not open the folder picker. Type or paste a path instead.");
    }
  };

  return (
    <div className="add-project">
      <div className="add-project-field">
        <input
          className={`add-project-input${error ? " is-error" : ""}`}
          value={path}
          autoFocus={autoFocus}
          placeholder="C:\path\to\repo"
          onChange={(e) => {
            setPath(e.target.value);
            setError(null);
          }}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault();
              submit();
            }
            if (e.key === "Escape") {
              e.stopPropagation();
              onCancel();
            }
          }}
        />
        {canBrowse ? (
          <button
            type="button"
            className="add-project-browse"
            onClick={browse}
            aria-label="Browse for a folder"
            title="Browse for a folder"
          >
            <IconFolder />
          </button>
        ) : null}
      </div>
      <div className="add-project-row">
        <span className={`add-project-msg${error ? " is-error" : ""}`}>
          {error ?? "Point at a local git repository."}
        </span>
        <div className="add-project-actions">
          <button type="button" className="btn-ghost btn-sm" onClick={onCancel}>
            Cancel
          </button>
          <button
            type="button"
            className="btn-primary btn-sm"
            onClick={submit}
            disabled={!path.trim() || busy}
          >
            {busy ? "Checking…" : "Add"}
          </button>
        </div>
      </div>
    </div>
  );
}

function IconFolder() {
  return (
    <svg width="14" height="14" viewBox="0 0 16 16" fill="none" aria-hidden>
      <path
        d="M1.8 4.4c0-.6.5-1.1 1.1-1.1h2.9l1.3 1.5h5.9c.6 0 1.1.5 1.1 1.1v5.9c0 .6-.5 1.1-1.1 1.1H2.9c-.6 0-1.1-.5-1.1-1.1V4.4z"
        stroke="currentColor"
        strokeWidth="1.3"
        strokeLinejoin="round"
      />
    </svg>
  );
}
