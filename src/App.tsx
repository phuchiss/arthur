import { useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { api, Availability, Workflow, WorkflowSummary } from "./lib/ipc";
import { RunView } from "./components/RunView";
import "./App.css";

export default function App() {
  const [project, setProject] = useState<string | null>(null);
  const [agents, setAgents] = useState<Availability[]>([]);
  const [workflows, setWorkflows] = useState<WorkflowSummary[]>([]);
  const [selected, setSelected] = useState<Workflow | null>(null);
  const [inputs, setInputs] = useState<Record<string, string>>({});
  const [run, setRun] = useState<{ workflow: Workflow; inputs: Record<string, string>; key: number } | null>(null);
  const [error, setError] = useState<string | null>(null);

  const refreshAgents = () => api.checkAgents().then(setAgents).catch((e) => setError(String(e)));
  useEffect(() => {
    refreshAgents();
  }, []);

  const pickProject = async () => {
    setError(null);
    const dir = await open({ directory: true, multiple: false, title: "Select project repository" });
    if (typeof dir === "string") {
      setProject(dir);
      setSelected(null);
      setRun(null);
      try {
        setWorkflows(await api.listWorkflows(dir));
      } catch (e) {
        setError(String(e));
        setWorkflows([]);
      }
    }
  };

  const reloadWorkflows = async () => {
    if (!project) return;
    try {
      setWorkflows(await api.listWorkflows(project));
    } catch (e) {
      setError(String(e));
    }
  };

  const selectWorkflow = async (path: string) => {
    setError(null);
    setRun(null);
    try {
      const wf = await api.getWorkflow(path);
      setSelected(wf);
      setInputs(Object.fromEntries(wf.inputs.map((i) => [i, ""])));
    } catch (e) {
      setError(String(e));
    }
  };

  const canRun =
    !!selected && !!project && selected.inputs.every((i) => (inputs[i] ?? "").trim().length > 0);

  const startRun = () => {
    if (selected && project) setRun({ workflow: selected, inputs, key: Date.now() });
  };

  return (
    <div className="app">
      <header className="topbar">
        <div className="brand">arthur</div>
        <div className="agents">
          {agents.map((a) => (
            <span
              key={a.id}
              className={`agent ${a.available ? "ok" : "missing"}`}
              title={a.path ?? "not found on PATH"}
            >
              {a.available ? "●" : "○"} {a.id}
              {a.version ? ` ${shortVer(a.version)}` : ""}
            </span>
          ))}
          <button className="link" title="re-check agents" onClick={refreshAgents}>
            ↻
          </button>
        </div>
        <div className="project">
          <button className="ghost" onClick={pickProject}>
            {project ? "Change project" : "Open project…"}
          </button>
        </div>
      </header>

      {project && (
        <div className="path-bar" title={project}>
          {project}
        </div>
      )}
      {error && <div className="error-bar">{error}</div>}

      <div className="main">
        <aside className="sidebar">
          <div className="sidebar-head">
            <span>Workflows</span>
            {project && (
              <button className="link" onClick={reloadWorkflows}>
                ↻
              </button>
            )}
          </div>
          {!project && (
            <p className="hint">
              Open a project to list its <code>.arthur/workflows</code>.
            </p>
          )}
          {project && workflows.length === 0 && (
            <p className="hint">
              No playbooks in <code>.arthur/workflows</code>.
            </p>
          )}
          <ul className="wf-list">
            {workflows.map((w) => (
              <li
                key={w.path}
                className={selected?.path === w.path ? "active" : ""}
                onClick={() => selectWorkflow(w.path)}
              >
                <span>{w.name}</span>
                <span className={`wf-source ${w.source}`}>{w.source}</span>
              </li>
            ))}
          </ul>
        </aside>

        <section className="content">
          {run ? (
            <RunView
              key={run.key}
              workflow={run.workflow}
              projectDir={project!}
              inputs={run.inputs}
              onBack={() => setRun(null)}
            />
          ) : selected ? (
            <div className="detail">
              <h2>{selected.name}</h2>
              {selected.inputs.length > 0 && (
                <div className="inputs">
                  {selected.inputs.map((i) => (
                    <label key={i} className="field">
                      <span>{i}</span>
                      <textarea
                        value={inputs[i] ?? ""}
                        rows={2}
                        onChange={(e) => setInputs((p) => ({ ...p, [i]: e.target.value }))}
                        placeholder={`Enter ${i}…`}
                      />
                    </label>
                  ))}
                </div>
              )}
              <ol className="steps preview">
                {selected.steps.map((s) => (
                  <li key={s.id} className="step">
                    <span className="step-title">{s.title}</span>
                    <span className="step-meta">
                      {[
                        s.config.agent ?? selected.defaults.agent,
                        s.config.model,
                        s.config.approval ? "approval" : null,
                        s.config.retry ? `retry×${s.config.retry.max}` : null,
                        s.config.when ? "when" : null,
                        s.config.goto ? `goto ${s.config.goto}` : null,
                      ]
                        .filter(Boolean)
                        .join(" · ")}
                    </span>
                  </li>
                ))}
              </ol>
              <div className="run-actions">
                <button className="primary" disabled={!canRun} onClick={startRun}>
                  Run workflow
                </button>
                {!canRun && selected.inputs.length > 0 && (
                  <span className="hint">Fill all inputs to run.</span>
                )}
              </div>
            </div>
          ) : (
            <div className="empty">{project ? "Select a workflow." : "Open a project to begin."}</div>
          )}
        </section>
      </div>
    </div>
  );
}

function shortVer(v: string): string {
  const m = v.match(/\d+\.\d+\.\d+/);
  return m ? m[0] : v;
}
