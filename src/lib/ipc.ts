import { invoke, Channel } from "@tauri-apps/api/core";

export type Availability = {
  id: string;
  available: boolean;
  version: string | null;
  path: string | null;
};

export type Retry = { max: number; until: string };

export type StepConfig = {
  agent?: string;
  model?: string;
  autonomy?: "read" | "edit" | "full";
  output?: string;
  approval?: boolean;
  when?: string;
  goto?: string;
  retry?: Retry;
};

export type Step = { id: string; title: string; config: StepConfig; prompt: string };
export type Defaults = { agent?: string; model?: string; autonomy?: string };
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

/** Mirrors the Rust `LogEvent` enum (serde tag = "type", snake_case). */
export type LogEvent =
  | { type: "run_started"; run_id: string; workflow: string }
  | { type: "step_started"; step_id: string; title: string; agent: string; model: string | null; attempt: number }
  | { type: "stdout"; step_id: string; line: string }
  | { type: "session_id"; step_id: string; session_id: string }
  | { type: "stderr"; step_id: string; line: string }
  | { type: "step_finished"; step_id: string; exit_code: number; attempt: number }
  | { type: "step_skipped"; step_id: string }
  | { type: "retrying"; step_id: string; attempt: number }
  | { type: "goto"; from: string; to: string }
  | { type: "awaiting_approval"; step_id: string; title: string }
  | { type: "approved"; step_id: string }
  | { type: "rejected"; step_id: string }
  | { type: "cancelled" }
  | { type: "done" }
  | { type: "error"; message: string };

/** Mirrors the Rust `chatstore::ChatSession` (serde snake_case). */
export type ChatMessage = { role: "user" | "assistant"; text: string };
export type ChatSession = {
  session_id?: string;
  agent: string;
  model?: string;
  autonomy: string;
  messages: ChatMessage[];
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
      autonomy: "read" | "edit" | "full";
      model?: string;
      projectDir: string;
      resume?: string;
    },
    onLog: Channel<LogEvent>
  ) => invoke<void>("start_chat", { ...args, onLog }),
  cancelChat: (chatId: string) => invoke<void>("cancel_chat", { chatId }),
  loadChat: (projectDir: string) =>
    invoke<ChatSession | null>("load_chat", { projectDir }),
  saveChat: (projectDir: string, session: ChatSession) =>
    invoke<void>("save_chat", { projectDir, session }),
  startRun: (
    args: { workflowPath: string; projectDir: string; inputs: Record<string, string> },
    onLog: Channel<LogEvent>
  ) => invoke<string>("start_run", { ...args, onLog }),
  approve: (runId: string, decision: "approve" | "reject") =>
    invoke<void>("approve", { runId, decision }),
  cancel: (runId: string) => invoke<void>("cancel", { runId }),
};
