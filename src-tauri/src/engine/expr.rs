use super::context::RunContext;
use evalexpr::{eval_boolean_with_context, ContextWithMutableVariables, HashMapContext, Value};

/// Evaluate a boolean condition (used by `when` and `retry.until`).
///
/// `{{ }}` references are expanded from `ctx` first; `extra` injects bare
/// numeric variables (e.g. `exit_code`, `attempts`) referenced directly.
pub fn eval_bool(expr: &str, ctx: &RunContext, extra: &[(&str, i64)]) -> Result<bool, String> {
    let rendered = ctx.render(expr);
    let mut ec: HashMapContext = HashMapContext::new();
    for (k, v) in extra {
        ec.set_value((*k).to_string(), Value::Int(*v))
            .map_err(|e| e.to_string())?;
    }
    eval_boolean_with_context(&rendered, &ec)
        .map_err(|e| format!("condition '{rendered}' failed: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evaluates_rendered_and_extra_vars() {
        let mut ctx = RunContext::default();
        ctx.steps.insert(
            "test".into(),
            super::super::context::StepResult {
                output: String::new(),
                exit_code: 2,
            },
        );
        // rendered template comparison
        assert!(eval_bool("{{ steps.test.exit_code }} != 0", &ctx, &[]).unwrap());
        // bare variable injected via extra
        assert!(eval_bool("exit_code == 0", &ctx, &[("exit_code", 0)]).unwrap());
        assert!(!eval_bool("exit_code == 0", &ctx, &[("exit_code", 1)]).unwrap());
        assert!(eval_bool("exit_code == 0 || attempts >= 3", &ctx, &[("exit_code", 1), ("attempts", 3)]).unwrap());
    }
}
