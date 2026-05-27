use std::collections::HashMap;

#[derive(Debug, Clone, Default)]
pub struct StepResult {
    pub output: String,
    pub exit_code: i32,
}

/// Mutable state carried across a workflow run. Used both for `{{ }}` template
/// expansion in prompts/conditions and for passing artifacts between steps.
#[derive(Debug, Default)]
pub struct RunContext {
    pub inputs: HashMap<String, String>,
    pub steps: HashMap<String, StepResult>,
    pub artifacts: HashMap<String, String>,
}

impl RunContext {
    /// Resolve a dotted reference like `inputs.x`, `steps.plan.output`,
    /// `steps.test.exit_code`, or `artifacts.plan`.
    pub fn lookup(&self, path: &str) -> Option<String> {
        let parts: Vec<&str> = path.split('.').map(str::trim).collect();
        match parts.as_slice() {
            ["inputs", k] => self.inputs.get(*k).cloned(),
            ["artifacts", k] => self.artifacts.get(*k).cloned(),
            ["steps", id, "output"] => self.steps.get(*id).map(|r| r.output.clone()),
            ["steps", id, "exit_code"] => self.steps.get(*id).map(|r| r.exit_code.to_string()),
            _ => None,
        }
    }

    /// Replace every `{{ ref }}` in `template` with the resolved value
    /// (empty string when unknown).
    pub fn render(&self, template: &str) -> String {
        let mut out = String::new();
        let mut rest = template;
        while let Some(start) = rest.find("{{") {
            out.push_str(&rest[..start]);
            let after = &rest[start + 2..];
            match after.find("}}") {
                Some(end) => {
                    let key = after[..end].trim();
                    out.push_str(&self.lookup(key).unwrap_or_default());
                    rest = &after[end + 2..];
                }
                None => {
                    out.push_str(&rest[start..]);
                    rest = "";
                }
            }
        }
        out.push_str(rest);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_inputs_and_step_refs() {
        let mut ctx = RunContext::default();
        ctx.inputs.insert("feature".into(), "dark mode".into());
        ctx.steps.insert(
            "test".into(),
            StepResult {
                output: "ok".into(),
                exit_code: 1,
            },
        );
        assert_eq!(ctx.render("build {{ inputs.feature }}"), "build dark mode");
        assert_eq!(ctx.render("{{ steps.test.exit_code }} != 0"), "1 != 0");
        assert_eq!(ctx.render("missing={{ inputs.nope }}!"), "missing=!");
    }
}
