---
name: Add Feature
inputs: [feature_description, grill_language]
defaults: { agent: claude, autonomy: edit }
---

## plan
```step
agent: claude
model: opus
mode: auto
output: plan
```
You are planning a new feature for this project: {{ inputs.feature_description }}

Explore the relevant code, then write a concise, step-by-step implementation
plan to a file named `plans/{{ run.started_at | date: "%Y%m%d-%H%M%S" }}/PLAN.md`
(create the directory if it does not exist). Keep it focused and actionable.

Record the exact directory path you used so later steps can reference it.

## grill
```step
agent: claude
model: opus
mode: auto
transport: acp
interactive: true
output: grill
```
Invoke the `grill-me` skill to stress-test the plan produced in the `plan` step
(located at `plans/<timestamp>/PLAN.md`). Interview relentlessly to surface
hidden assumptions, ambiguous requirements, and unresolved decision branches.

Conduct the entire grilling session in **{{ inputs.grill_language }}** — every
question, every option, every recommendation must be written in that language.
Code identifiers stay in English.

After the grilling session, update the same `PLAN.md` in place to incorporate
the clarifications, resolved decisions, and any newly discovered requirements.
Keep the plan focused and actionable.

## review
```step
approval: true
```

## implement
```step
agent: claude
model: opus
mode: auto
```
Implement the plan described in the `PLAN.md` produced by the `plan` step
(under `plans/<timestamp>/PLAN.md`, where `<timestamp>` is the `YYYYMMDD-HHMMSS`
directory created earlier). Make the necessary code changes across the repo,
following existing conventions. Do not commit yet.

## test
```step
agent: claude
model: sonnet
mode: auto
retry: { max: 3, until: "exit_code == 0" }
```
Run the project's test suite (plus linters/build if present). If anything fails,
fix the offending code and run again. Exit non-zero only if it still fails after
your fixes.
