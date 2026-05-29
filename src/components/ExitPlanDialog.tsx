import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { Mode, MODE_LABELS } from "../lib/ipc";
import { Icon } from "./Icon";

/**
 * Confirmation dialog Arthur shows when Claude's `ExitPlanMode` tool fires
 * over ACP — mirrors what Claude Desktop / Claude Code's TTY prompt does:
 * present the plan, ask the user to approve, and pick what permission mode
 * the follow-up turn should run under.
 */
export function ExitPlanDialog({
  plan,
  onApprove,
  onKeepPlanning,
  onClose,
}: {
  plan: string | null;
  onApprove: (mode: Mode) => void;
  onKeepPlanning: () => void;
  onClose: () => void;
}) {
  return (
    <div className="modal-backdrop" onClick={onClose}>
      <div className="modal exit-plan-modal" onClick={(e) => e.stopPropagation()}>
        <div className="ask-modal__top">
          <div className="ask-modal__progress">Plan ready</div>
          <button className="icon-btn" title="Dismiss" onClick={onClose}>
            <Icon name="x" size={13} />
          </button>
        </div>

        <h3 className="ask-modal__q">Claude is ready to start coding.</h3>
        <div className="ask-modal__hint">
          Approve the plan to leave Plan mode and run the implementation.
        </div>

        {plan && (
          <div className="exit-plan__preview chat-md">
            <ReactMarkdown remarkPlugins={[remarkGfm]}>{plan}</ReactMarkdown>
          </div>
        )}

        <div className="exit-plan__choices">
          <button className="btn-primary" onClick={() => onApprove("accept_edits")}>
            Approve · switch to {MODE_LABELS.accept_edits}
            <Icon name="chev-right" size={11} />
          </button>
          <button className="btn-ghost" onClick={() => onApprove("auto")}>
            Approve · switch to {MODE_LABELS.auto}
            <Icon name="chev-right" size={11} />
          </button>
          <button className="btn-ghost" onClick={onKeepPlanning}>
            Keep planning
          </button>
        </div>
      </div>
    </div>
  );
}
