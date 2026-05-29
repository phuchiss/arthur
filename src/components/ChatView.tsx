import { useEffect, useRef, useState } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { openUrl } from "@tauri-apps/plugin-opener";
import { open } from "@tauri-apps/plugin-dialog";
import {
  api,
  Availability,
  Channel,
  CommandInfo,
  LogEvent,
  Mode,
  MODE_LABELS,
  PermissionOption,
  UserQuestion,
  WorkflowSummary,
} from "../lib/ipc";
import { Icon } from "./Icon";
import { AskUserDialog } from "./AskUserDialog";
import { ExitPlanDialog } from "./ExitPlanDialog";
import { FilesPanel } from "./FilesPanel";

const AGENTS = ["claude", "codex", "gemini"];
const MODES: Mode[] = ["ask", "accept_edits", "plan", "auto"];
type Transport = "cli" | "acp";

// Old "autonomy" values written by previous app versions need to map to the
// new mode enum so saved chats keep working.
function normalizeMode(s: string | undefined): Mode {
  switch ((s ?? "").toLowerCase()) {
    case "ask":
      return "ask";
    case "accept_edits":
    case "edit":
      return "accept_edits";
    case "plan":
    case "read":
      return "plan";
    case "auto":
    case "full":
      return "auto";
    default:
      return "accept_edits";
  }
}

const MODELS: Record<string, { label: string; value: string }[]> = {
  claude: [
    { label: "Opus 4.8", value: "claude-opus-4-8" },
    { label: "Opus 4.7", value: "claude-opus-4-7" },
    { label: "Opus 4.6", value: "claude-opus-4-6" },
    { label: "Sonnet 4.6", value: "claude-sonnet-4-6" },
    { label: "Haiku 4.5", value: "claude-haiku-4-5-20251001" },
  ],
  codex: [
    { label: "GPT-5", value: "gpt-5" },
    { label: "GPT-5 Codex", value: "gpt-5-codex" },
  ],
  gemini: [
    { label: "2.5 Pro", value: "gemini-2.5-pro" },
    { label: "2.5 Flash", value: "gemini-2.5-flash" },
  ],
};

type Message = {
  role: "user" | "assistant";
  text: string;
  streaming?: boolean;
  error?: boolean;
  agent?: string;
};

// Lines the backend emits as discrete activity (tool calls, thoughts, plans,
// warnings) rather than prose — rendered as compact rows instead of markdown.
const ACTIVITY_RE = /^\s*(🔧|💭|📋|⚠)/u;

type Segment = { kind: "text" | "tool"; text: string };

function splitSegments(text: string): Segment[] {
  const segments: Segment[] = [];
  let buffer: string[] = [];
  const flush = () => {
    if (buffer.length) {
      const joined = buffer.join("\n");
      if (joined.trim()) segments.push({ kind: "text", text: joined });
      buffer = [];
    }
  };
  for (const line of text.split("\n")) {
    if (ACTIVITY_RE.test(line)) {
      flush();
      segments.push({ kind: "tool", text: line.trim() });
    } else {
      buffer.push(line);
    }
  }
  flush();
  return segments;
}

function detectTrigger(
  value: string,
  caret: number
): { kind: "file" | "command"; query: string; start: number } | null {
  const upto = value.slice(0, caret);
  const at = /(^|\s)@([^\s@]*)$/.exec(upto);
  if (at) {
    return { kind: "file", query: at[2], start: caret - at[2].length - 1 };
  }
  const slash = /^\/([^\s]*)$/.exec(upto);
  if (slash) {
    return { kind: "command", query: slash[1], start: 0 };
  }
  return null;
}

const mdComponents = {
  a: ({ href, children }: { href?: string; children?: React.ReactNode }) => (
    <a
      href={href}
      onClick={(e) => {
        e.preventDefault();
        if (href) openUrl(href).catch(() => {});
      }}
    >
      {children}
    </a>
  ),
};

function nowTime(): string {
  return new Date().toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", second: "2-digit" });
}

// Claude's ACP bridge often can't fulfil the AskUserQuestion tool, so the
// model falls back to writing a numbered list of questions in prose. Catch
// that shape so the dialog still works without the structured tool.
// Heuristic: 2+ numbered list items whose body ends with "?", optionally with
// "- bullet" options indented under each one.
function parseMarkdownQuestions(md: string): UserQuestion[] | null {
  const lines = md.split("\n");
  const questions: UserQuestion[] = [];
  let current: UserQuestion | null = null;

  const close = () => {
    if (current) questions.push(current);
    current = null;
  };

  for (const raw of lines) {
    const numbered = raw.match(/^\s*\d+\.\s+(.+)$/);
    if (numbered) {
      close();
      const body = numbered[1].trim();
      // Body may start with **header** that gives us a chip label.
      const boldMatch = body.match(/^\*\*([^*]+?)\*\*\s*(.*)$/);
      const headPart = boldMatch ? boldMatch[1].trim() : body;
      const rest = boldMatch ? boldMatch[2].trim() : "";
      const isQuestion =
        /\?\s*$/.test(headPart) || /\?\s*$/.test(rest) || /\?\s*$/.test(body);
      if (!isQuestion) continue;
      const question = boldMatch
        ? rest
          ? `${headPart} ${rest}`.trim()
          : headPart
        : body;
      const header = headPart.replace(/[?:.,]+$/, "").trim();
      current = {
        question,
        header: header.length <= 36 && header.length > 0 ? header : undefined,
        multi_select: false,
        options: [],
      };
      continue;
    }
    if (current) {
      const bullet = raw.match(/^\s*[-•*○]\s+(.+)$/);
      if (bullet) {
        // Strip trailing parenthetical descriptions so the label stays terse.
        const label = bullet[1].trim();
        current.options.push({ label });
      }
    }
  }
  close();
  return questions.length >= 2 ? questions : null;
}

function basename(path: string): string {
  return path.split(/[/\\]/).filter(Boolean).pop() ?? path;
}

/** kebab-case slug of a workflow name; used as its `/<slug>` slash command. */
function workflowSlug(name: string): string {
  return name
    .toLowerCase()
    .trim()
    .replace(/\s+/g, "-")
    .replace(/[^a-z0-9-]/g, "");
}

/** Match `/workflow-slug [arg…]` at the start of the draft. */
function tryParseWorkflowCommand(
  draft: string,
  workflows: WorkflowSummary[]
): { wf: WorkflowSummary; inputArg: string } | null {
  const m = draft.match(/^\/([\w][\w-]*)(?:\s+([\s\S]*))?$/);
  if (!m) return null;
  const slug = m[1].toLowerCase();
  const wf = workflows.find((w) => workflowSlug(w.name) === slug);
  if (!wf) return null;
  return { wf, inputArg: (m[2] ?? "").trim() };
}

// If a path lives inside the project, render it as a relative reference so the
// agent sees the same shape as @-mentions; otherwise keep the absolute path.
function toAgentRef(absolutePath: string, projectDir: string): string {
  const norm = (p: string) => p.replace(/\\/g, "/").replace(/\/+$/, "");
  const root = norm(projectDir) + "/";
  const abs = norm(absolutePath);
  return abs.startsWith(root) ? abs.slice(root.length) : abs;
}

// Strip the leading status emoji from a tool/activity line so the preview
// in a collapsed group stays compact.
function stripActivityPrefix(line: string): string {
  return line.replace(/^\s*(🔧|💭|📋|⚠)\s*/u, "").trim();
}

/** Collapsible cluster of consecutive activity rows (Claude-Desktop style). */
function ToolGroup({ lines, defaultOpen }: { lines: string[]; defaultOpen: boolean }) {
  const [open, setOpen] = useState(defaultOpen);
  if (lines.length === 0) return null;
  if (lines.length === 1) {
    // A lone activity row is fine as-is — collapsing it adds noise, not signal.
    return <div className="msg__activity">{lines[0]}</div>;
  }
  const preview = stripActivityPrefix(lines[lines.length - 1]);
  return (
    <div className={`tool-group${open ? " is-open" : ""}`}>
      <button
        type="button"
        className="tool-group__head"
        onClick={() => setOpen((o) => !o)}
      >
        <Icon name={open ? "chev-down" : "chev-right"} size={11} />
        <span className="tool-group__count">{lines.length} actions</span>
        {!open && preview && (
          <span className="tool-group__preview" title={preview}>
            {preview}
          </span>
        )}
      </button>
      {open && (
        <div className="tool-group__body">
          {lines.map((l, i) => (
            <div key={i} className="msg__activity">
              {l}
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

function MessageBody({ m }: { m: Message }) {
  if (m.role === "user") {
    return <div className="msg__text">{m.text}</div>;
  }
  const segments = splitSegments(m.text);
  if (segments.length === 0) {
    return m.streaming ? <div className="msg__text">…</div> : null;
  }
  // Cluster consecutive tool rows so a long sequence of file reads / bash
  // calls renders as one collapsible block instead of a wall of pills.
  type Group =
    | { kind: "text"; text: string }
    | { kind: "tools"; lines: string[] };
  const groups: Group[] = [];
  let buf: string[] = [];
  const flushTools = () => {
    if (buf.length) {
      groups.push({ kind: "tools", lines: buf });
      buf = [];
    }
  };
  for (const seg of segments) {
    if (seg.kind === "tool") {
      buf.push(seg.text);
    } else {
      flushTools();
      groups.push({ kind: "text", text: seg.text });
    }
  }
  flushTools();
  return (
    <>
      {groups.map((g, i) =>
        g.kind === "tools" ? (
          // Live turn: keep groups open so the user can watch progress.
          // Once the turn finishes, future re-renders default to collapsed.
          <ToolGroup key={i} lines={g.lines} defaultOpen={!!m.streaming} />
        ) : (
          <div key={i} className="chat-md">
            <ReactMarkdown remarkPlugins={[remarkGfm]} components={mdComponents}>
              {g.text}
            </ReactMarkdown>
          </div>
        )
      )}
    </>
  );
}

export function ChatView({
  projectDir,
  agents,
  workflows,
  convId,
  onSessionsChanged,
}: {
  projectDir: string;
  agents: Availability[];
  workflows: WorkflowSummary[];
  convId?: string;
  onSessionsChanged?: () => void;
}) {
  const available = AGENTS.filter((id) => agents.find((a) => a.id === id)?.available);
  const [agent, setAgent] = useState(available[0] ?? "claude");
  const [model, setModel] = useState("");
  const [mode, setMode] = useState<Mode>("accept_edits");
  const [transport, setTransport] = useState<Transport>("cli");
  const [messages, setMessages] = useState<Message[]>([]);
  // Active ACP permission prompt (mode=Ask). One at a time — the agent waits
  // for our response before issuing another, so we model it as Option<…>.
  const [permPrompt, setPermPrompt] = useState<{
    requestId: string;
    tool: string | null;
    options: PermissionOption[];
  } | null>(null);
  // Active AskUserQuestion dialog (Claude-Desktop style). Either fired by the
  // ACP tool call directly, or auto-inferred from the assistant's markdown
  // when the bridge can't run the tool.
  const [askDialog, setAskDialog] = useState<UserQuestion[] | null>(null);
  // Index of the last assistant message we already evaluated for an inferred
  // dialog, so we don't re-open after the user dismissed it.
  const askedForRef = useRef<number>(-1);
  // ExitPlanMode confirmation — surfaced when Claude finishes its plan and
  // wants to start executing.
  const [exitPlan, setExitPlan] = useState<{ plan: string | null } | null>(null);

  // Workflow execution inside the chat (the user typed /<wfname>). One at a
  // time; the next agent prompt is blocked until done.
  const workflowRunIdRef = useRef<string | null>(null);
  const [approvalReq, setApprovalReq] = useState<{ stepId: string; title: string } | null>(null);
  // Output we'll auto-prepend to the very next agent prompt so the chat agent
  // has context about what the workflow produced. Consumed after one use.
  const [pendingContext, setPendingContext] = useState<string | null>(null);
  const [msgTimes, setMsgTimes] = useState<string[]>([]);
  const [draft, setDraft] = useState("");
  const [attachments, setAttachments] = useState<string[]>([]);
  const [busy, setBusy] = useState(false);
  const [hasSession, setHasSession] = useState(false);
  const [title, setTitle] = useState("");

  const [localCommands, setLocalCommands] = useState<CommandInfo[]>([]);
  const [acpCommands, setAcpCommands] = useState<CommandInfo[]>([]);
  const [fileMatches, setFileMatches] = useState<string[]>([]);
  const [pop, setPop] = useState<{ kind: "file" | "command"; query: string; start: number } | null>(
    null
  );
  const [activeIdx, setActiveIdx] = useState(0);
  const taRef = useRef<HTMLTextAreaElement | null>(null);

  const convIdRef = useRef<string>(convId ?? crypto.randomUUID());
  const sessionRef = useRef<string | null>(null);
  const scrollEnd = useRef<HTMLDivElement | null>(null);
  const loadedRef = useRef(false);

  // Files panel state. `filesNonce` is the refresh signal — bumped when a turn
  // finishes or an inline workflow ends so the panel re-scans the working tree.
  const [filesOpen, setFilesOpen] = useState(false);
  const [filesNonce, setFilesNonce] = useState(0);
  const [filesCount, setFilesCount] = useState(0);

  const applySession = (s: NonNullable<Awaited<ReturnType<typeof api.loadChat>>>) => {
    if (s.agent) setAgent(s.agent);
    setModel(s.model ?? "");
    setMode(normalizeMode(s.mode));
    if (s.transport === "acp" || s.transport === "cli") setTransport(s.transport);
    if (s.conv_id) convIdRef.current = s.conv_id;
    sessionRef.current = s.session_id ?? null;
    setHasSession(!!s.session_id);
    setTitle(s.title ?? "");
    setMessages(
      s.messages.map((m) => ({ role: m.role as Message["role"], text: m.text }))
    );
    setMsgTimes(s.messages.map(() => ""));
  };

  // Restore the requested chat (or most recent for this project) on mount.
  useEffect(() => {
    api
      .loadChat(projectDir, convId)
      .then((s) => {
        if (s) applySession(s);
      })
      .catch(() => {})
      .finally(() => {
        loadedRef.current = true;
      });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [projectDir]);

  useEffect(() => {
    if (available.length && !available.includes(agent)) setAgent(available[0]);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [agents]);

  useEffect(() => {
    api.listSlashCommands(projectDir).then(setLocalCommands).catch(() => {});
  }, [projectDir]);

  // Persist after each turn completes and notify the rail.
  useEffect(() => {
    if (!loadedRef.current || busy) return;
    if (messages.length === 0) return;
    api
      .saveChat(projectDir, {
        session_id: sessionRef.current ?? undefined,
        agent,
        model: model || undefined,
        mode,
        messages: messages.map((m) => ({ role: m.role, text: m.text })),
        conv_id: convIdRef.current,
        transport,
        title,
      })
      .then(() => onSessionsChanged?.())
      .catch(() => {});
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [busy, messages, agent, model, mode, title]);

  const onAgentChange = (next: string) => {
    setAgent(next);
    setModel("");
  };

  useEffect(() => {
    scrollEnd.current?.scrollIntoView({ behavior: "smooth", block: "end" });
  }, [messages]);

  // Whenever the chat finishes a turn (busy goes false), refresh the files
  // panel so newly-touched files show up. Also fires the first time the panel
  // becomes visible — `filesNonce` is its only refresh trigger.
  const wasBusyRef = useRef(false);
  useEffect(() => {
    if (wasBusyRef.current && !busy) {
      setFilesNonce((n) => n + 1);
    }
    wasBusyRef.current = busy;
  }, [busy]);

  // After a turn finishes streaming, look at the final assistant message —
  // if it reads like a structured question list (e.g. when the AskUserQuestion
  // tool isn't available over ACP), surface the same dialog automatically.
  useEffect(() => {
    if (busy) return;
    if (askDialog) return;
    const lastIdx = messages.length - 1;
    if (lastIdx <= askedForRef.current) return;
    const last = messages[lastIdx];
    if (!last || last.role !== "assistant" || last.streaming || last.error) return;
    const parsed = parseMarkdownQuestions(last.text);
    if (parsed) {
      askedForRef.current = lastIdx;
      setAskDialog(parsed);
    } else {
      askedForRef.current = lastIdx;
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [busy, messages.length]);

  const appendToLast = (chunk: string) =>
    setMessages((prev) => {
      const next = [...prev];
      const last = next[next.length - 1];
      if (last?.role === "assistant") {
        next[next.length - 1] = { ...last, text: last.text + chunk };
      }
      return next;
    });

  const finalize = (error?: string) => {
    // Claude CLI reports a stale --resume id as "No conversation found";
    // drop our cached session id so the next send starts a fresh conversation
    // instead of looping on the same broken resume.
    if (error && /no conversation found/i.test(error)) {
      sessionRef.current = null;
      setHasSession(false);
    }
    setMessages((prev) => {
      const next = [...prev];
      const last = next[next.length - 1];
      if (last?.role === "assistant") {
        next[next.length - 1] = {
          ...last,
          streaming: false,
          error: !!error,
          text: error
            ? (last.text ? `${last.text}\n` : "") + error
            : last.text || "(no output)",
        };
      }
      return next;
    });
  };

  // Run a workflow inline in the chat thread. Each LogEvent becomes part of a
  // single accumulating assistant message; the final-step stdout is stashed
  // into pendingContext so the next agent prompt has context to refer to.
  const runWorkflowInline = async (wf: WorkflowSummary, inputArg: string) => {
    const inputs: Record<string, string> = {};
    if (wf.inputs.length > 0) {
      inputs[wf.inputs[0]] = inputArg;
      for (let i = 1; i < wf.inputs.length; i++) inputs[wf.inputs[i]] = "";
    }

    if (!title) setTitle(`/${wf.name}`);
    const t = nowTime();
    setMessages((p) => [
      ...p,
      { role: "user", text: `/${wf.name}${inputArg ? " " + inputArg : ""}` },
      { role: "assistant", text: "", streaming: true, agent: "workflow" },
    ]);
    setMsgTimes((p) => [...p, t, t]);
    setDraft("");
    setPop(null);
    setBusy(true);

    // Per-step stdout buffers and the order of completion so we know which
    // one is "last" when producing the summary.
    const stepText: Record<string, string> = {};
    const stepOrder: string[] = [];

    const result = await new Promise<{ ok: boolean; lastStepText: string }>((resolve) => {
      const channel = new Channel<LogEvent>();
      let lastStep = "";
      channel.onmessage = (ev) => {
        switch (ev.type) {
          case "run_started":
            workflowRunIdRef.current = ev.run_id;
            appendToLast(`▶ workflow ${ev.workflow}\n`);
            break;
          case "step_started":
            lastStep = ev.step_id;
            stepText[ev.step_id] = "";
            if (!stepOrder.includes(ev.step_id)) stepOrder.push(ev.step_id);
            appendToLast(
              `\n🔧 ${ev.title} — ${ev.agent}${ev.model ? ` (${ev.model})` : ""}${
                ev.attempt > 1 ? ` · attempt ${ev.attempt}` : ""
              }\n`
            );
            break;
          case "stdout":
            stepText[ev.step_id] = (stepText[ev.step_id] ?? "") + ev.line + "\n";
            appendToLast(`${ev.line}\n`);
            break;
          case "stderr":
            appendToLast(`⚠ ${ev.line}\n`);
            break;
          case "step_finished":
            appendToLast(
              ev.exit_code === 0
                ? `✓ ${ev.step_id} done\n`
                : `✗ ${ev.step_id} exit ${ev.exit_code}\n`
            );
            break;
          case "step_skipped":
            appendToLast(`⤼ skipped ${ev.step_id}\n`);
            break;
          case "retrying":
            appendToLast(`↻ retry ${ev.step_id} (attempt ${ev.attempt})\n`);
            break;
          case "goto":
            appendToLast(`↪ goto ${ev.from} → ${ev.to}\n`);
            break;
          case "awaiting_approval":
            setApprovalReq({ stepId: ev.step_id, title: ev.title });
            appendToLast(`⏸ awaiting approval: ${ev.title}\n`);
            break;
          case "approved":
            appendToLast(`✓ approved ${ev.step_id}\n`);
            break;
          case "rejected":
            appendToLast(`✕ rejected ${ev.step_id}\n`);
            break;
          case "done":
            appendToLast(`\n✔ workflow done\n`);
            finalize();
            resolve({ ok: true, lastStepText: stepText[lastStep] ?? "" });
            break;
          case "cancelled":
            appendToLast(`\n■ workflow cancelled\n`);
            finalize();
            resolve({ ok: false, lastStepText: stepText[lastStep] ?? "" });
            break;
          case "error":
            appendToLast(`\n✗ workflow error: ${ev.message}\n`);
            finalize(ev.message);
            resolve({ ok: false, lastStepText: stepText[lastStep] ?? "" });
            break;
        }
      };

      api
        .startRun({ workflowPath: wf.path, projectDir, inputs }, channel)
        .then((id) => {
          workflowRunIdRef.current = id;
        })
        .catch((e) => {
          appendToLast(`\n✗ failed to start workflow: ${e}\n`);
          finalize(String(e));
          resolve({ ok: false, lastStepText: "" });
        });
    });

    workflowRunIdRef.current = null;
    setApprovalReq(null);
    setBusy(false);

    if (result.ok && result.lastStepText.trim()) {
      // Stage a context preamble for the next agent prompt. Cap to keep
      // prompts manageable; user can still scroll the full output in chat.
      const snippet = result.lastStepText.trim().slice(0, 2000);
      const summary =
        `Context: workflow \`/${wf.name}\` just ran in this session.\n` +
        `Final-step output (truncated to 2 KB):\n\n` +
        "```\n" +
        snippet +
        "\n```";
      setPendingContext(summary);
    }
  };

  const decideApproval = (decision: "approve" | "reject") => {
    if (workflowRunIdRef.current) {
      api.approve(workflowRunIdRef.current, decision).catch(() => {});
    }
    setApprovalReq(null);
  };

  const cancelWorkflow = () => {
    if (workflowRunIdRef.current) {
      api.cancel(workflowRunIdRef.current).catch(() => {});
    }
  };

  const pickFiles = async () => {
    if (busy) return;
    try {
      const picked = await open({
        multiple: true,
        directory: false,
        title: "Attach file(s) to message",
        defaultPath: projectDir,
      });
      if (!picked) return;
      const list = Array.isArray(picked) ? picked : [picked];
      const refs = list.map((p) => toAgentRef(String(p), projectDir));
      setAttachments((prev) => Array.from(new Set([...prev, ...refs])));
    } catch {
      // user cancelled or dialog failed; nothing to surface
    }
  };

  const removeAttachment = (path: string) =>
    setAttachments((prev) => prev.filter((p) => p !== path));

  const send = async () => {
    const draftText = draft.trim();
    if ((!draftText && attachments.length === 0) || busy) return;

    // /<workflow> intercept: run inline instead of talking to the agent.
    const wfCmd = tryParseWorkflowCommand(draftText, workflows);
    if (wfCmd) {
      await runWorkflowInline(wfCmd.wf, wfCmd.inputArg);
      return;
    }

    if (!available.length) return;
    // Build the prompt the agent sees: optional workflow-context preamble
    // (consumed once), attached file refs, then the user's text.
    const ctxBlock = pendingContext ? `${pendingContext}\n\n` : "";
    const attachBlock =
      attachments.length > 0
        ? `Attached files:\n${attachments.map((p) => `- @${p}`).join("\n")}\n\n`
        : "";
    const prompt = ctxBlock + attachBlock + draftText;
    // Display message keeps attachments visible as @-refs even when the user
    // sent no prose — gives a record of what was shared.
    const displayText =
      attachments.length > 0 && !draftText
        ? attachments.map((p) => `@${p}`).join(" ")
        : draftText;

    setDraft("");
    setAttachments([]);
    setPop(null);
    setBusy(true);
    if (!title) {
      const firstLine = (displayText || prompt).split("\n").find((l) => l.trim());
      setTitle((firstLine ?? "(attachments)").slice(0, 60));
    }
    const t = nowTime();
    setMessages((p) => [
      ...p,
      { role: "user", text: displayText || prompt },
      { role: "assistant", text: "", streaming: true, agent },
    ]);
    setMsgTimes((p) => [...p, t, t]);

    const tx = transport;
    // If Claude's stderr tells us the stored --resume id is stale, we'll
    // restart this exact turn once without --resume so the user doesn't have
    // to hit Send again.
    let staleResume = false;
    const channel = new Channel<LogEvent>();
    channel.onmessage = (ev) => {
      if (ev.type === "stdout" || ev.type === "stderr") {
        appendToLast(tx === "acp" ? ev.line : `${ev.line}\n`);
        if (ev.type === "stderr" && /no conversation found/i.test(ev.line)) {
          staleResume = true;
          sessionRef.current = null;
          setHasSession(false);
        }
      } else if (ev.type === "session_id") {
        sessionRef.current = ev.session_id;
        setHasSession(true);
      } else if (ev.type === "available_commands") {
        setAcpCommands(ev.commands);
      } else if (ev.type === "permission_request") {
        setPermPrompt({
          requestId: ev.request_id,
          tool: ev.tool,
          options: ev.options,
        });
      } else if (ev.type === "ask_user_question") {
        setAskDialog(ev.questions);
      } else if (ev.type === "exit_plan_mode") {
        // Multiple plan-mode tool failures can fire this within one turn —
        // keep the first dialog up; ignore later duplicates until dismissed.
        setExitPlan((prev) => prev ?? { plan: ev.plan });
      }
    };

    let primaryError: unknown = null;
    try {
      await api.startChat(
        {
          chatId: convIdRef.current,
          agent,
          prompt,
          mode,
          model: model || undefined,
          projectDir,
          resume: sessionRef.current ?? undefined,
          transport: tx,
        },
        channel
      );
    } catch (e) {
      primaryError = e;
    }

    // Auto-recover from a stale --resume id: replace the failed assistant
    // turn with a fresh attempt that doesn't pass --resume.
    if (staleResume && tx === "cli") {
      setMessages((prev) => {
        const next = [...prev];
        if (next[next.length - 1]?.role === "assistant") {
          next[next.length - 1] = { role: "assistant", text: "", streaming: true, agent };
        }
        return next;
      });
      const retryChannel = new Channel<LogEvent>();
      retryChannel.onmessage = (ev) => {
        if (ev.type === "stdout" || ev.type === "stderr") {
          appendToLast(`${ev.line}\n`);
        } else if (ev.type === "session_id") {
          sessionRef.current = ev.session_id;
          setHasSession(true);
        } else if (ev.type === "available_commands") {
          setAcpCommands(ev.commands);
        }
      };
      try {
        await api.startChat(
          {
            chatId: convIdRef.current,
            agent,
            prompt,
            mode,
            model: model || undefined,
            projectDir,
            resume: undefined,
            transport: tx,
          },
          retryChannel
        );
        finalize();
      } catch (e) {
        finalize(String(e));
      }
    } else if (primaryError) {
      finalize(String(primaryError));
    } else {
      finalize();
    }

    setBusy(false);
    setPermPrompt(null);
    // The context preamble travelled with this turn — drop it so it doesn't
    // re-cling to subsequent prompts.
    setPendingContext(null);
  };

  const respondPerm = (optionId: string | null) => {
    if (!permPrompt) return;
    api
      .respondPermission(convIdRef.current, permPrompt.requestId, optionId)
      .catch(() => {});
    setPermPrompt(null);
  };

  // ExitPlanMode confirmation handlers — switch the conversation's mode then
  // stage a follow-up so the user can review and hit Send.
  const approveExitPlan = (nextMode: Mode) => {
    setExitPlan(null);
    setMode(nextMode);
    setDraft((prev) =>
      prev.trim()
        ? prev
        : "Plan approved — please proceed with the implementation."
    );
    requestAnimationFrame(() => taRef.current?.focus());
  };

  const keepPlanning = () => {
    setExitPlan(null);
    setDraft((prev) =>
      prev.trim() ? prev : "Let's revise the plan first — "
    );
    requestAnimationFrame(() => taRef.current?.focus());
  };

  // Format the answers from the AskUserQuestion dialog into a follow-up
  // message the agent can read, drop into the composer, and focus the textarea
  // so the user can review/edit and send.
  const submitAskAnswers = (answers: string[]) => {
    if (!askDialog) return;
    const lines: string[] = [];
    askDialog.forEach((q, i) => {
      const a = answers[i]?.trim();
      if (!a) return;
      const headerOrIndex = q.header ? `${q.header}` : `Q${i + 1}`;
      lines.push(`- **${headerOrIndex}** — ${q.question}\n  → ${a}`);
    });
    const body = lines.length > 0 ? `Here are my answers:\n\n${lines.join("\n\n")}` : "";
    setAskDialog(null);
    if (!body) return;
    setDraft((prev) => (prev.trim() ? `${prev.trim()}\n\n${body}` : body));
    requestAnimationFrame(() => {
      taRef.current?.focus();
      const ta = taRef.current;
      if (ta) ta.setSelectionRange(ta.value.length, ta.value.length);
    });
  };

  const cancel = () => {
    api.cancelChat(convIdRef.current).catch(() => {});
  };

  useEffect(() => {
    if (pop?.kind !== "file") return;
    const q = pop.query;
    const handle = setTimeout(() => {
      api
        .listProjectFiles(projectDir, q)
        .then(setFileMatches)
        .catch(() => setFileMatches([]));
    }, 120);
    return () => clearTimeout(handle);
  }, [pop, projectDir]);

  const mergedCommands = (() => {
    const workflowEntries: CommandInfo[] = workflows.map((w) => ({
      name: workflowSlug(w.name),
      description:
        (w.name !== workflowSlug(w.name) ? `${w.name} · ` : "") +
        (w.inputs.length > 0
          ? `workflow · inputs: ${w.inputs.join(", ")}`
          : "workflow"),
      kind: "workflow",
    }));
    const seen = new Set<string>();
    return [...workflowEntries, ...acpCommands, ...localCommands].filter((c) =>
      seen.has(c.name) ? false : (seen.add(c.name), true)
    );
  })();

  const popItems: { value: string; label: string; hint: string; tag?: string }[] = !pop
    ? []
    : pop.kind === "file"
    ? fileMatches.map((f) => ({ value: f, label: f, hint: "" }))
    : mergedCommands
        .filter((c) => c.name.toLowerCase().includes(pop.query.toLowerCase()))
        .map((c) => ({
          value: c.name,
          label: `/${c.name}`,
          hint: c.description ?? "",
          tag: c.kind,
        }));

  useEffect(() => {
    setActiveIdx(0);
  }, [pop?.kind, pop?.query, fileMatches.length, mergedCommands.length]);

  const onDraftChange = (e: React.ChangeEvent<HTMLTextAreaElement>) => {
    setDraft(e.target.value);
    setPop(detectTrigger(e.target.value, e.target.selectionStart ?? e.target.value.length));
  };

  const applySelection = (value: string) => {
    if (!pop) return;
    const caret = taRef.current?.selectionStart ?? draft.length;
    const before = draft.slice(0, pop.start);
    const after = draft.slice(caret);
    const insert = pop.kind === "file" ? `@${value} ` : `/${value} `;
    const next = before + insert + after;
    const newCaret = (before + insert).length;
    setDraft(next);
    setPop(null);
    requestAnimationFrame(() => {
      const ta = taRef.current;
      if (ta) {
        ta.focus();
        ta.setSelectionRange(newCaret, newCaret);
      }
    });
  };

  const onKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    if (pop && popItems.length > 0) {
      if (e.key === "ArrowDown") {
        e.preventDefault();
        setActiveIdx((i) => (i + 1) % popItems.length);
        return;
      }
      if (e.key === "ArrowUp") {
        e.preventDefault();
        setActiveIdx((i) => (i - 1 + popItems.length) % popItems.length);
        return;
      }
      if (e.key === "Enter" || e.key === "Tab") {
        e.preventDefault();
        applySelection(popItems[activeIdx].value);
        return;
      }
      if (e.key === "Escape") {
        e.preventDefault();
        setPop(null);
        return;
      }
    }
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      send();
    }
  };

  return (
    <div className="main">
      <div className="main__head">
        <div className="main__title">
          <span className={`agent__dot is-${busy ? "busy" : hasSession ? "ok" : "off"}`} />
          <span className="main__title-text" title={title || undefined}>
            {title || "New session"}
          </span>
        </div>
        <div className="main__head-right">
          {hasSession && (
            <span className="tag is-ok">
              <span className="tag__dot" />
              session
            </span>
          )}
          {busy && (
            <span className="tag is-run">
              <span className="tag__dot" />
              streaming
            </span>
          )}
          <select
            className="field-select"
            value={transport}
            disabled={busy}
            title="CLI (one-shot) or ACP (Agent Client Protocol)"
            onChange={(e) => setTransport(e.target.value as Transport)}
          >
            <option value="cli">CLI</option>
            <option value="acp">ACP</option>
          </select>
          <select
            className="field-select"
            value={agent}
            disabled={busy || available.length === 0}
            onChange={(e) => onAgentChange(e.target.value)}
          >
            {(available.length ? available : AGENTS).map((id) => (
              <option key={id} value={id}>
                {id}
              </option>
            ))}
          </select>
          <select
            className="field-select"
            value={model}
            disabled={busy || transport === "acp"}
            title={
              transport === "acp" ? "model selection not supported over ACP yet" : "model"
            }
            onChange={(e) => setModel(e.target.value)}
          >
            <option value="">Default model</option>
            {(MODELS[agent] ?? []).map((m) => (
              <option key={m.value} value={m.value}>
                {m.label}
              </option>
            ))}
          </select>
          <select
            className="field-select"
            value={mode}
            disabled={busy}
            title={
              mode === "ask" && transport === "cli"
                ? "Ask is interactive — works best with the ACP transport. CLI falls back to default permissions."
                : "Permission mode"
            }
            onChange={(e) => setMode(e.target.value as Mode)}
          >
            {MODES.map((m) => (
              <option key={m} value={m}>
                {MODE_LABELS[m]}
              </option>
            ))}
          </select>
          <button
            className={`composer__pill${filesOpen ? " is-active" : ""}`}
            onClick={() => setFilesOpen((v) => !v)}
            title={filesOpen ? "Hide files panel" : "Show changed files"}
          >
            <Icon name="folder" size={11} /> Files
            {filesCount > 0 && <span className="files-toggle__badge">{filesCount}</span>}
          </button>
        </div>
      </div>

      <div className={`chat-body${filesOpen ? " has-files" : ""}`}>
        <div className="scroll">
        {messages.length === 0 && (
          <div className="empty">
            <div className="empty__inner">
              <div className="empty__hello">arthur · session · new</div>
              <h1 className="empty__h1">What are we building?</h1>
              <div className="empty__sub">
                Describe a task, paste an error, drop a file. Arthur will route it through{" "}
                <code>{agent}</code>.
              </div>
            </div>
          </div>
        )}
        {messages.map((m, i) => (
          <div key={i} className={`msg ${m.role === "user" ? "msg--user" : ""} ${m.error ? "msg--error" : ""}`}>
            <div className="msg__gutter">
              <span className="who">{m.role === "user" ? "you" : m.agent ?? "assistant"}</span>
              <span className="time">
                {msgTimes[i] || ""}
                {m.streaming ? " · working…" : ""}
              </span>
            </div>
            <div className="msg__body">
              <MessageBody m={m} />
            </div>
          </div>
        ))}
        <div ref={scrollEnd} />
        </div>
        <FilesPanel
          sessionKey={convIdRef.current}
          projectDir={projectDir}
          refreshNonce={filesNonce}
          open={filesOpen}
          onClose={() => setFilesOpen(false)}
          onCountChange={setFilesCount}
        />
      </div>

      <div className="composer">
        <div className="composer__box">
          {pop && popItems.length > 0 && (
            <div className="composer__pop">
              <div className="composer__pop-head">
                {pop.kind === "file" ? "files" : "commands"}
              </div>
              {popItems.slice(0, 50).map((it, i) => (
                <div
                  key={it.value}
                  className={`composer__pop-item${i === activeIdx ? " active" : ""}`}
                  onMouseDown={(e) => {
                    e.preventDefault();
                    applySelection(it.value);
                  }}
                >
                  <span className="composer__pop-label">{it.label}</span>
                  {it.tag && <span className={`composer__pop-tag ${it.tag}`}>{it.tag}</span>}
                  {it.hint && <span className="composer__pop-hint">{it.hint}</span>}
                </div>
              ))}
            </div>
          )}
          {pop && pop.kind === "command" && popItems.length === 0 && (
            <div className="composer__pop">
              <div className="composer__pop-empty">
                No matching commands. Define them in <code>.claude/commands</code> or{" "}
                <code>.claude/skills</code>
                {transport === "acp" ? "; agent commands appear after it connects." : "."}
              </div>
            </div>
          )}
          {pendingContext && (
            <div className="composer__ctx" title={pendingContext}>
              <Icon name="play-step" size={10} />
              <span className="composer__ctx-label">
                Workflow output will be added to your next message
              </span>
              <button
                className="x"
                title="Discard"
                onClick={() => setPendingContext(null)}
                disabled={busy}
              >
                ×
              </button>
            </div>
          )}
          {attachments.length > 0 && (
            <div className="composer__chips">
              {attachments.map((p) => (
                <span key={p} className="composer__chip" title={p}>
                  <Icon name="paperclip" size={10} />
                  <span className="name">{basename(p)}</span>
                  <button
                    className="x"
                    title="Remove attachment"
                    onClick={() => removeAttachment(p)}
                    disabled={busy}
                  >
                    ×
                  </button>
                </span>
              ))}
            </div>
          )}
          <textarea
            ref={taRef}
            className="composer__input"
            value={draft}
            rows={3}
            placeholder={
              available.length
                ? `Message ${agent}…  (@ file · / command · Enter to send · Shift+Enter newline)`
                : "No CLIs detected on PATH"
            }
            disabled={busy || available.length === 0}
            onChange={onDraftChange}
            onKeyDown={onKeyDown}
          />
          <div className="composer__bar">
            <span className="abadge abadge--pill" data-agent={agent}>
              {agent}
            </span>
            <button
              className="composer__pill"
              onClick={pickFiles}
              disabled={busy}
              title="Attach files"
            >
              <Icon name="paperclip" size={11} /> attach
            </button>
            <span className="composer__spacer" />
            {busy ? (
              <button
                className="composer__cancel"
                onClick={workflowRunIdRef.current ? cancelWorkflow : cancel}
              >
                <Icon name="x" size={11} />{" "}
                {workflowRunIdRef.current ? "Cancel workflow" : "Cancel"}
              </button>
            ) : (
              <button
                className="composer__send"
                disabled={(!draft.trim() && attachments.length === 0) || available.length === 0}
                onClick={send}
              >
                <Icon name="send" size={11} /> Send <span className="kbd">⌘↵</span>
              </button>
            )}
          </div>
        </div>
      </div>

      {askDialog && (
        <AskUserDialog
          questions={askDialog}
          onSubmit={submitAskAnswers}
          onCancel={() => setAskDialog(null)}
        />
      )}

      {exitPlan && (
        <ExitPlanDialog
          plan={exitPlan.plan}
          onApprove={approveExitPlan}
          onKeepPlanning={keepPlanning}
          onClose={() => setExitPlan(null)}
        />
      )}

      {approvalReq && (
        <div className="modal-backdrop">
          <div className="modal">
            <h3>Approval required</h3>
            <p>
              Workflow step <strong>{approvalReq.title}</strong> is waiting. Approve to
              continue or reject to stop the run.
            </p>
            <div className="modal-actions">
              <button className="btn-danger" onClick={() => decideApproval("reject")}>
                Reject
              </button>
              <button className="btn-primary" onClick={() => decideApproval("approve")}>
                Approve
              </button>
            </div>
          </div>
        </div>
      )}

      {permPrompt && (
        <div className="modal-backdrop" onClick={() => respondPerm(null)}>
          <div className="modal perm-modal" onClick={(e) => e.stopPropagation()}>
            <h3>Permission requested</h3>
            <p>
              {permPrompt.tool ? (
                <>
                  Agent wants to: <strong>{permPrompt.tool}</strong>
                </>
              ) : (
                <>The agent is asking for permission to proceed.</>
              )}
            </p>
            <div className="perm-options">
              {permPrompt.options.map((o) => {
                const isAllow = o.kind?.startsWith("allow") ?? false;
                return (
                  <button
                    key={o.id}
                    className={isAllow ? "btn-primary" : "btn-ghost"}
                    onClick={() => respondPerm(o.id)}
                  >
                    {o.label}
                  </button>
                );
              })}
            </div>
            <div className="modal-actions">
              <button className="btn-ghost" onClick={() => respondPerm(null)}>
                Cancel
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
