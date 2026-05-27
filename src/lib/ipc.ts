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

export { Channel };

export const api = {
  checkAgents: () => invoke<Availability[]>("check_agents"),
  listWorkflows: (projectDir: string) =>
    invoke<WorkflowSummary[]>("list_workflows", { projectDir }),
  getWorkflow: (path: string) => invoke<Workflow>("get_workflow", { path }),
  startRun: (
    args: { workflowPath: string; projectDir: string; inputs: Record<string, string> },
    onLog: Channel<LogEvent>
  ) => invoke<string>("start_run", { ...args, onLog }),
  approve: (runId: string, decision: "approve" | "reject") =>
    invoke<void>("approve", { runId, decision }),
  cancel: (runId: string) => invoke<void>("cancel", { runId }),
};
