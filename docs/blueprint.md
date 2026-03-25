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

## 2. Public v0 Surface

Supported commands:

- `nightloop budget --hours 2|3|4|5|6`
- `nightloop lint-issue path/to/issue.md`
- `nightloop estimate-issue path/to/issue.md --basis template|local|hybrid|ai`
- `nightloop record-run path/to/run-record.json`
- `nightloop docs-check`
- `nightloop run --parent 221 --hours 4 [--dry-run]`

All command output is compact `key=value`.

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

Local paths are validated by `lint-issue`. URLs are syntax-only.

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

AI remains advisory. Scheduler logic continues using the baseline estimate.

## 7. Execution Model

### Dry run

Dry-run fetches Issues, parses them, lints them, computes estimates, applies eligibility rules, packs the nightly window, and prints selected or skipped children with reasons.

Dry-run makes no GitHub writes and no git changes.

### Real run

Real execution:

1. checks `gh` auth
2. checks for a clean worktree
3. prepares the same campaign plan as dry-run
4. creates `.nightloop/runs/<timestamp>-parent-<id>/child-<id>/`
5. snapshots issue metadata and writes the prompt file
6. adds `agent:running`
7. creates the child branch
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
