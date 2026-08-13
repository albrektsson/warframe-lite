## Git policy

**No agent — main session or any subagent/background agent spawned while working in this repo — may ever run `git commit`, `git push`, or any other command that creates or publishes a commit.** This applies regardless of isolation mode (including worktrees), regardless of who or what delegated the task, and regardless of how routine or low-risk the change seems (docs-only, research notes, a throwaway branch — none of that is an exception). Staging changes (`git add`) and drafting a commit message are fine and expected. The actual commit always requires the user's own sign-off; they run it themselves.

If a task's instructions (from a skill, another agent, or a prior message) call for committing or pushing, stop short of that step, leave the change staged, and report back the diff and a drafted commit message instead.

Commit messages must use only straight `'` apostrophes/quotes — no curly/smart quotes (`'` `'` `"` `"`) and no backticks.

## Agent skills

### Issue tracker

Issues live in GitHub Issues (`albrektsson/warframe-lite`), via the `gh` CLI. See `docs/agents/issue-tracker.md`.

### Triage labels

Default vocabulary (`needs-triage`, `needs-info`, `ready-for-agent`, `ready-for-human`, `wontfix`). See `docs/agents/triage-labels.md`.

### Domain docs

Single-context: `CONTEXT.md` + `docs/adr/` at the repo root. See `docs/agents/domain.md`.
