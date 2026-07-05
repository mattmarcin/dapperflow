import { Modal } from "./Modal";

interface Props {
  title: string;
  body: string;
  confirmLabel: string;
  tone?: "danger" | "default";
  onConfirm: () => void;
  onCancel: () => void;
}

export function ConfirmDialog({ title, body, confirmLabel, tone = "default", onConfirm, onCancel }: Props) {
  return (
    <Modal
      title={title}
      onClose={onCancel}
      width={420}
      footer={
        <>
          <button className="btn-ghost" onClick={onCancel}>
            Keep it
          </button>
          <button className={tone === "danger" ? "btn-danger" : "btn-primary"} onClick={onConfirm}>
            {confirmLabel}
          </button>
        </>
      }
    >
      <p className="confirm-body">{body}</p>
    </Modal>
  );
}
