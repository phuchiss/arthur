---
name: Add Feature
inputs: [feature_description]
defaults: { agent: claude, autonomy: edit }
---

## plan
```step
agent: claude
model: opus
autonomy: edit
output: plan
```
You are planning a new feature for this project: {{ inputs.feature_description }}

Explore the relevant code, then write a concise, step-by-step implementation
plan to a file named PLAN.md at the repo root. Keep it focused and actionable.

## review
```step
approval: true
```

## implement
```step
agent: codex
model: gpt-5-codex
autonomy: full
```
Implement the plan described in PLAN.md. Make the necessary code changes across
the repo, following existing conventions. Do not commit yet.

## test
```step
agent: claude
model: sonnet
autonomy: full
retry: { max: 3, until: "exit_code == 0" }
```
Run the project's test suite (plus linters/build if present). If anything fails,
fix the offending code and run again. Exit non-zero only if it still fails after
your fixes.

## fork
```step
when: "{{ steps.test.exit_code }} != 0"
goto: implement
```

## pr
```step
agent: claude
model: sonnet
autonomy: full
approval: true
```
Stage the changes, write a clear commit message, and open a pull request with
`gh pr create`. Summarize what changed and why, referencing the feature:
{{ inputs.feature_description }}
