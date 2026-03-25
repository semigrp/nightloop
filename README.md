# nightloop

`nightloop` is a minimal Rust CLI for local, issue-first nightly execution of coding agents.

It treats GitHub child Issues as the only executable work units:

- Parent Issue = campaign / plan container
- Child Issue = runnable implementation unit
- Night run = execute as many runnable child Issues as fit in a selected 2–6 hour budget

The core stays provider-agnostic. `nightloop` shells out to local tools instead of embedding a GitHub SDK or a vendor-specific agent runtime:

- `gh` for GitHub reads and writes
- `git` for branches, commits, and diff accounting
- configured shell commands for agent execution and optional AI estimation

## Status

This repository now implements the v0 CLI surface:

- `nightloop init canaria UTAGEDA/canaria /abs/path/to/repo`
- `nightloop check --target canaria`
- `nightloop lint path/to/issue.md --target canaria`
- `nightloop estimate path/to/issue.md --target canaria --basis template|local|hybrid|ai`
- `nightloop start 221 --target canaria [--dry-run]`
- `nightloop nightly 221 --target canaria --hours 2|3|4|5|6 [--dry-run]`
- `nightloop setup-labels --target canaria`
- `nightloop budget --hours 2|3|4|5|6`
- `nightloop record-run path/to/run-record.json`

Legacy aliases remain supported:

- `init-target`, `docs-check`, `lint-issue`, `estimate-issue`, `review-loop`, `run`

Output is intentionally compact and machine-readable on `stdout`. Real `start` and `nightly` runs also emit live human progress lines on `stderr`.

## Required Local Tools

- Rust 1.78-compatible toolchain
- `git`
- `gh`
- a configured agent command in `nightloop.toml`

For real `start` or `nightly` execution:

- `gh auth status` must succeed
- the git worktree must be clean before the run starts
- the configured base branch must exist locally
- if `github.request_copilot_review = true`, the repo/org must support Copilot review requests and the authenticated user must be allowed to request reviewers

## Quickstart

1. Create a named target once from the control repo:

```sh
nightloop init \
  canaria \
  UTAGEDA/canaria \
  /Users/semigrp/dev/canaria \
  --agent-command "codex exec --full-auto" \
  --plan-command "codex exec --full-auto" \
  --default-model gpt-5.4 \
  --default-reasoning-effort medium \
  --request-copilot-review
```

2. Ensure `gh` is authenticated for the target repository.
3. Author parent and child Issues using the templates in [`.github/ISSUE_TEMPLATE/`](/Users/semigrp/dev/nightloop/.github/ISSUE_TEMPLATE).
4. Check issue quality locally:

```sh
nightloop check --target canaria
nightloop lint --target canaria path/to/child-issue.md
nightloop estimate --target canaria path/to/child-issue.md --basis hybrid
```

5. Simulate the default Codex-first single-child workflow:

```sh
nightloop start 221 --target canaria --dry-run
```

6. Execute it for real:

```sh
nightloop start 221 --target canaria
```

Advanced or one-off usage can still bypass the target registry or use the multi-child nightly scheduler:

```sh
nightloop nightly 221 --target canaria --hours 4 --dry-run
nightloop --config /abs/path/to/nightloop.toml nightly 221 --hours 4
```

`init` can fill the common initial settings so you do not need to hand-edit the generated TOML for a normal setup. Supported flags are:

- `--base-branch`
- `--agent-command`
- `--plan-command`
- `--default-model`
- `--default-reasoning-effort`
- `--request-copilot-review`

## Control Repo Mode

`nightloop` can run from a control checkout while operating on a different target repository.

- `github.owner/repo` tells `gh` which repository to read and write.
- `agent.working_directory` is the canonical target repo root for all local git operations, agent execution, repo-relative source-of-truth checks, docs checks, telemetry, and default run artifacts.
- bundled files that ship with `nightloop`, such as `prompts/` and `docs/templates/`, are resolved from the directory containing `nightloop.toml`
- named targets live under `targets/<name>.toml` in the control repo

Recommended layout:

- keep the `nightloop` source checkout as the control repo
- point `agent.working_directory` at the target repo checkout
- keep `loop.run_root` and `[telemetry].history_path` relative so they land under the target repo by default
- files under the resolved `loop.run_root` are ignored by the clean-worktree preflight, so reruns do not fail just because prior run artifacts exist

Example:

```toml
[github]
owner = "other-org"
repo = "other-repo"
base_branch = "main"

[agent]
command = "codex exec --full-auto"
plan_command = "codex exec --full-auto"
working_directory = "/absolute/path/to/other-repo"
default_model = "gpt-5.4"
default_reasoning_effort = "medium"
```

Normal invocation from the control repo:

```sh
nightloop check --target canaria
nightloop start 221 --target canaria --dry-run
nightloop nightly 221 --target canaria --hours 4 --dry-run
```

## Issue Contracts

### Parent Issues

`nightloop nightly` and `nightloop start` parse the parent `## Ordered child Issues` section and preserve the listed order.

Supported checklist lines:

- `- [ ] #222 first child`
- `- [x] #223 already done`
- `- [ ] #224 depends on #223`

The checkbox state is tracking-only. GitHub Issue state and labels are the execution truth.

### Child Issues

`lint` and both execution workflows validate child Issues against the following sections:

- `## Background`
- `## Goal`
- `## Scope`
- `## Out of scope`
- `## Source of truth`
- `## Implementation constraints` (optional)
- `## Acceptance criteria`
- `## Verification`
- `## Dependencies`
- `## Target change size`
- `## Documentation impact`
- `## Suggested model profile`
- `## Suggested model override` (optional)
- `## Estimated execution time`
- `## Estimation basis`
- `## Estimation confidence`

The parser accepts both `## Heading` and `### Heading`, so local markdown snapshots and GitHub Issue form bodies both work.

### Source Of Truth

Inside `## Source of truth`, use one reference per non-empty line.

Allowed forms:

- repo-relative paths
- absolute local paths
- `http://` or `https://` URLs

`lint` validates local paths exist. URL validation is syntax-only.

Repo-relative paths are resolved against `agent.working_directory`, not the current shell directory.

### Verification

`nightloop` only accepts two verification formats:

1. A fenced shell block:

```sh
cargo test
cargo fmt --check
```

2. `cmd:` lines:

- `cmd: cargo test`
- `cmd: cargo fmt --check`

Free-form prose in `## Verification` is ignored. If the section exists but yields zero parseable commands, lint fails.

### Small Diff Exceptions

Below-minimum diffs are accepted only when `## Scope` contains an exact line:

- `docs-only`
- `config-only`

That exception is conservative by design. The runner does not guess intent from prose.

## Scheduling And Eligibility

Hours are selected explicitly between 2 and 6.

`budget` still reports the fallback slot model:

- `fallback_slots = floor((hours * 60 - fixed_overhead_minutes) / fallback_cycle_minutes)`

`nightly` uses issue-specific estimated minutes instead:

- reserve fixed overhead
- preserve parent order
- preserve dependency order
- select issues until the remaining nightly budget is exhausted

A child Issue is eligible only if all of the following are true:

- the Issue is open
- it has the `night-run` label
- it has the `agent:ready` label
- it does not already have `agent:running`, `agent:blocked`, `agent:done`, or `agent:review`
- dependencies are already closed / done on GitHub or planned/completed earlier in the same run
- child metadata parses and lints successfully
- the target size band fits within the configured global diff limits

`nightloop nightly` and `nightloop start` auto-create any missing managed workflow labels before a real run starts. `setup-labels` remains available as an explicit bootstrap command.

## Review Loop

`nightloop start` is the Codex-first single-child path.

Behavior:

- chooses the first runnable child Issue from the parent campaign
- runs `plan_command` first and saves the plan artifact
- runs `agent.command` to implement that child
- creates a draft PR and requests Copilot review
- waits for Copilot review on the latest PR head SHA
- runs one review-fix pass with the Copilot comments and updates the PR branch

Example:

```sh
nightloop start 221 --target canaria --dry-run
nightloop start 221 --target canaria
```

## Estimation Modes

The CLI supports four estimation modes:

- `template`
- `local`
- `hybrid`
- `ai`

Child Issue metadata may still declare `manual` as the recorded estimation basis for backward compatibility, but `estimate --basis` only accepts the four runtime modes above.

`hybrid` is the recommended default:

- template priors give the initial estimate
- local telemetry takes over once enough similar successful runs exist
- `ai` calls `agent.plan_command`, but remains advisory

When `--basis ai` is requested:

- `nightloop` still computes the baseline template/local/hybrid estimate
- the planning command reads [`prompts/estimate_issue.md`](/Users/semigrp/dev/nightloop/prompts/estimate_issue.md)
- the planning command is expected to return JSON only
- the CLI reports the AI estimate alongside the baseline
- scheduler logic still uses the baseline estimate

Prompt delivery contract:

- `nightloop` writes the prompt to `NIGHTLOOP_PROMPT_FILE`
- `nightloop` also sends the same prompt on stdin to `agent.command` and `plan_command`
- stdin-oriented CLIs such as `codex exec` work without shell wrappers

Review loop config:

```toml
[review_loop]
review_poll_interval_seconds = 120
review_wait_timeout_minutes = 90
review_max_fix_rounds = 1
planner_prompt_prefix = "/plan"
auto_split_stages = true
max_split_stages = 4
```

For Codex, keep `plan_command` as a normal executable such as `codex exec --full-auto`. `nightloop` injects `/plan` into the planner prompt automatically. Do not set `plan_command = "codex exec /plan"`.

If the planner decides that a child Issue should be split, `start` stores the staged plan in the target repo under `.nightloop/splits/` and opens only one stage PR at a time. A later `start` run advances to the next stage only after the active stage PR is merged. `nightly` never auto-splits into multiple PRs; it reports `split_required` and leaves that child for `start`.

## Run Behavior

### Dry Run

`nightloop nightly --dry-run`:

- fetches the parent Issue
- parses child Issue numbers
- fetches each child Issue with `gh issue view`
- lints and estimates each child
- applies eligibility rules
- packs the nightly window
- prints selected and skipped children plus reasons

Dry-run makes no GitHub writes and no git changes.

Before planning, dry-run also checks that the target repo exists, is a git worktree, and that its `origin` remote matches `github.owner/repo` when `origin` is configured. A mismatch fails with `target_repo_mismatch`. If no `origin` exists, the run continues with `target_repo_match=unknown`.

Dry-run also reports repairs that would be applied during a real run:

- missing managed labels as `label=<name> status=would_create`
- stale `agent:running` / `agent:blocked` child state as `repair=would_*`
- stale managed branches as `repair=would_deleted_stale_branch`

### Real Run

`nightloop nightly`:

1. requires valid `gh` auth and a clean worktree
2. fetches and prepares the campaign exactly as dry-run does
3. creates `<loop.run_root>/<timestamp>-parent-<id>/child-<id>/`
4. snapshots issue metadata and writes an agent prompt file
5. creates a stacked branch for the child
6. moves the child Issue from `agent:ready` to `agent:running`
7. executes `agent.command`
8. runs the parsed verification commands locally
9. measures changed lines with `git diff --numstat <base_sha> HEAD`
10. on success:
   - commits changes
   - creates a draft PR
   - optionally requests review from `github-copilot[bot]`
   - removes `agent:running`
   - adds `agent:review`
   - comments a concise success summary on the child Issue
   - appends telemetry
11. on failure:
   - removes `agent:running`
   - adds `agent:blocked`
   - comments a concise failure summary on the child Issue
   - appends telemetry
   - stops the campaign when `stop_on_failure = true`
12. comments a concise summary on the parent Issue

`agent_command_failed` can still happen for tool-specific failures, but prompt transport should no longer be the cause for stdin-based CLIs.

Guardrails:

- real runs auto-create missing managed labels
- selected children with stale `agent:running` or `agent:blocked` are normalized back to `agent:ready` before execution
- stale managed local branches named `nightloop/<parent>-<child>` are deleted and recreated automatically
- setup failures such as dirty worktrees or prompt write failures do not mark the child `agent:blocked`
- if a pre-execution setup step fails after labels changed, `nightloop` removes accidental `agent:running` state, restores `agent:ready`, and removes the created managed branch when possible

### Stacked PR Strategy

The first successful child branch is created from `github.base_branch`.

Each later successful child branch in the same run is created from the current HEAD produced by the previous successful child. Draft PR bases follow the same chain:

- first PR base = configured base branch
- later PR base = previous successful child branch

This is the v0 reviewability strategy. `nightloop` does not implement merge queues or multi-agent orchestration.

### Optional Copilot Review Request

When enabled in `nightloop.toml`:

- `github.request_copilot_review = true`
- `github.copilot_reviewer = "github-copilot[bot]"`

`nightloop nightly` requests review immediately after each successful draft PR is created. `nightloop start` does the same for its single-child workflow.

Behavior:

- success is reported as `copilot_review=requested`
- if the request fails, the PR still succeeds and the run continues
- failure is reported as `copilot_review=failed` and `copilot_review_request_failed`

This review request is GitHub-specific and intentionally optional.

## Diff Budget Enforcement

Two checks are enforced separately:

- the global diff budget from `[diff]`
- the child Issue target size band

If the implemented diff exceeds either maximum, the Issue is blocked.

If the diff falls below either minimum, it is accepted only when the child Issue uses the explicit `docs-only` or `config-only` scope marker described above.

## Telemetry

`record-run` appends JSON objects to the configured JSONL history path. The same schema is used by the real runner.

Relative telemetry and run-artifact paths are resolved from `agent.working_directory`, so control-repo operation still keeps execution state with the target repo.

Each record includes:

- `run_id`
- `parent_issue`
- `issue_number`
- `issue_title`
- `model_profile`
- `model`
- `reasoning_effort`
- `target_size`
- `docs_impact`
- `estimated_minutes`
- `actual_minutes`
- `changed_lines`
- `files_touched`
- `success`
- `status`
- `branch`
- `pr_base`
- `pr_url`
- `recorded_at`

Local estimation uses successful historical records matched by model profile, target size, and documentation impact.

## Repository Layout

```text
src/
  lib.rs
  main.rs
  config.rs
  models.rs
  issue_parse.rs
  issue_lint.rs
  estimate.rs
  selection.rs
  diff_budget.rs
  github.rs
  git_ops.rs
  agent_exec.rs
  runner.rs
  telemetry.rs
  docs_support.rs
  reporting.rs
tests/
  cli.rs
  fixtures/
```

## Non-Goals

Still intentionally out of scope for v0:

- merge queues
- auto-merge
- background scheduling daemons
- cloud services
- web UI
- issue creation bots
- multi-agent orchestration
