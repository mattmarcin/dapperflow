// The dispatch-time consent modal for privileged recipes. Renders the specced
// structured error (code: recipe_grant_required) as a per-project grant decision:
// exactly what is elevated, verbatim (security.md / Recipe trust tiers). Granting
// records project::recipe::hash and resumes the parked dispatch; declining closes.

import { useStore } from "../state/store";
import { Modal } from "./Modal";
import { ConsentSummary } from "./RecipeDial";

export function GrantModal() {
  const store = useStore();
  const err = store.pendingGrant;
  if (!err) return null;

  return (
    <Modal
      title="This recipe needs a grant"
      subtitle={err.message}
      onClose={store.declinePendingRecipe}
      width={520}
    >
      <ConsentSummary
        recipe={{ name: err.recipe, privileges: err.privileges }}
        projectId={err.project_id || null}
        onGrant={store.grantPendingRecipe}
        onDefer={store.declinePendingRecipe}
        deferLabel="Not now"
      />
    </Modal>
  );
}
