---
name: Multi-Agent Code Review
inputs: [scope]
defaults: { agent: claude, autonomy: read }
---

## claude-review
```step
agent: claude
model: opus
autonomy: full
output: claude_notes
```
Review the current code changes ({{ inputs.scope }}). Use `git diff` to see them.
Focus on correctness bugs and edge cases. List findings concisely with file:line
references. Do NOT modify any files.

## gemini-review
```step
agent: gemini
autonomy: full
output: gemini_notes
```
Independently review the current code changes ({{ inputs.scope }}). Use `git diff`.
Focus on maintainability, naming, and design. List findings concisely. Do NOT
modify any files.

## synthesize
```step
agent: claude
model: sonnet
autonomy: read
output: summary
```
Merge these two independent reviews into one prioritized list. Deduplicate
overlaps and mark each finding's severity (high / medium / low).

### Claude — correctness
{{ steps.claude-review.output }}

### Gemini — maintainability
{{ steps.gemini-review.output }}
