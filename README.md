# nightloop

`nightloop` is a Codex-first minimal parent/child issue runner.

It keeps one narrow model:

- Parent Issue = ordered campaign container
- Child Issue = executable work unit
- `start` = run the first runnable child
- `nightly` = run as many runnable children as fit in a 2–6 hour window

The CLI shells out to local tools instead of embedding provider-specific runtimes:

- `gh` for GitHub reads and writes
- `git` for branches, commits, diff accounting, and publication
- configured shell commands for planning and implementation

## Commands

The public command surface is intentionally small:

- `nightloop init canaria UTAGEDA/canaria /abs/path/to/repo`
- `nightloop check --target canaria`
- `nightloop lint path/to/child-issue.md --target canaria`
- `nightloop estimate path/to/child-issue.md --target canaria --basis template|local|hybrid`
- `nightloop start 221 --target canaria [--dry-run]`
- `nightloop nightly 221 --target canaria --hours 2|3|4|5|6 [--dry-run]`

Removed surfaces are not supported anymore:

- `setup-labels`
- `budget`
- `record-run`
- legacy aliases such as `review-loop`, `run`, `lint-issue`, `estimate-issue`, `init-target`

## Quickstart

```sh
nightloop init \
  canaria \
  UTAGEDA/canaria \
  /Users/semigrp/dev/canaria \
  --agent-command "codex exec --full-auto" \
  --plan-command "codex exec --full-auto"
```

```sh
nightloop check --target canaria
nightloop lint --target canaria path/to/child-issue.md
nightloop estimate --target canaria path/to/child-issue.md --basis hybrid
nightloop start 221 --target canaria --dry-run
nightloop nightly 221 --target canaria --hours 4 --dry-run
```

`check` is the bootstrap command. It validates required target paths and guarantees the managed workflow labels exist on GitHub.

## Config

`nightloop` can run from a control repo while operating on a separate target checkout.

- `github.owner/repo` is the GitHub source of truth
- `agent.working_directory` is the target repo root for git operations, source-of-truth resolution, execution, telemetry, and run artifacts
- `targets/<name>.toml` provides named target configs

Default required target paths are:

```toml
[docs]
required_paths = ["README.md", "AGENTS.md"]
```

Estimation runtime modes are limited to:

- `template`
- `local`
- `hybrid`

## Child Issue Contract

Required sections:

- `Background`
- `Goal`
- `Scope`
- `Out of scope`
- `Source of truth`
- `Acceptance criteria`
- `Verification`
- `Dependencies`
- `Target change size`
- `Documentation impact`
- `Suggested model profile`
- `Estimated execution time`
- `Estimation basis`
- `Estimation confidence`

Optional sections:

- `Implementation constraints`
- `Suggested model override`

`Verification` accepts only:

- fenced `sh` / `bash` / `shell` blocks
- `cmd:` lines

Below-minimum diffs are allowed only when `Scope` contains the exact line `docs-only` or `config-only`.

## Execution Rules

Eligibility requires:

- open child Issue
- `night-run`
- `agent:ready`
- no `agent:running`, `agent:blocked`, `agent:review`, or `agent:done`
- dependencies already closed/done or already planned earlier in the same run
- valid child metadata

`start`:

- selects the first runnable child
- runs `plan_command`
- runs `agent.command`
- runs parsed verification commands
- enforces diff budget
- creates a draft PR

`nightly`:

- preserves parent order
- preserves dependency order
- packs the selected nightly window using issue estimates
- creates one stacked child branch and draft PR per successful child

Nightly publication hardening is retained:

- each successful nightly child branch is pushed explicitly before `gh pr create`
- later nightly child PRs base on the previous successful child branch

Oversize changes are terminal for that child:

- if the produced diff exceeds the allowed budget, status becomes `split_required`
- no split state or split docs are written

## Verification

The repo verification gate for this slimmed-down surface is:

```sh
cargo fmt --check
cargo test
```
