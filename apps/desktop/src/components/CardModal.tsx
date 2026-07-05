import { useState } from "react";
import { CARD_TYPES, CardType } from "../model";
import { CARD_TYPE_META } from "../lib/glyphs";
import { useStore } from "../state/store";
import { Modal } from "./Modal";
import { ProjectPicker } from "./ProjectPicker";
import { RecipeDial } from "./RecipeDial";

interface Props {
  defaultProjectId: string | null;
  onClose: () => void;
  onCreate: (input: {
    title: string;
    type: CardType;
    project_id: string | null;
    dial_recipe: string | null;
    brief: string | null;
  }) => void;
}

// The full card-creation modal: title, type, project, process dial, optional brief.
export function CardModal({ defaultProjectId, onClose, onCreate }: Props) {
  const store = useStore();
  const [title, setTitle] = useState("");
  const [type, setType] = useState<CardType>("feature");
  const [projectId, setProjectId] = useState<string | null>(defaultProjectId);
  const [dialRecipe, setDialRecipe] = useState<string | null>(null);
  const [brief, setBrief] = useState("");
  const [addingProject, setAddingProject] = useState(false);

  const project = projectId ? store.projects.find((p) => p.id === projectId) : undefined;

  const canCreate = title.trim().length > 0;
  const submit = () => {
    if (!canCreate) return;
    onCreate({
      title: title.trim(),
      type,
      project_id: projectId,
      dial_recipe: dialRecipe,
      brief: brief.trim() ? brief.trim() : null,
    });
  };

  // Escape / scrim / X close the inline add-project form first so the card form
  // (title, type, brief) is never lost mid-add. Only a clean form closes the modal.
  const handleClose = () => {
    if (addingProject) {
      setAddingProject(false);
      return;
    }
    onClose();
  };

  return (
    <Modal
      title="New card"
      subtitle="Cards start in Inbox. Shape and dispatch them from the board."
      onClose={handleClose}
      width={520}
      footer={
        <>
          <button className="btn-ghost" onClick={handleClose}>
            Cancel
          </button>
          <button className="btn-primary" onClick={submit} disabled={!canCreate}>
            Create card
          </button>
        </>
      }
    >
      <label className="field">
        <span className="field-label">Title</span>
        <input
          className="field-input"
          value={title}
          onChange={(e) => setTitle(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) submit();
          }}
          placeholder="What needs doing?"
          autoFocus
        />
      </label>

      <label className="field">
        <span className="field-label">Type</span>
        <div className="type-picker">
          {CARD_TYPES.map((t) => (
            <button
              key={t}
              type="button"
              className={`type-option type-${CARD_TYPE_META[t].tone}${t === type ? " is-active" : ""}`}
              onClick={() => setType(t)}
            >
              {CARD_TYPE_META[t].label}
            </button>
          ))}
        </div>
      </label>

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
        <span className="field-label">
          Process dial <span className="field-optional">how this card runs</span>
        </span>
        <RecipeDial
          value={dialRecipe}
          projectId={projectId}
          projectDefault={project?.default_recipe ?? null}
          onChange={setDialRecipe}
          compact
        />
      </div>

      <label className="field">
        <span className="field-label">
          Brief <span className="field-optional">optional</span>
        </span>
        <textarea
          className="field-input field-textarea"
          value={brief}
          onChange={(e) => setBrief(e.target.value)}
          rows={4}
          placeholder="Context, acceptance criteria, links. You can shape this later."
        />
      </label>
    </Modal>
  );
}
