use super::model::{Defaults, Step, StepConfig, Workflow};
use serde::Deserialize;

#[derive(Debug, Default, Deserialize)]
struct WorkflowMeta {
    name: Option<String>,
    #[serde(default)]
    inputs: Vec<String>,
    #[serde(default)]
    defaults: Defaults,
}

/// Parse a Markdown playbook into a `Workflow`.
///
/// Format: optional YAML frontmatter (`--- ... ---`), then one `## heading`
/// per step. Each step may begin with a ```` ```step ```` fenced YAML block
/// (its `StepConfig`); the remaining markdown body is the prompt template.
pub fn parse_workflow(src: &str, path: Option<String>) -> Result<Workflow, String> {
    let (frontmatter, body) = split_frontmatter(src);
    let meta: WorkflowMeta = match frontmatter {
        Some(f) if !f.trim().is_empty() => {
            serde_norway::from_str(&f).map_err(|e| format!("frontmatter error: {e}"))?
        }
        _ => WorkflowMeta::default(),
    };

    let steps = parse_steps(&body)?;
    if steps.is_empty() {
        return Err("no steps found (a playbook needs at least one `## ` heading)".into());
    }

    let name = meta
        .name
        .filter(|s| !s.trim().is_empty())
        .or_else(|| path.as_deref().and_then(file_stem))
        .unwrap_or_else(|| "workflow".to_string());

    Ok(Workflow {
        name,
        inputs: meta.inputs,
        defaults: meta.defaults,
        steps,
        path,
    })
}

fn file_stem(p: &str) -> Option<String> {
    std::path::Path::new(p)
        .file_stem()
        .and_then(|s| s.to_str())
        .map(|s| s.to_string())
}

fn split_frontmatter(src: &str) -> (Option<String>, String) {
    let src = src.strip_prefix('\u{feff}').unwrap_or(src);
    let mut lines = src.lines();
    if lines.next().map(str::trim) != Some("---") {
        return (None, src.to_string());
    }
    let mut fm = String::new();
    let mut body: Vec<&str> = Vec::new();
    let mut closed = false;
    for line in lines {
        if !closed && line.trim() == "---" {
            closed = true;
            continue;
        }
        if closed {
            body.push(line);
        } else {
            fm.push_str(line);
            fm.push('\n');
        }
    }
    if !closed {
        return (None, src.to_string());
    }
    (Some(fm), body.join("\n"))
}

fn parse_steps(body: &str) -> Result<Vec<Step>, String> {
    let mut steps = Vec::new();
    let mut cur: Option<(String, Vec<String>)> = None;
    for line in body.lines() {
        if let Some(rest) = line.strip_prefix("## ") {
            if let Some((title, lines)) = cur.take() {
                steps.push(build_step(title, lines)?);
            }
            cur = Some((rest.trim().to_string(), Vec::new()));
        } else if let Some((_, lines)) = cur.as_mut() {
            lines.push(line.to_string());
        }
    }
    if let Some((title, lines)) = cur.take() {
        steps.push(build_step(title, lines)?);
    }
    Ok(steps)
}

fn build_step(title: String, lines: Vec<String>) -> Result<Step, String> {
    let id = slugify(&title);
    let mut config = StepConfig::default();
    let mut prompt_lines: Vec<String> = Vec::new();
    let mut got_config = false;
    let mut i = 0;
    while i < lines.len() {
        let trimmed = lines[i].trim_start();
        if !got_config && is_step_fence(trimmed) {
            i += 1;
            let mut yaml = String::new();
            while i < lines.len() && !lines[i].trim_start().starts_with("```") {
                yaml.push_str(&lines[i]);
                yaml.push('\n');
                i += 1;
            }
            if i < lines.len() {
                i += 1; // skip closing fence
            }
            if !yaml.trim().is_empty() {
                config = serde_norway::from_str(&yaml)
                    .map_err(|e| format!("step '{id}' config error: {e}"))?;
            }
            got_config = true;
            continue;
        }
        prompt_lines.push(lines[i].clone());
        i += 1;
    }

    Ok(Step {
        id,
        title,
        config,
        prompt: prompt_lines.join("\n").trim().to_string(),
    })
}

fn is_step_fence(trimmed: &str) -> bool {
    trimmed.starts_with("```") && trimmed.trim_start_matches('`').trim() == "step"
}

fn slugify(s: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            dash = false;
        } else if !out.is_empty() && !dash {
            out.push('-');
            dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        "step".to_string()
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::model::Mode;

    const SAMPLE: &str = r#"---
name: Add Feature
inputs: [feature_description]
defaults: { agent: claude, mode: accept_edits }
---

## plan
```step
agent: claude
model: opus
mode: accept_edits
output: plan
```
Plan this feature: {{ inputs.feature_description }}

## review
```step
approval: true
```

## test
```step
agent: claude
mode: auto
retry: { max: 3, until: "exit_code == 0" }
```
Run the tests.

## fork
```step
when: "{{ steps.test.exit_code }} != 0"
goto: test
```
"#;

    #[test]
    fn parses_meta_and_steps() {
        let wf = parse_workflow(SAMPLE, Some("/x/add-feature.md".into())).unwrap();
        assert_eq!(wf.name, "Add Feature");
        assert_eq!(wf.inputs, vec!["feature_description"]);
        assert_eq!(wf.defaults.agent.as_deref(), Some("claude"));
        assert_eq!(wf.steps.len(), 4);

        let plan = &wf.steps[0];
        assert_eq!(plan.id, "plan");
        assert_eq!(plan.config.agent.as_deref(), Some("claude"));
        assert_eq!(plan.config.model.as_deref(), Some("opus"));
        assert_eq!(plan.config.mode, Some(Mode::AcceptEdits));
        assert_eq!(plan.config.output.as_deref(), Some("plan"));
        assert!(plan.prompt.contains("Plan this feature: {{ inputs.feature_description }}"));

        assert!(wf.steps[1].config.approval);

        let test = &wf.steps[2];
        assert_eq!(test.config.mode, Some(Mode::Auto));
        let retry = test.config.retry.as_ref().unwrap();
        assert_eq!(retry.max, 3);
        assert_eq!(retry.until, "exit_code == 0");

        let fork = &wf.steps[3];
        assert_eq!(fork.config.goto.as_deref(), Some("test"));
        assert!(fork.config.when.is_some());
        assert!(fork.prompt.is_empty());
    }

    #[test]
    fn errors_without_steps() {
        assert!(parse_workflow("just prose, no headings", None).is_err());
    }

    #[test]
    fn parses_transport_and_interactive() {
        let src = r#"## grill
```step
transport: acp
interactive: true
```
ask away
"#;
        let wf = parse_workflow(src, None).unwrap();
        let cfg = &wf.steps[0].config;
        assert_eq!(
            cfg.transport,
            Some(crate::engine::model::Transport::Acp),
            "transport: acp must deserialise to Transport::Acp"
        );
        assert!(cfg.interactive, "interactive: true must deserialise as true");
    }

    /// Reproduce a real-world workflow file that paired `interactive: true`
    /// with a trailing inline comment (`# ← ใหม่`) and an unknown frontmatter
    /// key (`autonomy:`). Both must parse cleanly: the comment is stripped by
    /// YAML, the unknown key is silently ignored by serde, and `interactive`
    /// still deserialises as true.
    #[test]
    fn parses_interactive_with_inline_comment_and_unknown_keys() {
        let src = "---\nname: Add Feature with GM\ninputs: [feature_description]\ndefaults: { agent: claude, autonomy: edit }\n---\n\n## grill\n```step\nagent: claude\nmodel: opus\nmode: auto\ntransport: acp\ninteractive: true     # ← ใหม่\noutput: grill\n```\nbody\n";
        let wf = parse_workflow(src, None).unwrap();
        assert_eq!(wf.name, "Add Feature with GM");
        let grill = wf.steps.iter().find(|s| s.id == "grill").unwrap();
        assert_eq!(
            grill.config.transport,
            Some(crate::engine::model::Transport::Acp)
        );
        assert!(grill.config.interactive, "inline `# ← ใหม่` comment must not break parsing");
    }

    #[test]
    fn example_playbooks_parse() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../.arthur/workflows");
        let mut count = 0;
        for entry in std::fs::read_dir(dir).expect("workflows dir should exist") {
            let path = entry.unwrap().path();
            if path.extension().and_then(|e| e.to_str()) == Some("md") {
                let src = std::fs::read_to_string(&path).unwrap();
                let wf = parse_workflow(&src, Some(path.display().to_string()))
                    .unwrap_or_else(|e| panic!("failed to parse {}: {e}", path.display()));
                assert!(!wf.steps.is_empty(), "{} has no steps", path.display());
                count += 1;
            }
        }
        assert!(count >= 1, "expected at least one example playbook");
    }
}
