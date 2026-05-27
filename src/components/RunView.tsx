import { useEffect, useRef, useState } from "react";
import { api, Channel, LogEvent, Workflow } from "../lib/ipc";

type Status = "pending" | "running" | "done" | "failed" | "skipped" | "awaiting";
type StepState = { status: Status; exit?: number; agent?: string; attempt?: number };
type LogLine = { kind: "out" | "err" | "info"; stepId?: string; text: string };

const STATUS_LABEL: Record<Status, string> = {
  pending: "·",
  running: "▶",
  done: "✓",
  failed: "✗",
  skipped: "⤼",
  awaiting: "⏸",
};

export function RunView({
  workflow,
  projectDir,
  inputs,
  onBack,
}: {
  workflow: Workflow;
  projectDir: string;
  inputs: Record<string, string>;
  onBack: () => void;
}) {
  const [runId, setRunId] = useState<string | null>(null);
  const [steps, setSteps] = useState<Record<string, StepState>>(() =>
    Object.fromEntries(workflow.steps.map((s) => [s.id, { status: "pending" as Status }]))
  );
  const [logs, setLogs] = useState<LogLine[]>([]);
  const [awaiting, setAwaiting] = useState<{ stepId: string; title: string } | null>(null);
  const [finished, setFinished] = useState<string | null>(null);

  const started = useRef(false);
  const runIdRef = useRef<string | null>(null);
  const finishedRef = useRef(false);
  const logEnd = useRef<HTMLDivElement | null>(null);

  const setStep = (id: string, patch: Partial<StepState>) =>
    setSteps((prev) => ({ ...prev, [id]: { ...prev[id], ...patch } }));
  const addLog = (line: LogLine) => setLogs((prev) => [...prev, line]);
  const finish = (outcome: string) => {
    finishedRef.current = true;
    setFinished(outcome);
  };

  useEffect(() => {
    if (started.current) return; // guard StrictMode double-mount
    started.current = true;

    const channel = new Channel<LogEvent>();
    channel.onmessage = (ev) => {
      switch (ev.type) {
        case "run_started":
          setRunId(ev.run_id);
          runIdRef.current = ev.run_id;
          addLog({ kind: "info", text: `▶ run started: ${ev.workflow}` });
          break;
        case "step_started":
          setStep(ev.step_id, { status: "running", agent: ev.agent, attempt: ev.attempt });
          addLog({
            kind: "info",
            stepId: ev.step_id,
            text: `● ${ev.title} — ${ev.agent}${ev.model ? ` (${ev.model})` : ""}${
              ev.attempt > 1 ? ` · attempt ${ev.attempt}` : ""
            }`,
          });
          break;
        case "stdout":
          addLog({ kind: "out", stepId: ev.step_id, text: ev.line });
          break;
        case "stderr":
          addLog({ kind: "err", stepId: ev.step_id, text: ev.line });
          break;
        case "step_finished":
          setStep(ev.step_id, { status: ev.exit_code === 0 ? "done" : "failed", exit: ev.exit_code });
          break;
        case "step_skipped":
          setStep(ev.step_id, { status: "skipped" });
          addLog({ kind: "info", stepId: ev.step_id, text: `⤼ skipped ${ev.step_id}` });
          break;
        case "retrying":
          addLog({ kind: "info", stepId: ev.step_id, text: `↻ retry ${ev.step_id} (attempt ${ev.attempt})` });
          break;
        case "goto":
          addLog({ kind: "info", text: `↪ goto ${ev.from} → ${ev.to}` });
          break;
        case "awaiting_approval":
          setStep(ev.step_id, { status: "awaiting" });
          setAwaiting({ stepId: ev.step_id, title: ev.title });
          addLog({ kind: "info", stepId: ev.step_id, text: `⏸ awaiting approval: ${ev.title}` });
          break;
        case "approved":
          setAwaiting(null);
          addLog({ kind: "info", stepId: ev.step_id, text: `✓ approved ${ev.step_id}` });
          break;
        case "rejected":
          setAwaiting(null);
          finish("rejected");
          addLog({ kind: "info", stepId: ev.step_id, text: `✕ rejected ${ev.step_id}` });
          break;
        case "cancelled":
          finish("cancelled");
          addLog({ kind: "info", text: "■ cancelled" });
          break;
        case "done":
          finish("done");
          addLog({ kind: "info", text: "✔ done" });
          break;
        case "error":
          finish("error");
          addLog({ kind: "err", text: `error: ${ev.message}` });
          break;
      }
    };

    api
      .startRun({ workflowPath: workflow.path ?? "", projectDir, inputs }, channel)
      .then((id) => {
        setRunId(id);
        runIdRef.current = id;
      })
      .catch((e) => {
        finish("error");
        addLog({ kind: "err", text: String(e) });
      });

    return () => {
      if (runIdRef.current && !finishedRef.current) {
        api.cancel(runIdRef.current).catch(() => {});
      }
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    logEnd.current?.scrollIntoView({ behavior: "smooth", block: "end" });
  }, [logs]);

  const decide = (decision: "approve" | "reject") => {
    if (runIdRef.current) api.approve(runIdRef.current, decision).catch(() => {});
    setAwaiting(null);
  };

  return (
    <div className="run">
      <div className="run-header">
        <button className="link" onClick={onBack}>
          ← back
        </button>
        <span className="run-title">{workflow.name}</span>
        <span className={`run-status ${finished ?? "active"}`}>{finished ?? "running…"}</span>
        {!finished && runId && (
          <button className="danger" onClick={() => api.cancel(runId).catch(() => {})}>
            Cancel
          </button>
        )}
      </div>

      <div className="run-body">
        <ol className="steps">
          {workflow.steps.map((s) => {
            const st = steps[s.id];
            return (
              <li key={s.id} className={`step ${st.status}`}>
                <span className="step-badge">{STATUS_LABEL[st.status]}</span>
                <span className="step-title">{s.title}</span>
                <span className="step-meta">
                  {st.agent ?? s.config.agent ?? workflow.defaults.agent ?? ""}
                  {st.exit !== undefined ? ` · exit ${st.exit}` : ""}
                </span>
              </li>
            );
          })}
        </ol>

        <div className="logs">
          {logs.map((l, i) => (
            <div key={i} className={`log ${l.kind}`}>
              {l.stepId ? <span className="log-step">[{l.stepId}]</span> : null} {l.text}
            </div>
          ))}
          <div ref={logEnd} />
        </div>
      </div>

      {awaiting && (
        <div className="modal-backdrop">
          <div className="modal">
            <h3>Approval required</h3>
            <p>
              Step <strong>{awaiting.title}</strong> is waiting. Approve to continue or reject to stop the run.
            </p>
            <div className="modal-actions">
              <button className="danger" onClick={() => decide("reject")}>
                Reject
              </button>
              <button className="primary" onClick={() => decide("approve")}>
                Approve
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
