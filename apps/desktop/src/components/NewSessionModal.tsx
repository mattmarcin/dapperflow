import { useMemo, useState } from "react";
import { useStore } from "../state/store";
import { Modal } from "./Modal";
import { ProjectPicker } from "./ProjectPicker";
import { LauncherPicker } from "./LauncherPicker";

// The session-first front door (product.md / Session-first workflow). Pick a
// project (reusing ProjectPicker with inline add), pick an enabled launcher, add an
// optional first prompt, and land directly in a live terminal - no card is created.
// Keyboard-first: sensible defaults are preselected and Enter launches.
export function NewSessionModal() {
  const store = useStore();
  const { projects, agents } = store;
  const enabled = useMemo(() => agents.filter((a) => a.enabled), [agents]);

  // Default to the board's current project filter, else the only project if there is
  // exactly one, else no project.
  const defaultProject =
    store.filterProjectId ?? (projects.length === 1 ? projects[0].id : null);
  const [projectId, setProjectId] = useState<string | null>(defaultProject);
  const [agentId, setAgentId] = useState<string | null>(enabled[0]?.id ?? null);
  const [prompt, setPrompt] = useState("");
  const [addingProject, setAddingProject] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const daemonReady = store.daemon === "connected";
  const canLaunch = !!agentId && daemonReady && !busy;

  const launch = async () => {
    if (!agentId || !daemonReady || busy) return;
    setBusy(true);
    setError(null);
    try {
      await store.startSession({
        agent: agentId,
        projectId,
        firstPrompt: prompt.trim() ? prompt.trim() : null,
      });
      // startSession opens the session view and closes this modal.
    } catch (e) {
      setError(messageOf(e));
      setBusy(false);
    }
  };

  const manage = () => {
    store.closeNewSession();
    store.setView("settings");
  };

  const handleClose = () => {
    if (addingProject) {
      setAddingProject(false);
      return;
    }
    store.closeNewSession();
  };

  // Enter launches from anywhere except the multiline prompt and while adding a
  // project (that form owns its own Enter). Space still toggles the pickers.
  const onKeyDown = (e: React.KeyboardEvent) => {
    if (e.key !== "Enter") return;
    if (addingProject) return;
    const tag = (e.target as HTMLElement).tagName;
    if (tag === "TEXTAREA") return;
    e.preventDefault();
    launch();
  };

  return (
    <Modal
      title="New session"
      subtitle="Pick a project and a launcher, then start talking. No card needed."
      onClose={handleClose}
      width={540}
      footer={
        <>
          <button className="btn-ghost" onClick={handleClose}>
            Cancel
          </button>
          <button className="btn-primary" onClick={launch} disabled={!canLaunch}>
            {busy ? "Starting…" : "Start session"}
          </button>
        </>
      }
    >
      <div onKeyDown={onKeyDown} className="new-session">
        {!daemonReady ? (
          <div className="new-session-offline" role="status">
            Live sessions need the daemon. Start <code>dflowd</code> to launch one.
          </div>
        ) : null}

        <div className="field">
          <span className="field-label">Project</span>
          <ProjectPicker
            value={projectId}
            onChange={setProjectId}
            allowNone
            adding={addingProject}
            onAddingChange={setAddingProject}
          />
        </div>

        <div className="field">
          <span className="field-label">Launcher</span>
          <LauncherPicker value={agentId} onChange={setAgentId} onManage={manage} />
        </div>

        <label className="field">
          <span className="field-label">
            First prompt <span className="field-optional">optional</span>
          </span>
          <textarea
            className="field-input field-textarea"
            value={prompt}
            onChange={(e) => setPrompt(e.target.value)}
            rows={3}
            placeholder="What should the agent start on? You can also just type in the terminal."
          />
        </label>

        {error ? <p className="agent-form-error">{error}</p> : null}
      </div>
    </Modal>
  );
}

function messageOf(e: unknown): string {
  if (e instanceof Error) return e.message;
  if (e && typeof e === "object" && "message" in e) return String((e as { message: unknown }).message);
  return String(e);
}
