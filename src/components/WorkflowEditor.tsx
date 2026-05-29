import { useEffect, useState } from "react";
import { api, Availability, Step, Workflow } from "../lib/ipc";
import { Icon } from "./Icon";

const AGENTS = ["claude", "codex", "gemini"];

export type EditorTarget = { mode: "edit"; path: string } | { mode: "new" };

export function WorkflowEditor({
  target,
  initialContent,
  projectDir,
  agents,
  onClose,
  onSaved,
}: {
  target: EditorTarget;
  initialContent: string;
  projectDir: string;
  agents: Availability[];
  onClose: () => void;
  onSaved: (path: string) => void;
}) {
  const [content, setContent] = useState(initialContent);
  const [preview, setPreview] = useState<Workflow | null>(null);
  const [previewError, setPreviewError] = useState<string | null>(null);
  const [showPreview, setShowPreview] = useState(true);
  const [name, setName] = useState("");
  const [scope, setScope] = useState<"project" | "global">("project");
  const [dirty, setDirty] = useState(false);
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);

  const availableAgents = AGENTS.filter((id) => agents.find((a) => a.id === id)?.available);
  const [improveAgent, setImproveAgent] = useState(availableAgents[0] ?? "claude");
  const [instruction, setInstruction] = useState("");
  const [improving, setImproving] = useState(false);
  const [improveId, setImproveId] = useState<string | null>(null);
  const [improveError, setImproveError] = useState<string | null>(null);
  const [improveNote, setImproveNote] = useState<string | null>(null);
  const [revertTo, setRevertTo] = useState<string | null>(null);

  useEffect(() => {
    if (!showPreview) return;
    const path = target.mode === "edit" ? target.path : undefined;
    const handle = setTimeout(() => {
      api
        .parseWorkflowSource(content, path)
        .then((wf) => {
          setPreview(wf);
          setPreviewError(null);
        })
        .catch((e) => {
          setPreview(null);
          setPreviewError(String(e));
        });
    }, 250);
    return () => clearTimeout(handle);
  }, [content, showPreview, target]);

  useEffect(() => {
    if (availableAgents.length && !availableAgents.includes(improveAgent)) {
      setImproveAgent(availableAgents[0]);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [agents, improveAgent]);

  const onChange = (v: string) => {
    setContent(v);
    setDirty(true);
  };

  const onKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === "Tab") {
      e.preventDefault();
      document.execCommand("insertText", false, "  ");
    }
  };

  const canSave =
    !saving &&
    content.trim().length > 0 &&
    (target.mode === "edit" || name.trim().length > 0);

  const save = async () => {
    setSaving(true);
    setSaveError(null);
    try {
      let path: string;
      if (target.mode === "edit") {
        await api.saveWorkflow(target.path, content);
        path = target.path;
      } else {
        path = await api.createWorkflow({ projectDir, scope, fileName: name, content });
      }
      setDirty(false);
      onSaved(path);
    } catch (e) {
      setSaveError(String(e));
    } finally {
      setSaving(false);
    }
  };

  const improve = async () => {
    const id = crypto.randomUUID();
    const before = content;
    setImproving(true);
    setImproveId(id);
    setImproveError(null);
    setImproveNote(null);
    try {
      const improved = await api.improveWorkflow({
        improveId: id,
        agent: improveAgent,
        content,
        instruction: instruction.trim() || undefined,
        projectDir,
      });
      setContent(improved);
      setDirty(true);
      setRevertTo(before);
      setImproveNote(`Rewritten by ${improveAgent}.`);
    } catch (e) {
      if (String(e).includes("cancelled")) {
        setImproveNote("Improvement cancelled.");
      } else {
        setImproveError(String(e));
      }
    } finally {
      setImproving(false);
      setImproveId(null);
    }
  };

  const cancelImprove = () => {
    if (improveId) api.cancelImprove(improveId).catch(() => {});
  };

  const revert = () => {
    if (revertTo === null) return;
    setContent(revertTo);
    setRevertTo(null);
    setImproveNote(null);
    setDirty(true);
  };

  const close = () => {
    if (dirty && !window.confirm("Discard unsaved changes?")) return;
    onClose();
  };

  return (
    <div className="main">
      <div className="main__head">
        <button className="icon-btn" title="Back" onClick={close}>
          <Icon name="back" size={13} />
        </button>
        <div className="main__title">
          <Icon name="edit" size={12} />
          {target.mode === "new" ? (
            <input
              className="editor__name"
              value={name}
              placeholder="file-name"
              onChange={(e) => setName(e.target.value)}
            />
          ) : (
            <span className="main__title-text" title={target.path}>
              {fileName(target.path)}
            </span>
          )}
        </div>
        <div className="main__head-right">
          {target.mode === "new" && (
            <select
              className="field-select"
              value={scope}
              onChange={(e) => setScope(e.target.value as "project" | "global")}
            >
              <option value="project">project</option>
              <option value="global">global</option>
            </select>
          )}
          <button
            className={`btn-ghost ${showPreview ? "is-active" : ""}`}
            onClick={() => setShowPreview((s) => !s)}
            title="Toggle preview"
          >
            preview
          </button>
          <button className="btn-primary" disabled={!canSave} onClick={save}>
            {saving ? "Saving…" : target.mode === "new" ? "Create" : "Save"}
          </button>
        </div>
      </div>

      <div className="editor__ai">
        <span className="editor__ai-label">✨ Improve with</span>
        <select
          className="field-select"
          value={improveAgent}
          disabled={improving || availableAgents.length === 0}
          onChange={(e) => setImproveAgent(e.target.value)}
        >
          {(availableAgents.length ? availableAgents : AGENTS).map((id) => (
            <option key={id} value={id}>
              {id}
            </option>
          ))}
        </select>
        <input
          className="editor__instruction"
          value={instruction}
          placeholder="Optional: what to focus on…"
          disabled={improving}
          onChange={(e) => setInstruction(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && !improving && availableAgents.length) improve();
          }}
        />
        {improving ? (
          <button className="btn-danger" onClick={cancelImprove}>
            Cancel
          </button>
        ) : (
          <button
            className="btn-ghost"
            disabled={availableAgents.length === 0 || content.trim().length === 0}
            title={availableAgents.length === 0 ? "No CLIs detected on PATH" : undefined}
            onClick={improve}
          >
            Improve
          </button>
        )}
        {improving && <span className="editor__ai-status">running {improveAgent}…</span>}
        {!improving && revertTo !== null && (
          <button className="icon-btn" title="Revert" onClick={revert}>
            <Icon name="back" size={12} />
          </button>
        )}
        {!improving && improveNote && <span className="editor__ai-note">{improveNote}</span>}
      </div>

      {improveError && <div className="error-bar">{improveError}</div>}
      {saveError && <div className="error-bar">{saveError}</div>}

      <div className={`editor__body ${showPreview ? "" : "no-preview"}`}>
        <textarea
          className="editor__text"
          value={content}
          spellCheck={false}
          readOnly={improving}
          onChange={(e) => onChange(e.target.value)}
          onKeyDown={onKeyDown}
        />
        {showPreview && (
          <div className="editor__preview">
            {previewError ? (
              <div className="editor__preview-error">{previewError}</div>
            ) : preview ? (
              <>
                <h3>{preview.name}</h3>
                {preview.inputs.length > 0 && (
                  <div className="editor__preview-inputs">
                    {preview.inputs.map((i) => (
                      <span key={i} className="chip">
                        <span className="chip__dot" />
                        {i}
                      </span>
                    ))}
                  </div>
                )}
                <ol className="detail__steps">
                  {preview.steps.map((s) => (
                    <li key={s.id} className="detail__step">
                      <span className="title">{s.title}</span>
                      <span className="meta">{stepMeta(s, preview)}</span>
                    </li>
                  ))}
                </ol>
              </>
            ) : (
              <div className="hint">Parsing…</div>
            )}
          </div>
        )}
      </div>
    </div>
  );
}

function stepMeta(s: Step, wf: Workflow): string {
  return [
    s.config.agent ?? wf.defaults.agent,
    s.config.model,
    s.config.mode ?? wf.defaults.mode,
    s.config.approval ? "approval" : null,
    s.config.retry ? `retry×${s.config.retry.max}` : null,
    s.config.when ? "when" : null,
    s.config.goto ? `goto ${s.config.goto}` : null,
  ]
    .filter(Boolean)
    .join(" · ");
}

function fileName(path: string): string {
  return path.split(/[/\\]/).pop() ?? path;
}
