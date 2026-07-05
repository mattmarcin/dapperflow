import { useState } from "react";
import { useStore } from "../state/store";
import { AddProjectForm } from "./AddProjectForm";

interface Props {
  /** Selected project id, or null for no project. */
  value: string | null;
  onChange: (projectId: string | null) => void;
  /** Offer a "No project" choice (cross-project work). Default true. */
  allowNone?: boolean;
  /**
   * Optional controlled inline-add state. A host that must coordinate with the
   * add flow (the card modal cancels it on Escape/scrim before closing itself)
   * passes both; standalone consumers omit them and the picker self-manages.
   */
  adding?: boolean;
  onAddingChange?: (adding: boolean) => void;
}

// Pick a project, with inline add-project (native picker + path input). Reused
// wherever work gets attached to a project: the card modal today, the New
// Session front door in a later phase. Lists registered projects as pills,
// offers "Add a project..." backed by AddProjectForm, and auto-selects a newly
// registered project. Empty registry gets a designed explanation, not a bare
// "No project" option.
export function ProjectPicker({ value, onChange, allowNone = true, adding, onAddingChange }: Props) {
  const { projects } = useStore();
  const [addingInternal, setAddingInternal] = useState(false);
  const isAdding = adding ?? addingInternal;
  const setAdding = (next: boolean) => {
    if (adding === undefined) setAddingInternal(next);
    onAddingChange?.(next);
  };

  if (isAdding) {
    return (
      <AddProjectForm
        onAdded={(p) => {
          onChange(p.id);
          setAdding(false);
        }}
        onCancel={() => setAdding(false)}
      />
    );
  }

  // Designed empty state: projects are local git repos, so explain and invite.
  if (projects.length === 0) {
    return (
      <div className="project-picker">
        <p className="project-picker-empty">
          No projects yet. A project is a local git repository DapperFlow works in.
          {allowNone ? " Register one, or continue without a project." : " Register one to continue."}
        </p>
        <button type="button" className="project-add" onClick={() => setAdding(true)}>
          <IconPlus />
          Add a project…
        </button>
      </div>
    );
  }

  return (
    <div className="project-picker">
      <div className="project-options">
        {allowNone ? (
          <button
            type="button"
            className={`project-option${value === null ? " is-active" : ""}`}
            onClick={() => onChange(null)}
          >
            No project
          </button>
        ) : null}
        {projects.map((p) => (
          <button
            key={p.id}
            type="button"
            className={`project-option${value === p.id ? " is-active" : ""}`}
            onClick={() => onChange(p.id)}
            title={p.path}
          >
            {p.name}
          </button>
        ))}
      </div>
      <button type="button" className="project-add" onClick={() => setAdding(true)}>
        <IconPlus />
        Add a project…
      </button>
    </div>
  );
}

function IconPlus() {
  return (
    <svg width="12" height="12" viewBox="0 0 13 13" aria-hidden>
      <path d="M6.5 1.5v10M1.5 6.5h10" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" />
    </svg>
  );
}
