# nightloop requirements

This document is the canonical requirements source of truth for `nightloop`.
Other docs may summarize or operationalize these requirements, but they do not override this file.

## External product definition

`nightloop` is a Codex-first parent/child GitHub Issue runner.
It is a Rust CLI operated from a control repo against a separate target repo.

Its fixed product shape is intentionally narrow:

- parent issue = campaign container
- child issue = executable implementation unit and future contract
- `start` = single-issue deep run policy
- `nightly` = budgeted multi-run orchestration policy

`nightloop` is designed to work in GitHub-native and repo-native ways.
It should produce reviewable repository changes and reviewable GitHub state without requiring extra core infrastructure.

## Internal design principles

`nightloop` uses a small ontology so requirements, procedure, and execution evidence do not get mixed together.

- `Strategy`: higher-level direction for what kinds of campaigns should exist and why
- `Doctrine`: stable product rules and invariants that define what `nightloop` is
- `Routine`: reusable operating procedures, prompts, templates, and checklists
- `Intent`: planned change descriptions such as PRDs, specs, plans, ADRs, and issues
- `World`: the actual state of the target repo, Git history, and live GitHub state
- `Run`: one concrete execution attempt that reads intent and acts on the world

PRD, spec, plan, and issue text describe `Intent`, not `World`.
`start` and `nightly` are run policies applied to the same ontology; they are not ontology categories themselves.

## First-class objects

The main first-class objects are:

- control repo
- target repo
- parent issue
- child issue
- PRD / spec / plan / ADR documents
- prompts and templates
- GitHub labels and PRs
- run artifacts
- telemetry history

Their roles are intentionally different:

- the control repo defines how `nightloop` runs
- the target repo is the world being inspected and changed
- parent issues track campaigns and ordered execution intent
- child issues define executable implementation units
- prompts and templates shape routine behavior without redefining doctrine
- run artifacts and telemetry record what happened during a run

## Core invariants

The following are stable product requirements and should be treated as hard invariants:

- control repo and target repo stay conceptually separate
- parent issues and child issues are different objects with different roles
- child issues are future contracts, not just queue entries
- PRDs and specs describe intent, not the world itself
- `Strategy`, `Doctrine`, `Routine`, `Intent`, `World`, and `Run` remain separate concepts
- `start` and `nightly` are run policies, not ontology terms
- nightly execution does not automatically create staged splits
- GitHub-native and repo-native operation is preferred
- diff budget and issue lint are first-class gates, not optional polish
- rerun and resume resilience matter more than clever one-shot execution
- docs, templates, and routines should solve process problems before new product features do
- provider plugin systems, DB-backed cores, daemons, and complex UI cores are non-goals

## Current implementation assumptions

The following describe the current repo and workflow shape, but they are not core invariants and may change later without redefining the product:

- current workflow labels are `night-run`, `agent:ready`, `agent:running`, `agent:review`, `agent:blocked`, and `agent:done`
- current branch naming uses `nightloop/{parent}-{child}`
- current nightly publication stacks later child PRs on the previous successful child branch
- current publication hardening pushes a branch before creating the PR
- current `start` behavior runs a planning step before implementation, while current `nightly` behavior does not
- current run artifacts and telemetry use configured paths such as `.nightloop/runs` and `.nightloop/history.jsonl`; those are defaults, not permanent path contracts
- current child issue metadata allows `manual` as an issue-authored estimation basis, while the public `estimate --basis` CLI remains `template|local|hybrid`
- current `Source of truth` resolution supports repo-local and absolute local files only; remote URLs are rejected during lint
- exact PR bodies, parent summary comments, stdout pair formatting, and report field shapes are current reporting choices
- current stop-on-failure behavior and any fixed review or fix-round heuristics are operating choices, not doctrine

## Operating policy

Current operating policy is intentionally narrow:

- `start` is the default deep-run policy for a single runnable child issue
- `nightly` is the budgeted policy for multiple runnable child issues within a selected window
- parent order is preserved
- dependency order is preserved
- child issues must lint cleanly before they are eligible to run
- declared verification commands are part of the child issue contract
- diff budget is enforced on produced changes
- when a child exceeds diff budget, the correct outcome is to stop that child and require re-authoring into smaller future contracts rather than silently staging extra split state

These are operating policies over the same issue ontology, not separate product modes.

## Authoritative state

Authority is deliberately split so doctrine, intent, world state, and evidence are not confused.

- `Strategy` authority: the highest-level campaign-shaping intent captured in current planning docs and parent campaign framing; it guides what campaigns should exist but does not override doctrine or world state
- `Doctrine` authority: this file, `docs/requirements.md`
- `Routine` authority: prompts, templates, and operational docs such as plan/spec/eval templates and release smoke instructions
- `Intent` authority: linked PRD/spec/plan/ADR docs, parent issues, child issues, and each child issue's `Source of truth`
- `World` authority: actual target repo contents, Git history, and live GitHub issue/PR state
- GitHub workflow state authority: current GitHub issue state, PR state, and workflow labels
- split progression authority: child issues plus GitHub workflow state only; there is no separate authoritative split-state ledger
- run artifacts and telemetry: evidence and measurements about runs, not authoritative intent or authoritative world state

## Child issue lifecycle

The lifecycle below is conceptual. It should not be frozen to any one label vocabulary.

- `ready`: the child issue is well-formed, intended to run, and currently eligible
- `in_progress`: a run has claimed the issue or is actively executing against it
- `blocked`: execution cannot proceed because of dependency, verification, environment, repo-state, or workflow blockers
- `split_required`: the attempted change exceeded allowed scope and must be rewritten into smaller future contracts
- `completed`: the intended work is accepted as done
- `abandoned`: the intended work is no longer being pursued

Current GitHub implementation maps evidence onto these conceptual states:

- `ready` is currently evidenced by an open child issue that passes lint and eligibility checks and carries the runnable workflow labels
- `in_progress` is currently evidenced mainly by the running workflow label during an active run
- `blocked` is currently evidenced by the blocked workflow label or by run-stopping blockers such as unsatisfied dependencies, verification failure, or repo-state failure
- `split_required` is currently emitted as a run/report outcome for the current child attempt rather than as a dedicated managed label
- `completed` is currently evidenced by accepted GitHub state such as issue closure or done-state workflow labeling
- `abandoned` is currently represented by human GitHub workflow decisions such as closing or de-prioritizing the child rather than by a dedicated lifecycle label
- a draft PR may be evidence that execution succeeded and the child is waiting for human review, but reviewability is not itself the same thing as conceptual completion

These current labels and PR conventions are implementation evidence, not the ontology itself.

## Run outcome semantics

Each run should be interpretable with a small set of outcomes:

- `success`: the intended child completed in this run and produced a reviewable result
- `partial_success`: the run advanced the selected scope or campaign but did not fully complete all intended children
- `blocked`: the run stopped because a blocker prevented progress
- `aborted`: the run stopped intentionally before normal completion
- `retryable_failure`: the run failed in a way that can reasonably be retried without redefining intent
- `terminal_failure`: the run failed in a way that means the child intent itself must change before rerun

Current implementation/reporting terms such as `success`, `blocked`, `split_required`, `selected`, `skipped`, and top-level `ok=` output remain reporting vocabulary.
They should be interpreted through the semantics above rather than treated as the only possible ontology.

## Required capabilities

`nightloop` must be able to:

- operate from a control repo against a separate target repo
- validate required target docs and control-side routine files
- lint child issue contracts before execution
- estimate child runtime using the supported estimation surface
- select runnable children from an ordered parent campaign
- run a single child deeply via `start`
- run multiple children within a budget via `nightly`
- execute declared verification commands
- enforce diff budgets, including narrow below-minimum exceptions for intentionally docs-only or config-only tasks
- create reviewable Git branches and draft PRs through GitHub-native flows
- preserve human-reviewable outputs rather than bypassing review with hidden state

## Non-functional requirements

The product should remain:

- GitHub-native and repo-native
- small in public surface area
- reviewable by humans at the issue, diff, and PR levels
- resilient to rerun and resume
- biased toward explicit contracts and explicit evidence
- conservative about hidden coordination state
- narrow enough that workflow hardening usually lands in docs, templates, or routines before new runtime features

## Explicit non-goals

`nightloop` is not:

- a generic AI development platform
- a generic multi-agent platform
- a provider plugin system in disguise
- a DB-centered orchestration system
- a daemon-centered orchestration system
- a complex UI-centered orchestration system
- a system that auto-splits oversized nightly work into staged subruns
- a place to solve routine and documentation problems by defaulting to new runtime features

## Future direction

The following topics are intentionally not current requirements.
If they are revisited, they belong here rather than in core invariants unless and until they become stable doctrine:

- exact `PromotionCandidates` semantics
- the specific `niche operating system` phrasing
- detailed developmental-niche mappings
- treating an issue graph as curriculum
- concrete split-state file paths or ledgers
- strict long-term review loop procedures
- Copilot review integration as a product requirement
- exact fix-round counts or similar current heuristics
