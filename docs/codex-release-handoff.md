# Codex Release Handoff

Use this repository as an already-working `nightloop` v0 CLI, not as an empty scaffold.

Current reality:

- the CLI surface already exists: `init`, `check`, `lint`, `estimate`, `start`, `nightly`, `setup-labels`, `budget`, `record-run`
- GitHub integration, issue parsing, linting, estimation, `start`, `nightly`, Copilot review request, split staging, and telemetry are already implemented
- prompts are delivered on stdin and also written to `NIGHTLOOP_PROMPT_FILE`
- `--verbose` streams command banners and live subprocess output to `stderr`

When continuing work, optimize for release hardening and spec alignment only.

Preferred remaining work:

1. polish deterministic stacked PR behavior in `nightly`
2. keep generated target configs ergonomic for repos without a `docs/` directory
3. keep README / blueprint / example config exactly aligned with implemented behavior
4. add regression tests before widening behavior

Do not re-open broad scaffold work such as:

- replacing the CLI surface wholesale
- introducing a GitHub SDK
- building a daemon or service
- adding cloud-only infrastructure
- rebuilding parsing, estimation, or review-loop from scratch
