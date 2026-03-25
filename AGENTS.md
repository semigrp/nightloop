# AGENTS.md

## Repository expectations

- Treat GitHub child Issues as the only executable work units.
- Treat parent Issues as campaign containers and progress trackers only.
- Before implementing a child Issue, read its Source of truth section and any linked plan or ADR.
- Prefer the smallest change that satisfies the acceptance criteria.
- Keep diffs within the declared target change size whenever possible.
- If the Issue includes a suggested model profile or estimated execution time, preserve those fields and report deviations in the PR summary.
- If you change public behavior and the child Issue declares documentation impact, update the required docs in the same branch.
- Run the verification steps named in the Issue before marking work ready for review.
- If verification fails, stop and report the failure rather than widening scope.
- When recording a completed run, capture actual runtime, diff lines, files touched, and final model profile in telemetry.

## Preferred workflow

1. Read the child Issue.
2. Read `AGENTS.md` plus the linked docs.
3. Summarize the scope in one paragraph.
4. Implement only that scope.
5. Run verification.
6. Prepare a concise reviewer summary.
7. Record runtime and result into local telemetry.
