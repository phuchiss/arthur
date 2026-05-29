use super::context::{RunContext, StepResult};
use super::model::{AgentInvocation, AgentResult, Mode, Workflow};
use super::{expr, Decision, EventSink, LogEvent};
use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::mpsc::Receiver;
use tokio_util::sync::CancellationToken;

pub type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

/// Runs one resolved agent invocation. The real implementation spawns a CLI;
/// tests substitute a mock. Decoupling here keeps the engine free of any
/// process/Tauri dependency.
pub type AgentRunner = Arc<
    dyn Fn(AgentInvocation, Arc<dyn EventSink>, CancellationToken) -> BoxFuture<Result<AgentResult, String>>
        + Send
        + Sync,
>;

#[derive(Debug, Clone, PartialEq)]
pub enum RunOutcome {
    Done,
    Rejected,
    Cancelled,
    Error(String),
}

const MAX_TRANSITIONS: u32 = 1000;

/// Execute a workflow: sequential by default, with `when` (skip), `approval`
/// (pause), `retry`/`until` (loop), and `goto` (jump).
pub async fn run_workflow(
    workflow: &Workflow,
    inputs: HashMap<String, String>,
    working_dir: PathBuf,
    runner: AgentRunner,
    sink: Arc<dyn EventSink>,
    cancel: CancellationToken,
    mut decision_rx: Receiver<Decision>,
) -> RunOutcome {
    let mut ctx = RunContext {
        inputs,
        ..Default::default()
    };
    let steps = &workflow.steps;
    let index: HashMap<&str, usize> = steps
        .iter()
        .enumerate()
        .map(|(i, s)| (s.id.as_str(), i))
        .collect();

    let mut pc = 0usize;
    let mut transitions = 0u32;

    while pc < steps.len() {
        if cancel.is_cancelled() {
            sink.emit(LogEvent::Cancelled);
            return RunOutcome::Cancelled;
        }
        transitions += 1;
        if transitions > MAX_TRANSITIONS {
            let msg = "max step transitions exceeded (possible infinite loop)".to_string();
            sink.emit(LogEvent::Error { message: msg.clone() });
            return RunOutcome::Error(msg);
        }

        let step = &steps[pc];

        // `when`: skip the step entirely if the condition is false.
        if let Some(cond) = &step.config.when {
            match expr::eval_bool(cond, &ctx, &[]) {
                Ok(false) => {
                    sink.emit(LogEvent::StepSkipped {
                        step_id: step.id.clone(),
                    });
                    pc += 1;
                    continue;
                }
                Ok(true) => {}
                Err(e) => {
                    sink.emit(LogEvent::Error { message: e.clone() });
                    return RunOutcome::Error(e);
                }
            }
        }

        // Human approval gate (before running the step).
        if step.config.approval {
            sink.emit(LogEvent::AwaitingApproval {
                step_id: step.id.clone(),
                title: step.title.clone(),
            });
            let decision = tokio::select! {
                _ = cancel.cancelled() => {
                    sink.emit(LogEvent::Cancelled);
                    return RunOutcome::Cancelled;
                }
                d = decision_rx.recv() => d.unwrap_or(Decision::Reject),
            };
            match decision {
                Decision::Approve => sink.emit(LogEvent::Approved {
                    step_id: step.id.clone(),
                }),
                Decision::Reject => {
                    sink.emit(LogEvent::Rejected {
                        step_id: step.id.clone(),
                    });
                    return RunOutcome::Rejected;
                }
            }
        }

        // Run an agent only when the step has a prompt. A step with an empty
        // body is a control/gate step (approval and/or goto only).
        if !step.prompt.trim().is_empty() {
            // Resolve agent / model / mode with workflow defaults.
            let agent = step
                .config
                .agent
                .clone()
                .or_else(|| workflow.defaults.agent.clone())
                .unwrap_or_else(|| "claude".to_string());
            let model = step.config.model.clone().or_else(|| workflow.defaults.model.clone());
            let mode = step
                .config
                .mode
                .or(workflow.defaults.mode)
                .unwrap_or(Mode::AcceptEdits);

            // Run the step, retrying while `until` is false (up to `max`).
            let max_attempts = step.config.retry.as_ref().map(|r| r.max.max(1)).unwrap_or(1);
            let mut attempt = 0u32;
            loop {
                if cancel.is_cancelled() {
                    sink.emit(LogEvent::Cancelled);
                    return RunOutcome::Cancelled;
                }
                attempt += 1;
                sink.emit(LogEvent::StepStarted {
                    step_id: step.id.clone(),
                    title: step.title.clone(),
                    agent: agent.clone(),
                    model: model.clone(),
                    attempt,
                });

                let prompt = ctx.render(&step.prompt);
                let invocation = AgentInvocation {
                    agent: agent.clone(),
                    model: model.clone(),
                    mode,
                    prompt,
                    working_dir: working_dir.clone(),
                    step_id: step.id.clone(),
                    resume: None,
                };

                let result = match (runner)(invocation, sink.clone(), cancel.clone()).await {
                    Ok(r) => r,
                    Err(e) => {
                        if cancel.is_cancelled() {
                            sink.emit(LogEvent::Cancelled);
                            return RunOutcome::Cancelled;
                        }
                        sink.emit(LogEvent::Error { message: e.clone() });
                        return RunOutcome::Error(e);
                    }
                };

                ctx.steps.insert(
                    step.id.clone(),
                    StepResult {
                        output: result.final_text.clone(),
                        exit_code: result.exit_code,
                    },
                );
                if let Some(name) = &step.config.output {
                    ctx.artifacts.insert(name.clone(), result.final_text.clone());
                }
                sink.emit(LogEvent::StepFinished {
                    step_id: step.id.clone(),
                    exit_code: result.exit_code,
                    attempt,
                    final_text: result.final_text.clone(),
                });

                match &step.config.retry {
                    Some(retry) => {
                        let stop = expr::eval_bool(
                            &retry.until,
                            &ctx,
                            &[("exit_code", result.exit_code as i64), ("attempts", attempt as i64)],
                        );
                        match stop {
                            Ok(done) => {
                                if done || attempt >= max_attempts {
                                    break;
                                }
                                sink.emit(LogEvent::Retrying {
                                    step_id: step.id.clone(),
                                    attempt,
                                });
                            }
                            Err(e) => {
                                sink.emit(LogEvent::Error { message: e.clone() });
                                return RunOutcome::Error(e);
                            }
                        }
                    }
                    None => break,
                }
            }
        }

        // `goto` (on any step) branches after the step completes.
        if let Some(target) = &step.config.goto {
            match index.get(target.as_str()) {
                Some(&t) => {
                    sink.emit(LogEvent::Goto {
                        from: step.id.clone(),
                        to: target.clone(),
                    });
                    pc = t;
                    continue;
                }
                None => {
                    let e = format!("goto target '{target}' not found");
                    sink.emit(LogEvent::Error { message: e.clone() });
                    return RunOutcome::Error(e);
                }
            }
        }

        pc += 1;
    }

    sink.emit(LogEvent::Done);
    RunOutcome::Done
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::model::{Step, StepConfig};
    use std::sync::Mutex as StdMutex;
    use tokio::sync::mpsc;

    #[derive(Default)]
    struct VecSink(StdMutex<Vec<LogEvent>>);
    impl EventSink for VecSink {
        fn emit(&self, event: LogEvent) {
            self.0.lock().unwrap().push(event);
        }
    }

    fn step(id: &str, cfg: StepConfig, prompt: &str) -> Step {
        Step {
            id: id.to_string(),
            title: id.to_string(),
            config: cfg,
            prompt: prompt.to_string(),
        }
    }

    fn wf(steps: Vec<Step>) -> Workflow {
        Workflow {
            name: "t".into(),
            inputs: vec![],
            defaults: Default::default(),
            steps,
            path: None,
        }
    }

    /// Mock runner: records executed step ids and returns scripted exit codes
    /// (the `build` step fails twice then succeeds, `run` always fails).
    fn mock_runner(calls: Arc<StdMutex<Vec<String>>>) -> AgentRunner {
        let attempts = Arc::new(StdMutex::new(HashMap::<String, u32>::new()));
        Arc::new(move |inv, _sink, _cancel| {
            let calls = calls.clone();
            let attempts = attempts.clone();
            Box::pin(async move {
                calls.lock().unwrap().push(inv.step_id.clone());
                let n = {
                    let mut a = attempts.lock().unwrap();
                    let e = a.entry(inv.step_id.clone()).or_insert(0);
                    *e += 1;
                    *e
                };
                let exit = match inv.step_id.as_str() {
                    "build" => {
                        if n >= 3 {
                            0
                        } else {
                            1
                        }
                    }
                    "run" => 1,
                    _ => 0,
                };
                Ok(AgentResult {
                    final_text: format!("ran {}", inv.step_id),
                    exit_code: exit,
                })
            })
        })
    }

    async fn execute(workflow: Workflow, decisions: Vec<Decision>) -> (RunOutcome, Vec<String>) {
        let calls = Arc::new(StdMutex::new(Vec::new()));
        let sink: Arc<dyn EventSink> = Arc::new(VecSink::default());
        let (tx, rx) = mpsc::channel(8);
        for d in decisions {
            tx.send(d).await.unwrap();
        }
        drop(tx);
        let outcome = run_workflow(
            &workflow,
            HashMap::new(),
            PathBuf::from("/tmp"),
            mock_runner(calls.clone()),
            sink,
            CancellationToken::new(),
            rx,
        )
        .await;
        let executed = calls.lock().unwrap().clone();
        (outcome, executed)
    }

    #[tokio::test]
    async fn retries_until_success() {
        let retry_cfg = StepConfig {
            retry: Some(crate::engine::model::Retry {
                max: 3,
                until: "exit_code == 0".into(),
            }),
            ..Default::default()
        };
        let (outcome, executed) =
            execute(wf(vec![step("build", retry_cfg, "go")]), vec![]).await;
        assert_eq!(outcome, RunOutcome::Done);
        assert_eq!(executed, vec!["build", "build", "build"]);
    }

    #[tokio::test]
    async fn branches_via_when_and_goto() {
        let branch = StepConfig {
            when: Some("{{ steps.run.exit_code }} != 0".into()),
            goto: Some("recover".into()),
            ..Default::default()
        };
        let workflow = wf(vec![
            step("run", StepConfig::default(), "go"),
            step("branch", branch, ""),
            step("normal-end", StepConfig::default(), "skip me"),
            step("recover", StepConfig::default(), "fix it"),
        ]);
        let (outcome, executed) = execute(workflow, vec![]).await;
        assert_eq!(outcome, RunOutcome::Done);
        // run fails -> branch jumps to recover, skipping normal-end
        assert_eq!(executed, vec!["run", "recover"]);
    }

    #[tokio::test]
    async fn approval_gate_approve_then_run() {
        let gate = StepConfig {
            approval: true,
            ..Default::default()
        };
        let workflow = wf(vec![
            step("gate", gate, ""),
            step("after", StepConfig::default(), "go"),
        ]);
        let (outcome, executed) = execute(workflow, vec![Decision::Approve]).await;
        assert_eq!(outcome, RunOutcome::Done);
        assert_eq!(executed, vec!["after"]);
    }

    #[tokio::test]
    async fn approval_gate_reject_stops() {
        let gate = StepConfig {
            approval: true,
            ..Default::default()
        };
        let workflow = wf(vec![
            step("gate", gate, ""),
            step("after", StepConfig::default(), "go"),
        ]);
        let (outcome, executed) = execute(workflow, vec![Decision::Reject]).await;
        assert_eq!(outcome, RunOutcome::Rejected);
        assert!(executed.is_empty());
    }
}
