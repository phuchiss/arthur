---
name: Fix Bug
inputs: [bug_report]
defaults: { agent: claude, autonomy: full }
---

## reproduce
```step
agent: claude
model: sonnet
autonomy: full
output: cause
```
Reproduce and diagnose this bug: {{ inputs.bug_report }}

Run whatever is needed to confirm it and find the root cause. Summarize the cause
and which files need to change. Do NOT fix it yet.

## review-cause
```step
approval: true
```

## fix
```step
agent: codex
model: gpt-5-codex
autonomy: full
```
Fix the root cause described below. Make the minimal change and do not commit.

{{ steps.reproduce.output }}

## verify
```step
agent: claude
model: sonnet
autonomy: full
retry: { max: 3, until: "exit_code == 0" }
```
Run the test suite, and add a regression test for this bug if appropriate. If
anything fails, fix it and run again. Exit non-zero only if it still fails.

## retry-fix
```step
when: "{{ steps.verify.exit_code }} != 0"
goto: fix
```

## pr
```step
agent: claude
model: sonnet
autonomy: full
approval: true
```
Commit the fix and open a PR with `gh pr create`. Describe the root cause and the
fix, referencing: {{ inputs.bug_report }}
