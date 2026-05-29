---
name: Create GitHub Issue
inputs: [issue_summary]
defaults: { agent: claude, autonomy: read }
---

## investigate
```step
agent: claude
model: sonnet
autonomy: read
output: context
```
Someone wants to file this issue: {{ inputs.issue_summary }}

Read the relevant parts of this codebase to gather context. Do NOT modify or run
anything. Briefly summarize the files/areas involved and anything that belongs in
the issue (reproduction hints, affected components, related code).

## draft
```step
agent: claude
model: opus
autonomy: read
output: draft
```
Context gathered from the codebase:

{{ steps.investigate.output }}

Draft a GitHub issue for: {{ inputs.issue_summary }}

Output ONLY the final issue in markdown — the first line is the title, then a
blank line, then the body with sections: **Problem**, **Expected behavior**,
**Acceptance criteria**.

## review
```step
approval: true
```

## create
```step
agent: claude
model: sonnet
autonomy: full
```
Create the issue with `gh issue create`, using exactly this drafted content (first
line = title, the remainder = body):

{{ steps.draft.output }}

Print the URL of the created issue.
