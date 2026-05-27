import { useEffect, useRef, useState } from "react";
import { api, Availability, Channel, LogEvent } from "../lib/ipc";

const AGENTS = ["claude", "codex", "gemini"];
const AUTONOMY = ["read", "edit", "full"] as const;
type Autonomy = (typeof AUTONOMY)[number];

// Selectable models per agent (passed to the CLI's --model flag). The empty
// value means "don't pass --model" — let the CLI use its own default.
const MODELS: Record<string, { label: string; value: string }[]> = {
  claude: [
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

export function ChatView({
  projectDir,
  agents,
  onBack,
}: {
  projectDir: string;
  agents: Availability[];
  onBack: () => void;
}) {
  const available = AGENTS.filter((id) => agents.find((a) => a.id === id)?.available);
  const [agent, setAgent] = useState(available[0] ?? "claude");
  const [model, setModel] = useState("");
  const [autonomy, setAutonomy] = useState<Autonomy>("edit");
  const [messages, setMessages] = useState<Message[]>([]);
  const [draft, setDraft] = useState("");
  const [busy, setBusy] = useState(false);
  const [hasSession, setHasSession] = useState(false);

  const chatIdRef = useRef<string | null>(null);
  // Agent session id (claude), captured from the stream so the next turn can
  // --resume it and keep conversation context.
  const sessionRef = useRef<string | null>(null);
  const scrollEnd = useRef<HTMLDivElement | null>(null);
  // Gate persistence until the initial restore has run, so we never overwrite a
  // saved chat with the empty mount state.
  const loadedRef = useRef(false);

  // Restore this project's saved chat once on mount.
  useEffect(() => {
    api
      .loadChat(projectDir)
      .then((s) => {
        if (s) {
          if (s.agent) setAgent(s.agent);
          setModel(s.model ?? "");
          if (s.autonomy) setAutonomy(s.autonomy as Autonomy);
          sessionRef.current = s.session_id ?? null;
          setHasSession(!!s.session_id);
          setMessages(
            s.messages.map((m) => ({ role: m.role as Message["role"], text: m.text }))
          );
        }
      })
      .catch(() => {})
      .finally(() => {
        loadedRef.current = true;
      });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [projectDir]);

  // Keep the selected agent valid if availability resolves after mount.
  useEffect(() => {
    if (available.length && !available.includes(agent)) setAgent(available[0]);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [agents]);

  // Persist after each turn completes (busy → false) and when preferences
  // change, but never mid-stream or before the initial restore.
  useEffect(() => {
    if (!loadedRef.current || busy) return;
    api
      .saveChat(projectDir, {
        session_id: sessionRef.current ?? undefined,
        agent,
        model: model || undefined,
        autonomy,
        messages: messages.map((m) => ({ role: m.role, text: m.text })),
      })
      .catch(() => {});
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [busy, messages, agent, model, autonomy]);

  // Switching agent starts a fresh session (session id + model list are
  // agent-specific); keep the visible history.
  const onAgentChange = (next: string) => {
    setAgent(next);
    setModel("");
    sessionRef.current = null;
    setHasSession(false);
  };

  const newSession = () => {
    sessionRef.current = null;
    setHasSession(false);
    setMessages([]);
  };

  useEffect(() => {
    scrollEnd.current?.scrollIntoView({ behavior: "smooth", block: "end" });
  }, [messages]);

  // Append streamed output onto the in-flight assistant message.
  const appendToLast = (chunk: string) =>
    setMessages((prev) => {
      const next = [...prev];
      const last = next[next.length - 1];
      if (last?.role === "assistant") {
        next[next.length - 1] = { ...last, text: last.text + chunk };
      }
      return next;
    });

  const finalize = (error?: string) =>
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

  const send = async () => {
    const prompt = draft.trim();
    if (!prompt || busy || !available.length) return;
    setDraft("");
    setBusy(true);
    setMessages((p) => [
      ...p,
      { role: "user", text: prompt },
      { role: "assistant", text: "", streaming: true, agent },
    ]);

    const id = crypto.randomUUID();
    chatIdRef.current = id;

    const channel = new Channel<LogEvent>();
    channel.onmessage = (ev) => {
      if (ev.type === "stdout" || ev.type === "stderr") appendToLast(`${ev.line}\n`);
      else if (ev.type === "session_id") {
        sessionRef.current = ev.session_id;
        setHasSession(true);
      }
    };

    try {
      await api.startChat(
        {
          chatId: id,
          agent,
          prompt,
          autonomy,
          model: model || undefined,
          projectDir,
          resume: sessionRef.current ?? undefined,
        },
        channel
      );
      finalize();
    } catch (e) {
      finalize(String(e));
    } finally {
      setBusy(false);
      chatIdRef.current = null;
    }
  };

  const cancel = () => {
    if (chatIdRef.current) api.cancelChat(chatIdRef.current).catch(() => {});
  };

  const onKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    // Enter sends; Shift+Enter inserts a newline.
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      send();
    }
  };

  return (
    <div className="chat">
      <div className="chat-head">
        <button className="link" onClick={onBack}>
          ← back
        </button>
        <span className="chat-title">Chat</span>
        {hasSession && <span className="chat-session" title="conversation context is being kept">● session</span>}
        <button className="link" disabled={busy || messages.length === 0} onClick={newSession}>
          new session
        </button>
        <select
          className="editor-scope"
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
          className="editor-scope"
          value={model}
          disabled={busy}
          title="model"
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
          className="editor-scope"
          value={autonomy}
          disabled={busy}
          onChange={(e) => setAutonomy(e.target.value as Autonomy)}
        >
          {AUTONOMY.map((a) => (
            <option key={a} value={a}>
              {a}
            </option>
          ))}
        </select>
      </div>

      <div className="chat-thread">
        {messages.length === 0 && (
          <p className="hint">
            Ask <code>{agent}</code> to do something in this project. Output streams in live below.
          </p>
        )}
        {messages.map((m, i) => (
          <div key={i} className={`chat-msg ${m.role}${m.error ? " error" : ""}`}>
            <div className="chat-role">
              {m.role === "user" ? "you" : m.agent ?? "assistant"}
              {m.streaming && <span className="chat-working"> · working…</span>}
            </div>
            <div className="chat-text">{m.text || (m.streaming ? "…" : "")}</div>
          </div>
        ))}
        <div ref={scrollEnd} />
      </div>

      <div className="chat-input">
        <textarea
          value={draft}
          rows={3}
          placeholder={
            available.length ? `Message ${agent}…  (Enter to send, Shift+Enter for newline)` : "No CLIs detected on PATH"
          }
          disabled={busy || available.length === 0}
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={onKeyDown}
        />
        {busy ? (
          <button className="danger" onClick={cancel}>
            Cancel
          </button>
        ) : (
          <button
            className="primary"
            disabled={!draft.trim() || available.length === 0}
            onClick={send}
          >
            Send
          </button>
        )}
      </div>
    </div>
  );
}
