# nightloop blueprint

## 1. Situation summary

The product should solve one operational problem well:

- run a local autonomous nightly loop
- select a time budget between 2 and 6 hours
- use parent Issue -> child Issue campaigns
- keep PRs reviewable
- keep the system generic enough to publish on GitHub and reuse across repos
- support repo-adjacent documentation without turning into a documentation platform
- keep the implementation minimal and essential
- use Rust for the runner
- avoid a single fixed per-Issue runtime assumption

## 2. Product definition

`nightloop` is a Rust CLI that orchestrates local, issue-driven agent execution.

It treats GitHub Issues as the source of execution truth:

- **Parent Issue** = campaign / plan container
- **Child Issue** = runnable unit
- **Nightly run** = execute as many runnable child Issues as fit in the chosen time budget

The child Issue is the only implementation unit. The parent Issue is planning and tracking only.

## 3. Why this shape matches current practice

GitHub Spec Kit documents a sequence of creating a spec, then a technical implementation plan, then tasks. That maps directly to the proposed flow of `PRD -> spec -> plan -> child Issues`.

OpenAI’s Codex guidance recommends `AGENTS.md` as durable repository guidance and explains that Codex reads `AGENTS.md` files before doing any work, layering global and project-specific guidance on each run. citeturn311860view3

Codex also supports explicit model selection. The CLI and IDE share `config.toml`, where you can set a default model; you can also start a new local thread with a specific model using `codex -m ...` or switch models interactively with `/model`. citeturn311860view0turn311860view2

OpenAI currently recommends `gpt-5.4` as the default model for most coding work, and the Codex docs position `gpt-5.4-mini` as a faster option for lighter coding tasks and subagents. citeturn311860view4turn273991view1turn397359view1

Codex configuration also exposes `model_reasoning_effort`, so model choice in this product should be represented as a combination of model name and reasoning effort rather than only a single string. citeturn273991view2

Codex writes local logs under a configurable log directory, and the docs explicitly mention `~/.codex/log/codex-tui.log` plus optional `session-*.jsonl` session logging for auditing. That makes local empirical timing feasible without adding a heavy database to v0. citeturn974336search0turn397359view0

GitHub supports YAML-based Issue forms inside `.github/ISSUE_TEMPLATE`, which makes it possible to standardize parent and child Issue structure at input time.

Anthropic’s guidance on effective agents argues that the strongest implementations tend to use simple, composable patterns rather than complex frameworks. That supports keeping this product deliberately narrow.

## 4. Design principles

1. **Issue-first**
   All executable work is represented as child Issues.

2. **Campaign-based nightly loop**
   A parent Issue is a campaign. The runner works through its runnable child Issues in order.

3. **Reviewability over throughput**
   One child Issue should produce one reviewable PR or branch.

4. **Diff budgets are first-class**
   A task without a target diff size is not night-run-ready.

5. **Documentation is explicit**
   The runner checks docs and prompts for doc updates, but does not try to become a CMS.

6. **Provider-agnostic core**
   Codex is a strong default, but the Rust engine should shell out to a configured agent command.

7. **Minimal mechanism**
   No multi-agent orchestration, autonomous merge, or PM framework in v0.

8. **Estimate, then schedule**
   The nightly scheduler should pack issue-specific estimated minutes, not assume one constant cycle time forever.

## 5. Required document set

For a generic open-source release, the minimum useful document set is:

- `README.md` — project definition and quickstart
- `AGENTS.md` — durable repo guidance for agents
- `docs/templates/prd.md`
- `docs/templates/spec.md`
- `docs/templates/plan.md`
- `docs/templates/eval.md`
- optional `docs/templates/adr.md`
- GitHub Issue forms for parent and child Issues

This matches the observed `spec -> plan -> tasks` flow and adds just enough structure for documentation and review.

## 6. Issue model

### Parent Issue fields

- Background
- Goal
- Source of truth
- Campaign policy
- Ordered child Issue list
- Completion condition

### Child Issue fields

- Background
- Goal
- Scope
- Out of scope
- Source of truth
- Constraints
- Acceptance criteria
- Verification
- Dependencies
- Target change size
- Documentation impact
- Suggested model profile
- Optional exact model override
- Estimated execution time
- Estimation basis
- Estimation confidence

## 7. Labels

Minimal state labels:

- `campaign`
- `night-run`
- `agent:ready`
- `agent:running`
- `agent:review`
- `agent:blocked`
- `agent:done`

Optional priority labels:

- `P0`
- `P1`
- `P2`

## 8. Time budgeting

The runner should accept `--hours 2..6`.

However, the selected hours should not map only to a fixed number of slots. Instead:

1. estimate minutes per child Issue
2. reserve a fixed startup / cleanup overhead
3. pack child Issues in dependency order until the remaining minutes are exhausted

A fallback fixed-slot formula is still useful before enough telemetry exists:

```text
slots = floor((hours * 60 - fixed_overhead_minutes) / fallback_cycle_minutes)
```

Default fallback config:

- `fallback_cycle_minutes = 40`
- `fixed_overhead_minutes = 20`

But once there are enough local samples, scheduling should use summed estimated minutes.

## 9. Estimation policy

The product should support three estimation sources.

### 9.1 Template prior

Use a simple heuristic from:

- target size band
- docs impact
- dependency count
- model profile runtime multiplier

Suggested initial priors:

- `XS` -> 35 minutes
- `S` -> 50 minutes
- `M` -> 80 minutes
- `L` -> 120 minutes

### 9.2 Local empirical history

Store local run results as JSONL records with at least:

- issue number
- model profile
- exact model
- reasoning effort
- target size
- docs impact
- estimated minutes
- actual minutes
- changed lines
- files touched
- success / failure
- recorded time

Use local history once there are at least a few successful comparable samples.

### 9.3 AI assist

Optionally ask the configured planning agent to estimate:

- the appropriate model profile
- a rough execution time
- split-required risk

This should remain advisory. The product should display it beside the heuristic / telemetry estimate, not replace them blindly.

### 9.4 Recommended default

Use **hybrid** by default:

- template early
- local history when sample size is sufficient
- AI estimate as a second opinion when desired

## 10. Model profiles

The core should represent model selection as a profile, not just a literal model string.

Recommended starting profiles:

- `fast` -> `gpt-5.4-mini`, medium reasoning effort, lighter tasks
- `balanced` -> `gpt-5.4`, medium reasoning effort, default tasks
- `deep` -> `gpt-5.4`, high reasoning effort, architecture-sensitive or larger tasks

This is aligned with current Codex guidance that recommends `gpt-5.4` for most tasks while positioning `gpt-5.4-mini` for lighter coding tasks and subagents. citeturn311860view4turn273991view1turn397359view1

## 11. Diff budget control

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

## 12. Documentation support model

The user wanted documentation support, but also insisted on simplicity. The correct interpretation is:

- provide templates
- require source-of-truth references
- require documentation impact declaration on child Issues
- optionally ask the agent to update docs when `docs impact != none`
- check required docs paths exist

Do not add:

- automatic documentation site generation
- knowledge graph indexing
- docs-specific workflow engines

## 13. PRD/spec creation support

The user wants AI to interrogate the human during PRD and spec creation.

The clean implementation is not to bake planning logic into Rust. Instead:

- ship prompt templates such as `prompts/refine_prd.md` and `prompts/refine_spec.md`
- optionally let `nightloop` print or invoke those prompts through the configured agent command
- keep the engine focused on execution, linting, estimation, and verification

That keeps the core small while still making documentation support real.

## 14. Minimal command set

### `nightloop budget --hours N`

Calculates fallback slot count.

### `nightloop lint-issue path/to/issue.md`

Checks whether an Issue snapshot has the required sections.

### `nightloop estimate-issue path/to/issue.md --basis hybrid`

Returns the recommended model profile and estimated local runtime.

### `nightloop record-run path/to/run-record.json`

Appends one completed run to local telemetry history.

### `nightloop docs-check`

Checks required documentation paths.

### `nightloop run --parent 221 --hours 4`

Runs the campaign in dependency order.

## 15. Execution algorithm

1. Load config.
2. Resolve the parent Issue.
3. Read child Issue numbers and order.
4. Filter children by labels: `night-run` and `agent:ready`.
5. Remove children whose dependencies are not done.
6. Estimate each remaining child Issue using the configured basis.
7. Pack children into the selected nightly window by estimated minutes.
8. For each admitted child until the budget is exhausted:
   - mark `agent:running`
   - snapshot the Issue and linked docs
   - invoke the configured agent command with the selected model profile
   - run verification
   - compute changed-line count
   - if success, create branch / PR and mark `agent:review`
   - if failure, mark `agent:blocked` and stop by default
   - record actual runtime and outcome into telemetry
9. Post a summary comment on the parent Issue.

## 16. Repo structure recommendation

```text
src/
  main.rs          # CLI entry
  config.rs        # TOML config
  budget.rs        # fallback slot calculation + estimate packing
  estimate.rs      # model selection + runtime estimation
  telemetry.rs     # JSONL run history
  issue_lint.rs    # issue structure validation
  selection.rs     # campaign selection logic
  docs_support.rs  # documentation checks
  github.rs        # later: GitHub API client
  runner.rs        # later: campaign execution
  diff_budget.rs   # later: git diff accounting
```

## 17. What to publish first

Publish v0 with:

- this blueprint
- config schema
- issue forms
- PR template
- docs templates
- budget command
- issue lint command
- estimate command
- telemetry append command
- docs check command
- a scaffolded `run` command

That is already useful for Canaria and also general enough for public release.
