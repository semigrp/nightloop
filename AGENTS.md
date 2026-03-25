# AGENTS.md

## Repository Expectations

- Treat GitHub child Issues as the only executable work units.
- Treat parent Issues as campaign containers and progress trackers only.
- Read the child Issue `Source of truth` section before changing code.
- Keep diffs inside the declared target change size whenever possible.
- If the child Issue uses `Documentation impact != none`, update the required docs in the same branch.
- Do not widen scope beyond the child Issue acceptance criteria.
- Run the parsed verification commands before considering a task complete.
- Record actual runtime, changed lines, files touched, and final model profile in telemetry.

## Child Issue Conventions

- `Source of truth` is one reference per non-empty line.
- `Verification` must use either `cmd:` lines or a fenced `sh` / `bash` / `shell` block.
- Use an exact `docs-only` or `config-only` line in `Scope` only when the task is intentionally allowed to land below the normal diff minimum.
- Preserve the suggested model profile unless there is a concrete reason to override it.

## Preferred Workflow

1. Read the child Issue.
2. Read `AGENTS.md` plus the linked docs.
3. Summarize the scope in one paragraph.
4. Implement only that scope.
5. Run verification.
6. Prepare a concise reviewer summary.
7. Record runtime and result into local telemetry.
