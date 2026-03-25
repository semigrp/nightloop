# nightloop blueprint

## 1. Product Definition

`nightloop` is a minimal Rust CLI for local, issue-first nightly execution of coding agents.

The product shape is fixed:

- Parent Issue = campaign / plan container
- Child Issue = executable work unit
- Night run = execute as many runnable child Issues as fit in a selected 2–6 hour budget
- Reviewability comes before throughput
- The core stays generic and provider-agnostic

The runner shells out to:

- `gh` for GitHub reads and writes
- `git` for branch and diff operations
- configured shell commands for agent execution and optional AI estimation

An optional GitHub-specific post-PR hook may request review from `github-copilot[bot]` after draft PR creation.

`nightloop` may run from a separate control checkout. In that mode:

- the config file location is the control root
- bundled `prompts/` and `docs/templates/` are read from the control root
- `agent.working_directory` is the canonical target repo root for local validation, git operations, agent execution, telemetry, and run artifacts

## 2. Public v0 Surface

Supported commands:

- `nightloop init canaria UTAGEDA/canaria /abs/path/to/repo`
- `nightloop check`
- `nightloop lint path/to/issue.md`
- `nightloop estimate path/to/issue.md --basis template|local|hybrid|ai`
- `nightloop start 221`
- `nightloop nightly 221 --hours 4`
- `nightloop setup-labels`
- `nightloop budget --hours 2|3|4|5|6`
- `nightloop record-run path/to/run-record.json`

Compatibility aliases remain supported:

- `init-target`
- `docs-check`
- `lint-issue`
- `estimate-issue`
- `review-loop`
- `run`

Final command output is compact `key=value` on `stdout`. Real `start` and `nightly` runs emit live phase progress on `stderr`.

Named target resolution order is:

1. explicit `--config PATH`
2. `--target NAME` -> `targets/NAME.toml`
3. fallback `./nightloop.toml`

`setup-labels` bootstraps the managed workflow labels from `[labels]` for the selected target repo and is safe to rerun, but real `start` and `nightly` commands also auto-create missing managed labels.

`start` and `nightly` reporting may include:

- `target_repo_root=...`
- `run_root=...`
- `target_repo_match=true|false|unknown`

`start` is the Codex-first single-child workflow:

- pick the first runnable child from the parent Issue
- create a plan with `plan_command`
- implement with `agent.command`
- request Copilot review on the draft PR
- wait for that review and run one fix pass

For Codex, `plan_command` remains a normal execution command. The planner prompt is prefixed with `/plan` automatically via `review_loop.planner_prompt_prefix`.

When a child Issue is too large for one reviewable PR, `start` may persist a staged split plan under `.nightloop/splits/` in the target repo and advance one stage PR per run. `nightly` does not emit staged PRs; it reports `split_required` instead.

`init` writes `targets/<name>.toml` from the shipped example template and fills:

- `github.owner`
- `github.repo`
- `github.base_branch`
- `agent.working_directory`

## 3. Issue Contracts

### Parent Issue

The runner parses `## Ordered child Issues` and preserves the listed order.

Supported lines:

- `- [ ] #222 first child`
- `- [x] #223 already done`
- `- [ ] #224 depends on #223`

The checkbox state is tracking-only. GitHub state and labels are the execution truth.

### Child Issue

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

The parser accepts both `##` and `###` headings so local markdown snapshots and GitHub Issue form bodies share one implementation path.

## 4. Parsing Conventions

### Source of truth

One reference per non-empty line. Allowed forms:

- repo-relative paths
- absolute local paths
- `http://` or `https://` URLs

Local paths are validated by `lint`. URLs are syntax-only.

Repo-relative paths are resolved against the target repo root, not the process `cwd`.

### Verification

Only two formats are executable:

1. fenced `sh` / `bash` / `shell` blocks
2. `cmd:` lines

Any other prose is ignored. A non-empty verification section with zero parseable commands is invalid.

### Small diff exceptions

Below-minimum diffs are accepted only when `Scope` contains an exact line:

- `docs-only`
- `config-only`

This is explicit on purpose. The runner does not infer those cases from prose.

## 5. Eligibility And Scheduling

The fallback `budget` command still reports slot count from:

```text
floor((hours * 60 - fixed_overhead_minutes) / fallback_cycle_minutes)
```

The real scheduler uses issue-specific estimates instead:

1. reserve fixed overhead
2. preserve parent order
3. preserve dependency order
4. pack child Issues until the nightly budget is exhausted

A child Issue is eligible only if:

- it is open
- it has `night-run`
- it has `agent:ready`
- it does not already have `agent:running`, `agent:blocked`, `agent:done`, or `agent:review`
- dependencies are already closed/done on GitHub or planned/completed earlier in the same run
- metadata is valid
- the target size band fits within the configured global diff limits

The workflow labels are not assumed to preexist in a fresh repo. Real `start` and `nightly` runs auto-create any missing managed labels. `setup-labels` remains available as an explicit bootstrap command.

## 6. Estimation Policy

Supported runtime modes:

- `template`
- `local`
- `hybrid`
- `ai`

Recorded child metadata may still use `manual` for backward compatibility, but the CLI runtime basis is limited to the four modes above.

Recommended default:

- `hybrid`

Behavior:

- template uses target size, docs impact, dependency count, and profile runtime multiplier
- local uses successful telemetry matched by model profile, target size, and docs impact
- hybrid combines template and local using configured weights
- ai invokes `agent.plan_command`, reads `prompts/estimate_issue.md`, expects JSON, and reports the AI result alongside the baseline
- prompts are delivered via stdin and also exposed via `NIGHTLOOP_PROMPT_FILE`

AI remains advisory. Scheduler logic continues using the baseline estimate.

## 7. Execution Model

### Dry run

Dry-run fetches Issues, parses them, lints them, computes estimates, applies eligibility rules, packs the nightly window, and prints selected or skipped children with reasons.

Dry-run makes no GitHub writes and no git changes.

Preflight also checks:

- the target repo exists
- the target repo is a git worktree
- if `origin` exists, it matches `github.owner/repo`

A remote mismatch is a hard failure with `target_repo_mismatch`. Missing `origin` is allowed and reported as `target_repo_match=unknown`.

Dry-run also reports repair actions that a real run would apply:

- missing managed labels
- stale `agent:running` or `agent:blocked` child state
- stale managed local branches for selected children

### Real run

Real execution:

1. checks `gh` auth
2. checks for a clean worktree
3. prepares the same campaign plan as dry-run
4. creates `<loop.run_root>/<timestamp>-parent-<id>/child-<id>/`
5. snapshots issue metadata and writes the prompt file
6. creates the child branch
7. adds `agent:running`
8. executes `agent.command`
9. runs parsed verification commands locally
10. measures diff lines with `git diff --numstat <base_sha> HEAD`
11. on success:
   - commit
   - create draft PR
   - optionally request Copilot review
   - remove `agent:running`
   - add `agent:review`
   - comment on the child Issue
   - append telemetry
12. on failure:
   - remove `agent:running`
   - add `agent:blocked`
   - comment on the child Issue
   - append telemetry
   - stop when `stop_on_failure = true`
13. comment on the parent Issue with selected, completed, blocked, skipped, PR chain, and estimated vs actual minutes

Prompt transport is dual-path by default:

- stdin carries the full prompt text
- `NIGHTLOOP_PROMPT_FILE` points at the written prompt file for tools that prefer file reads

Failure guardrails:

- the clean-worktree check ignores files under the resolved `loop.run_root`
- real runs auto-create missing managed labels
- selected children with stale `agent:running` or `agent:blocked` are normalized back to `agent:ready`
- stale managed local branches named `nightloop/<parent>-<child>` are deleted and recreated automatically
- setup-only failures restore accidental `agent:running` state, clean up the created managed branch when possible, and do not mark the child `agent:blocked`

## 8. Stacked PR Strategy

Stacking is the v0 reviewability mechanism:

- first successful child branch starts from `github.base_branch`
- each later successful child branch starts from the current HEAD produced by the previous successful child
- first draft PR base = configured base branch
- later draft PR base = previous successful child branch

This supports dependency chains without introducing a merge queue or orchestration system.

## 8.1 Optional Copilot Review Request

If enabled by config, the runner requests review from `github-copilot[bot]` immediately after each successful draft PR is created.

Policy:

- disabled by default
- failure to request review is non-blocking
- the child remains successful and stays in `agent:review`
- the warning is surfaced in machine-readable output and summary comments

## 9. Diff Budget Policy

Two checks are enforced separately:

- global `[diff]` limits
- child target size band limits

If the diff exceeds either maximum, the Issue is blocked.

If the diff falls below either minimum, it is accepted only with the explicit `docs-only` or `config-only` scope marker.

## 10. Telemetry

Telemetry is JSONL, not a database.

Relative telemetry paths and the default `loop.run_root = ".nightloop/runs"` are both resolved from the target repo root.

Each line records:

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

## 11. Non-Goals

Still out of scope for v0:

- merge queues
- auto-merge
- background daemons
- cloud services
- web UI
- issue creation bots
- multi-agent orchestration
