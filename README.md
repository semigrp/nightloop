# nightloop

`nightloop` is a minimal, issue-first nightly execution runner for coding agents.

It is designed for one job: take a parent GitHub Issue that represents a campaign, select runnable child Issues in dependency order, implement them one by one with an agent, verify the result locally, and leave draft PRs or local branches that a human can review in the morning.

The design goal is deliberately narrow:

- issue-first, not framework-first
- simple, composable, local execution
- reviewable PRs, not giant overnight diffs
- generic enough for any repo, but strong enough for product teams
- compatible with Codex-first workflows, without hard-coding one vendor into the core design

## Core model

- **Parent Issue** = campaign / plan unit
- **Child Issue** = implementation unit
- **Night run** = process as many runnable child Issues as fit in the selected time budget

`nightloop` does **not** try to invent work from scratch. It expects humans and agents together to produce:

1. PRD
2. spec
3. implementation plan
4. child Issues
5. eval / verification notes

Then `nightloop` executes the queue.

## Why this shape

The current agentic coding workflow has converged on a rough sequence of `spec -> plan -> tasks`, with reusable agent guidance such as `AGENTS.md` and skills layered on top. GitHub Spec Kit explicitly models `spec`, then a technical implementation plan, then tasks. Codex guidance recommends durable repository guidance in `AGENTS.md`, and OpenAI skills package instructions, resources, and optional scripts for repeatable workflows. GitHub also supports structured Issue forms, which makes Issue quality enforceable at input time. See the cited references in `docs/blueprint.md`.

## What is intentionally out of scope

- automatic merge to `main`
- multi-agent parallel orchestration
- autonomous roadmap generation
- deep PM features
- vendor-specific planning logic in the Rust core

Those can be added later, but are excluded from v0 on purpose.

## Repository layout

```text
nightloop/
  Cargo.toml
  README.md
  AGENTS.md
  nightloop.example.toml
  src/
    main.rs
    config.rs
    budget.rs
    estimate.rs
    telemetry.rs
    issue_lint.rs
    models.rs
    selection.rs
    docs_support.rs
    errors.rs
  docs/
    blueprint.md
    templates/
      prd.md
      spec.md
      plan.md
      eval.md
      adr.md
  prompts/
    refine_prd.md
    refine_spec.md
    child_issue_from_plan.md
    estimate_issue.md
  .github/
    ISSUE_TEMPLATE/
      campaign-parent.yml
      child-task.yml
    pull_request_template.md
```

## First release scope

### Required CLI commands

- `nightloop budget --hours 2|3|4|5|6`
- `nightloop lint-issue path/to/issue.md`
- `nightloop estimate-issue path/to/issue.md --basis hybrid`
- `nightloop record-run path/to/run-record.json`
- `nightloop docs-check`
- `nightloop run --parent 221 --hours 4`

### Required runtime behavior

- select runnable child Issues from the parent campaign
- enforce dependency order
- respect line-change budgets
- stop on failure by default
- update issue state labels
- create one branch / one PR per child Issue
- leave a morning summary on the parent Issue
- estimate each child Issue before scheduling it into the nightly window

## Time budgeting

The runner still supports a fallback fixed-slot model, but the main scheduler should use **issue-specific estimated minutes**, not a single global cycle length.

Default fallback model:

- issue implementation cycle: `40` minutes
- fixed startup/cleanup overhead: `20` minutes

That remains useful when no estimates exist yet. Once local telemetry exists, scheduling should pack Issues by summed estimated minutes.

## Estimation model

At Issue creation time, each child Issue should carry:

- suggested model profile
- optional exact model override
- estimated execution time in minutes
- estimation basis (`template`, `local`, `hybrid`, `ai`, or `manual`)
- optional confidence note

`nightloop` should support two estimation inputs and one optional third input:

1. **Template prior** — fast heuristic from target size, docs impact, and dependency count.
2. **Local telemetry** — empirical history from previous local runs, stored in JSONL.
3. **AI assist** — optional rough estimate generated from the Issue body and source docs using the configured planning agent.

The recommended default is **hybrid**:

- use template priors early in a new repo
- prefer local telemetry once there are enough comparable samples
- optionally show an AI estimate beside both numbers, but do not make it the sole source of truth

## Model selection

Codex lets you set a default model in `config.toml`, override it per run, and switch models interactively. The public docs currently recommend `gpt-5.4` for most Codex tasks, while `gpt-5.4-mini` is positioned as a faster option for lighter coding tasks and subagents. `nightloop` should therefore treat **model profiles** as first-class configuration and keep the exact model name override optional in each Issue. See `docs/blueprint.md` for citations.

A practical default profile set is:

- `fast` -> `gpt-5.4-mini`
- `balanced` -> `gpt-5.4`
- `deep` -> `gpt-5.4` with higher reasoning effort

## Diff budget control

The product should let users constrain review size between 50 and 1000 changed lines.

Recommended bands:

- `XS` = 50–120
- `S` = 120–250
- `M` = 250–500
- `L` = 500–1000

Policy:

- If an Issue is estimated above `L`, it must be split before a night run.
- If an implemented diff exceeds configured max lines, the runner marks the Issue blocked or split-required.
- If an implemented diff falls below min lines, it is accepted only if the Issue explicitly declares doc-only or config-only scope.

## Configuration

See `nightloop.example.toml`.

Key additions in v0.2 design:

- model profiles with exact Codex models and reasoning effort
- telemetry history path
- heuristic estimation weights
- min/max nightly window hours

## Notes

This repository is intentionally text-first. In this environment, Rust toolchain verification was not available, so the included Rust files are a starter scaffold rather than a compiled build.
