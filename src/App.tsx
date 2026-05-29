import { useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { api, Availability, ChatSummary, Workflow, WorkflowSummary } from "./lib/ipc";
import { RunView } from "./components/RunView";
import { ChatView } from "./components/ChatView";
import { EditorTarget, WorkflowEditor } from "./components/WorkflowEditor";
import { Icon } from "./components/Icon";
import "./App.css";

const NEW_TEMPLATE = [
  "---",
  "name: New Workflow",
  "inputs: [task]",
  "defaults: { agent: claude, mode: accept_edits }",
  "---",
  "",
  "## plan",
  "```step",
  "agent: claude",
  "mode: plan",
  "output: plan",
  "```",
  "Plan this task: {{ inputs.task }}",
  "",
  "## implement",
  "```step",
  "agent: claude",
  "mode: auto",
  "```",
  "Implement the plan from the previous step.",
  "",
].join("\n");

type View =
  | { kind: "chat"; convId?: string; nonce: number }
  | { kind: "workflow"; path: string }
  | { kind: "editor"; target: EditorTarget; content: string }
  | { kind: "run"; workflow: Workflow; inputs: Record<string, string>; key: number }
  | { kind: "empty" };

function basename(path: string): string {
  return path.split(/[/\\]/).filter(Boolean).pop() ?? path;
}

function formatRelative(unixSec: number): string {
  if (!unixSec) return "—";
  const diff = Math.max(0, Date.now() / 1000 - unixSec);
  if (diff < 60) return "just now";
  if (diff < 3600) return `${Math.floor(diff / 60)}m`;
  if (diff < 86400) return `${Math.floor(diff / 3600)}h`;
  if (diff < 86400 * 7) return `${Math.floor(diff / 86400)}d`;
  return new Date(unixSec * 1000).toLocaleDateString();
}

export default function App() {
  const [project, setProject] = useState<string | null>(null);
  const [agents, setAgents] = useState<Availability[]>([]);
  const [workflows, setWorkflows] = useState<WorkflowSummary[]>([]);
  const [sessions, setSessions] = useState<ChatSummary[]>([]);
  const [selected, setSelected] = useState<Workflow | null>(null);
  const [inputs, setInputs] = useState<Record<string, string>>({});
  const [view, setView] = useState<View>({ kind: "empty" });
  const [sessFilter, setSessFilter] = useState("");
  const [wfFilter, setWfFilter] = useState("");
  const [error, setError] = useState<string | null>(null);

  const refreshAgents = () =>
    api.checkAgents().then(setAgents).catch((e) => setError(String(e)));

  useEffect(() => {
    refreshAgents();
  }, []);

  const refreshSessions = async (dir: string) => {
    try {
      setSessions(await api.listChats(dir));
    } catch {
      // ignore — no chats dir yet
    }
  };

  const refreshWorkflows = async (dir: string) => {
    try {
      setWorkflows(await api.listWorkflows(dir));
    } catch (e) {
      setError(String(e));
      setWorkflows([]);
    }
  };

  const pickProject = async () => {
    setError(null);
    const dir = await open({ directory: true, multiple: false, title: "Select project repository" });
    if (typeof dir === "string") {
      setProject(dir);
      setSelected(null);
      await Promise.all([refreshWorkflows(dir), refreshSessions(dir)]);
      setView({ kind: "chat", nonce: Date.now() });
    }
  };

  const openWorkflow = async (path: string) => {
    setError(null);
    try {
      const wf = await api.getWorkflow(path);
      setSelected(wf);
      setInputs(Object.fromEntries(wf.inputs.map((i) => [i, ""])));
      setView({ kind: "workflow", path });
    } catch (e) {
      setError(String(e));
    }
  };

  const newWorkflow = () => {
    setError(null);
    setView({ kind: "editor", target: { mode: "new" }, content: NEW_TEMPLATE });
  };

  const editCurrentWorkflow = async () => {
    if (!selected?.path) return;
    setError(null);
    try {
      const content = await api.readWorkflowSource(selected.path);
      setView({ kind: "editor", target: { mode: "edit", path: selected.path }, content });
    } catch (e) {
      setError(String(e));
    }
  };

  const onEditorSaved = async (path: string) => {
    if (project) await refreshWorkflows(project);
    await openWorkflow(path);
  };

  const startRun = () => {
    if (selected && project)
      setView({ kind: "run", workflow: selected, inputs, key: Date.now() });
  };

  const openChat = (convId?: string) => {
    setError(null);
    setView({ kind: "chat", convId, nonce: Date.now() });
  };

  const newChat = () => {
    setError(null);
    // nonce forces ChatView remount with no convId — a fresh session.
    setView({ kind: "chat", nonce: Date.now() });
  };

  const deleteSession = async (convId: string) => {
    if (!project) return;
    await api.closeChat(convId).catch(() => {});
    await api.deleteChat(project, convId).catch(() => {});
    await refreshSessions(project);
    if (view.kind === "chat" && view.convId === convId) newChat();
  };

  // Map raw availability to header dots.
  const headerAgents = agents.map((a) => ({
    id: a.id,
    status: a.available ? ("ok" as const) : ("err" as const),
    label: a.available ? `${a.id} ${shortVer(a.version ?? "")}` : `${a.id} not found`,
    title: a.path ?? "not found on PATH",
  }));

  const filteredSessions = sessFilter
    ? sessions.filter((s) => s.title.toLowerCase().includes(sessFilter.toLowerCase()))
    : sessions;

  const filteredWorkflows = wfFilter
    ? workflows.filter((w) => w.name.toLowerCase().includes(wfFilter.toLowerCase()))
    : workflows;

  const canRun =
    !!selected && !!project && selected.inputs.every((i) => (inputs[i] ?? "").trim().length > 0);

  const activeConvId = view.kind === "chat" ? view.convId : undefined;
  const activeWfPath = view.kind === "workflow" ? view.path : undefined;

  const footerStatus = (() => {
    if (!project) return { text: "no project", tone: "warn" as const };
    if (view.kind === "run") return { text: "running…", tone: "warn" as const };
    if (view.kind === "editor") return { text: "editing", tone: "muted" as const };
    return { text: "ready", tone: "ok" as const };
  })();

  return (
    <div className="app">
      <header className="header">
        <div className="brand">
          <div className="brand__mark">A</div>
          <div className="brand__name">arthur</div>
          <div className="brand__build">0.1</div>
        </div>

        <div className="header__center">
          <button
            className="proj"
            onClick={pickProject}
            title={project ? `${project} — click to switch` : "Open a project"}
          >
            <span className="proj__icon" />
            {project ? (
              <>
                <span className="proj__path">{project}</span>
              </>
            ) : (
              <span className="proj__name">Open project…</span>
            )}
            <span className="proj__caret">
              <Icon name="chev-down" size={12} />
            </span>
          </button>
        </div>

        <div className="header__right">
          <div className="agents" title="AI CLIs detected on PATH">
            {headerAgents.length === 0 && (
              <span className="agent">
                <span className="agent__dot is-off" />
                <span className="agent__name">—</span>
              </span>
            )}
            {headerAgents.map((a) => (
              <span className="agent" key={a.id} title={a.title}>
                <span className={`agent__dot is-${a.status}`} />
                <span className="agent__name">{a.id}</span>
                <span className="tooltip">{a.label}</span>
              </span>
            ))}
          </div>
          <button className="icon-btn" title="re-check agents" onClick={refreshAgents}>
            <Icon name="refresh" />
          </button>
        </div>
      </header>

      {error && <div className="error-bar">{error}</div>}

      <div className={`app-body ${project ? "" : "no-rail"}`}>
        {project && (
          <aside className="rail">
            <div className="rail__sessions">
              <div className="rail__head">
                <div className="rail__title">
                  Sessions
                  <span className="rail__count">{sessions.length}</span>
                </div>
                <div className="rail__actions">
                  <button className="icon-btn" title="New chat session" onClick={newChat}>
                    <Icon name="plus" size={13} />
                  </button>
                </div>
              </div>
              <div className="rail__search">
                <Icon name="search" />
                <input
                  placeholder="Filter sessions…"
                  value={sessFilter}
                  onChange={(e) => setSessFilter(e.target.value)}
                />
              </div>
              <div className="rail__list">
                {filteredSessions.length === 0 ? (
                  <div className="rail__empty">
                    No chat sessions yet. Click <code>+</code> to start one.
                  </div>
                ) : (
                  filteredSessions.map((s) => (
                    <div
                      key={s.conv_id}
                      className={`sess ${activeConvId === s.conv_id ? "is-active" : ""}`}
                      onClick={() => openChat(s.conv_id)}
                    >
                      <span className="sess__statusdot s-idle" />
                      <div className="sess__body">
                        <div className="sess__title">{s.title || "(untitled)"}</div>
                        <div className="sess__meta">
                          <span
                            className="abadge abadge--pill"
                            data-agent={s.agent}
                          >
                            {s.agent}
                          </span>
                          <span className="dot">·</span>
                          <span>{formatRelative(s.updated_at)}</span>
                          <span className="dot">·</span>
                          <span>{s.message_count} msg</span>
                        </div>
                      </div>
                      <button
                        className="sess__del"
                        title="Delete session"
                        onClick={(e) => {
                          e.stopPropagation();
                          deleteSession(s.conv_id);
                        }}
                      >
                        ×
                      </button>
                    </div>
                  ))
                )}
              </div>
            </div>

            <div className="rail__workflows">
              <div className="rail__head">
                <div className="rail__title">
                  Workflows
                  <span className="rail__count">{workflows.length}</span>
                </div>
                <div className="rail__actions">
                  <button className="icon-btn" title="New workflow" onClick={newWorkflow}>
                    <Icon name="plus" size={13} />
                  </button>
                  <button
                    className="icon-btn"
                    title="Reload workflows"
                    onClick={() => project && refreshWorkflows(project)}
                  >
                    <Icon name="refresh" size={12} />
                  </button>
                </div>
              </div>
              <div className="rail__search">
                <Icon name="search" />
                <input
                  placeholder="Filter workflows…"
                  value={wfFilter}
                  onChange={(e) => setWfFilter(e.target.value)}
                />
              </div>
              <div className="rail__list">
                {filteredWorkflows.length === 0 ? (
                  <div className="rail__empty">
                    No playbooks in <code>.arthur/workflows</code>.
                  </div>
                ) : (
                  filteredWorkflows.map((w) => (
                    <div
                      key={w.path}
                      className={`wf ${activeWfPath === w.path ? "is-active" : ""}`}
                      onClick={() => openWorkflow(w.path)}
                    >
                      <div className="wf__icon">
                        <Icon name="play-step" size={10} />
                      </div>
                      <div>
                        <div className="wf__name">{w.name}</div>
                      </div>
                      <span className={`wf__source ${w.source}`}>{w.source}</span>
                    </div>
                  ))
                )}
              </div>
            </div>
          </aside>
        )}

        {!project ? (
          <div className="empty">
            <div className="empty__inner">
              <div className="empty__hello">arthur · ai agents desk</div>
              <h1 className="empty__h1">Open a project to begin.</h1>
              <div className="empty__sub">
                Arthur orchestrates Claude, Codex, and Gemini CLIs against your local repo —
                chats, workflows, and live runs in one desk.
              </div>
              <button className="btn-primary" onClick={pickProject}>
                <Icon name="folder" size={12} /> Open project…
              </button>
            </div>
          </div>
        ) : view.kind === "chat" ? (
          <ChatView
            key={`${view.convId ?? "new"}-${view.nonce}`}
            projectDir={project}
            agents={agents}
            workflows={workflows}
            convId={view.convId}
            onSessionsChanged={() => refreshSessions(project)}
          />
        ) : view.kind === "editor" ? (
          <WorkflowEditor
            target={view.target}
            initialContent={view.content}
            projectDir={project}
            agents={agents}
            onClose={() => setView({ kind: "empty" })}
            onSaved={onEditorSaved}
          />
        ) : view.kind === "run" ? (
          <RunView
            key={view.key}
            workflow={view.workflow}
            projectDir={project}
            inputs={view.inputs}
            onBack={() => selected && openWorkflow(selected.path!)}
          />
        ) : view.kind === "workflow" && selected ? (
          <div className="main">
            <div className="main__head">
              <div className="main__title">
                <Icon name="play-step" size={12} />
                <span className="main__title-text">{selected.name}</span>
              </div>
              <div className="main__head-right">
                <span className="tag">{selected.steps.length} steps</span>
                {selected.defaults.agent && (
                  <span className="abadge abadge--pill" data-agent={selected.defaults.agent}>
                    {selected.defaults.agent}
                  </span>
                )}
                <button className="btn-ghost" onClick={editCurrentWorkflow}>
                  <Icon name="edit" size={12} /> Edit
                </button>
                <button className="btn-primary" disabled={!canRun} onClick={startRun}>
                  <Icon name="play-step" size={11} /> Run workflow
                </button>
              </div>
            </div>
            <div className="detail">
              {selected.inputs.length > 0 && (
                <div className="detail__inputs">
                  {selected.inputs.map((i) => (
                    <label key={i} className="detail__field">
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
              <ol className="detail__steps">
                {selected.steps.map((s) => (
                  <li key={s.id} className="detail__step">
                    <span className="title">{s.title}</span>
                    <span className="meta">
                      {[
                        s.config.agent ?? selected.defaults.agent,
                        s.config.model,
                        s.config.mode ?? selected.defaults.mode,
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
              {!canRun && selected.inputs.length > 0 && (
                <div className="hint">Fill all inputs to run.</div>
              )}
            </div>
            <div /> {/* footer slot reserved by grid */}
          </div>
        ) : (
          <div className="empty">
            <div className="empty__inner">
              <div className="empty__hello">arthur · pick something</div>
              <h1 className="empty__h1">Start a chat or open a workflow.</h1>
              <div className="empty__sub">
                Use the left rail. Sessions are persistent across restarts; workflows live in{" "}
                <code>.arthur/workflows</code>.
              </div>
              <button className="btn-primary" onClick={newChat}>
                <Icon name="plus" size={12} /> New chat
              </button>
            </div>
          </div>
        )}
      </div>

      <div className="footer">
        <span>
          <Icon name="folder" size={11} />{" "}
          <span className="footer__path">{project ? basename(project) : "no project"}</span>
        </span>
        <span className="sep">·</span>
        <span>
          <Icon name="branch" size={11} /> main
        </span>
        <span className="sep">·</span>
        <span className={footerStatus.tone}>●</span> <span>{footerStatus.text}</span>
        <div className="right">
          <span>
            {agents.filter((a) => a.available).length}/{agents.length || 0} agents
          </span>
          <span className="sep">·</span>
          <span>arthur 0.1</span>
        </div>
      </div>
    </div>
  );
}

function shortVer(v: string): string {
  const m = v.match(/\d+\.\d+\.\d+/);
  return m ? m[0] : v;
}
