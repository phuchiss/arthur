import { invoke, Channel } from "@tauri-apps/api/core";

export type Availability = {
  id: string;
  available: boolean;
  version: string | null;
  path: string | null;
};

export type Retry = { max: number; until: string };

/** Permission mode (mirrors Rust `Mode`, serde snake_case). */
export type Mode = "ask" | "accept_edits" | "plan" | "auto";

export const MODE_LABELS: Record<Mode, string> = {
  ask: "Ask permissions",
  accept_edits: "Accept edits",
  plan: "Plan mode",
  auto: "Auto mode",
};

export type StepConfig = {
  agent?: string;
  model?: string;
  mode?: Mode;
  output?: string;
  approval?: boolean;
  when?: string;
  goto?: string;
  retry?: Retry;
};

export type Step = { id: string; title: string; config: StepConfig; prompt: string };
export type Defaults = { agent?: string; model?: string; mode?: Mode };
export type Workflow = {
  name: string;
  inputs: string[];
  defaults: Defaults;
  steps: Step[];
  path?: string;
};
export type WorkflowSummary = {
  name: string;
  path: string;
  inputs: string[];
  source: "project" | "global";
};

export type PermissionOption = {
  id: string;
  label: string;
  /** "allow_once" | "allow_always" | "reject_once" | "reject_always" (or absent). */
  kind?: string;
};

export type UserQuestionOption = {
  label: string;
  description?: string;
};

export type UserQuestion = {
  question: string;
  header?: string;
  multi_select: boolean;
  options: UserQuestionOption[];
};

/** Mirrors the Rust `LogEvent` enum (serde tag = "type", snake_case). */
export type LogEvent =
  | { type: "run_started"; run_id: string; workflow: string }
  | { type: "step_started"; step_id: string; title: string; agent: string; model: string | null; attempt: number }
  | { type: "stdout"; step_id: string; line: string }
  | { type: "session_id"; step_id: string; session_id: string }
  | {
      type: "token_usage";
      step_id: string;
      input_tokens: number;
      output_tokens: number;
      cache_creation_input_tokens: number;
      cache_read_input_tokens: number;
      cost_usd?: number;
    }
  | { type: "available_commands"; step_id: string; commands: CommandInfo[] }
  | { type: "stderr"; step_id: string; line: string }
  | { type: "step_finished"; step_id: string; exit_code: number; attempt: number; final_text: string }
  | { type: "step_skipped"; step_id: string }
  | { type: "retrying"; step_id: string; attempt: number }
  | { type: "goto"; from: string; to: string }
  | { type: "awaiting_approval"; step_id: string; title: string }
  | { type: "approved"; step_id: string }
  | { type: "rejected"; step_id: string }
  | {
      type: "permission_request";
      step_id: string;
      request_id: string;
      tool: string | null;
      options: PermissionOption[];
    }
  | {
      type: "ask_user_question";
      step_id: string;
      questions: UserQuestion[];
    }
  | {
      type: "exit_plan_mode";
      step_id: string;
      plan: string | null;
    }
  | { type: "cancelled" }
  | { type: "done" }
  | { type: "error"; message: string };

export type CommandInfo = {
  name: string;
  description: string | null;
  /** "command" | "skill" for local entries; absent for ACP-supplied items. */
  kind?: string;
};

/** Mirrors the Rust `chatstore::ChatSession` (serde snake_case). */
export type ChatMessage = { role: "user" | "assistant"; text: string };
export type TokenTotals = {
  input_tokens: number;
  output_tokens: number;
  cache_creation_input_tokens: number;
  cache_read_input_tokens: number;
  cost_usd: number;
  turns: number;
};
export type ChatSession = {
  session_id?: string;
  agent: string;
  model?: string;
  /** New name for the old `autonomy` field. Rust reads either via serde alias. */
  mode: string;
  messages: ChatMessage[];
  conv_id?: string;
  transport?: "cli" | "acp";
  title?: string;
  /** Staged workflow-result preamble awaiting the next send; persisted so it
   *  survives reload. Mirrors Rust `pending_context`. */
  pending_context?: string | null;
  updated_at?: number;
  created_at?: number;
  /** Cumulative token usage; absent on legacy saves and on agents that don't
   *  report usage (currently only claude stream-json does). */
  tokens?: TokenTotals | null;
};

export type ChatSummary = {
  conv_id: string;
  title: string;
  agent: string;
  transport?: "cli" | "acp" | string;
  updated_at: number;
  message_count: number;
};

/** Mirrors the Rust `files::FileStatus` (serde snake_case). */
export type FileStatus =
  | "modified"
  | "added"
  | "deleted"
  | "renamed"
  | "untracked"
  | "unchanged";
export type ChangedFile = { path: string; status: FileStatus };
export type ChangedFilesResult = {
  files: ChangedFile[];
  git_available: boolean;
  truncated: boolean;
};
export type FilePreview = {
  content: string;
  truncated: boolean;
  binary: boolean;
  /** `data:<mime>;base64,…` URL for inlined image files; null otherwise. */
  image: string | null;
};

/** Mirrors the Rust `projectstore::RecentProject` (serde snake_case). */
export type RecentProject = {
  path: string;
  last_opened_at: number;
};

export { Channel };

export const api = {
  checkAgents: () => invoke<Availability[]>("check_agents"),
  listWorkflows: (projectDir: string) =>
    invoke<WorkflowSummary[]>("list_workflows", { projectDir }),
  getWorkflow: (path: string) => invoke<Workflow>("get_workflow", { path }),
  readWorkflowSource: (path: string) => invoke<string>("read_workflow_source", { path }),
  parseWorkflowSource: (content: string, path?: string) =>
    invoke<Workflow>("parse_workflow_source", { content, path }),
  saveWorkflow: (path: string, content: string) =>
    invoke<void>("save_workflow", { path, content }),
  createWorkflow: (args: {
    projectDir: string;
    scope: "project" | "global";
    fileName: string;
    content: string;
  }) => invoke<string>("create_workflow", args),
  improveWorkflow: (args: {
    improveId: string;
    agent: string;
    content: string;
    instruction?: string;
    model?: string;
    projectDir?: string;
  }) => invoke<string>("improve_workflow", args),
  cancelImprove: (improveId: string) => invoke<void>("cancel_improve", { improveId }),
  startChat: (
    args: {
      chatId: string;
      agent: string;
      prompt: string;
      mode: Mode;
      model?: string;
      projectDir: string;
      resume?: string;
      transport?: "cli" | "acp";
    },
    onLog: Channel<LogEvent>
  ) => invoke<void>("start_chat", { ...args, onLog }),
  cancelChat: (chatId: string) => invoke<void>("cancel_chat", { chatId }),
  closeChat: (chatId: string) => invoke<void>("close_chat", { chatId }),
  respondPermission: (chatId: string, requestId: string, optionId: string | null) =>
    invoke<void>("respond_permission", { chatId, requestId, optionId }),
  listProjectFiles: (projectDir: string, query: string) =>
    invoke<string[]>("list_project_files", { projectDir, query }),
  listSlashCommands: (projectDir: string) =>
    invoke<CommandInfo[]>("list_slash_commands", { projectDir }),
  listChats: (projectDir: string) =>
    invoke<ChatSummary[]>("list_chats", { projectDir }),
  loadChat: (projectDir: string, convId?: string) =>
    invoke<ChatSession | null>("load_chat", { projectDir, convId }),
  saveChat: (projectDir: string, session: ChatSession) =>
    invoke<void>("save_chat", { projectDir, session }),
  deleteChat: (projectDir: string, convId: string) =>
    invoke<void>("delete_chat", { projectDir, convId }),
  startRun: (
    args: { workflowPath: string; projectDir: string; inputs: Record<string, string> },
    onLog: Channel<LogEvent>
  ) => invoke<string>("start_run", { ...args, onLog }),
  approve: (runId: string, decision: "approve" | "reject") =>
    invoke<void>("approve", { runId, decision }),
  cancel: (runId: string) => invoke<void>("cancel", { runId }),
  listChangedFiles: (sessionKey: string, projectDir: string) =>
    invoke<ChangedFilesResult>("list_changed_files", { sessionKey, projectDir }),
  listAllFiles: (sessionKey: string, projectDir: string) =>
    invoke<ChangedFilesResult>("list_all_files", { sessionKey, projectDir }),
  readFilePreview: (projectDir: string, relPath: string) =>
    invoke<FilePreview>("read_file_preview", { projectDir, relPath }),
  diffFile: (sessionKey: string, projectDir: string, relPath: string) =>
    invoke<string>("diff_file", { sessionKey, projectDir, relPath }),
  resetFilesBaseline: (sessionKey: string, projectDir: string) =>
    invoke<string>("reset_files_baseline", { sessionKey, projectDir }),
  listRecentProjects: () => invoke<RecentProject[]>("list_recent_projects"),
  addRecentProject: (path: string) => invoke<void>("add_recent_project", { path }),
  removeRecentProject: (path: string) =>
    invoke<void>("remove_recent_project", { path }),
  clearRecentProjects: () => invoke<void>("clear_recent_projects"),
};
