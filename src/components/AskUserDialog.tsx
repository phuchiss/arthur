import { useState } from "react";
import { UserQuestion } from "../lib/ipc";
import { Icon } from "./Icon";

/** Sequential multi-question dialog — one question at a time, Claude-Desktop style. */
export function AskUserDialog({
  questions,
  onSubmit,
  onCancel,
}: {
  questions: UserQuestion[];
  onSubmit: (answers: string[]) => void;
  onCancel: () => void;
}) {
  const [idx, setIdx] = useState(0);
  // Per-question state. `picked` holds the chosen option labels; `other` is
  // free-text that, when non-empty, augments / replaces the picks.
  const [picked, setPicked] = useState<string[][]>(() => questions.map(() => []));
  const [other, setOther] = useState<string[]>(() => questions.map(() => ""));

  const current = questions[idx];
  if (!current) return null;

  const isLast = idx === questions.length - 1;
  const currentPicked = picked[idx];
  const currentOther = other[idx];

  // Build the consolidated answer for the current question.
  const answerOf = (i: number): string => {
    const parts: string[] = [...picked[i]];
    if (other[i].trim()) parts.push(other[i].trim());
    return parts.join(", ");
  };

  const canAdvance = currentPicked.length > 0 || currentOther.trim().length > 0;

  const togglePick = (label: string) => {
    setPicked((prev) => {
      const next = prev.map((arr, i) => (i === idx ? [...arr] : arr));
      const arr = next[idx];
      if (current.multi_select) {
        const at = arr.indexOf(label);
        if (at >= 0) arr.splice(at, 1);
        else arr.push(label);
      } else {
        next[idx] = arr[0] === label ? [] : [label];
      }
      return next;
    });
  };

  const advance = () => {
    if (!canAdvance) return;
    if (isLast) {
      onSubmit(questions.map((_, i) => answerOf(i)));
    } else {
      setIdx(idx + 1);
    }
  };

  const back = () => {
    if (idx > 0) setIdx(idx - 1);
  };

  return (
    <div className="modal-backdrop" onClick={onCancel}>
      <div className="modal ask-modal" onClick={(e) => e.stopPropagation()}>
        <div className="ask-modal__top">
          <div className="ask-modal__progress">
            Question {idx + 1} of {questions.length}
          </div>
          <button className="icon-btn" title="Skip dialog" onClick={onCancel}>
            <Icon name="x" size={13} />
          </button>
        </div>

        {current.header && <div className="ask-modal__chip">{current.header}</div>}
        <h3 className="ask-modal__q">{current.question}</h3>
        {current.multi_select && (
          <div className="ask-modal__hint">Pick one or more.</div>
        )}

        <div className="ask-modal__options">
          {current.options.map((opt) => {
            const isOn = currentPicked.includes(opt.label);
            return (
              <button
                key={opt.label}
                className={`ask-opt ${isOn ? "is-on" : ""}`}
                onClick={() => togglePick(opt.label)}
              >
                <span className="ask-opt__check">
                  {isOn && <Icon name="check" size={11} />}
                </span>
                <span className="ask-opt__body">
                  <span className="ask-opt__label">{opt.label}</span>
                  {opt.description && (
                    <span className="ask-opt__desc">{opt.description}</span>
                  )}
                </span>
              </button>
            );
          })}

          <div className={`ask-opt ask-opt--other ${currentOther ? "is-on" : ""}`}>
            <span className="ask-opt__check">
              {currentOther.trim() && <Icon name="check" size={11} />}
            </span>
            <span className="ask-opt__body">
              <span className="ask-opt__label">Other</span>
              <input
                className="ask-opt__input"
                placeholder="Type your own answer…"
                value={currentOther}
                onChange={(e) =>
                  setOther((prev) => prev.map((v, i) => (i === idx ? e.target.value : v)))
                }
                onKeyDown={(e) => {
                  if (e.key === "Enter" && canAdvance) {
                    e.preventDefault();
                    advance();
                  }
                }}
              />
            </span>
          </div>
        </div>

        <div className="ask-modal__actions">
          <button className="btn-ghost" onClick={onCancel}>
            Cancel
          </button>
          <span className="composer__spacer" />
          {idx > 0 && (
            <button className="btn-ghost" onClick={back}>
              Back
            </button>
          )}
          <button className="btn-primary" disabled={!canAdvance} onClick={advance}>
            {isLast ? "Send answers" : "Next"}
            <Icon name="chev-right" size={11} />
          </button>
        </div>
      </div>
    </div>
  );
}
