# nightloop blueprint

## Product

`nightloop` is a Codex-first minimal parent/child issue runner.

Fixed shape:

- Parent Issue = campaign container with ordered child references
- Child Issue = executable work unit
- `start` = single-child execution path
- `nightly` = ordered multi-child execution path inside a selected 2–6 hour window

The core stays provider-agnostic and shells out to:

- `gh`
- `git`
- configured planning and implementation commands

## Public Surface

Supported commands:

- `nightloop init`
- `nightloop check`
- `nightloop lint`
- `nightloop estimate --basis template|local|hybrid`
- `nightloop start`
- `nightloop nightly`

Not part of the product anymore:

- `setup-labels`
- `budget`
- `record-run`
- legacy aliases
- Copilot review loop
- staged split state/docs

## Contracts

Child Issues must contain:

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

Optional:

- `Implementation constraints`
- `Suggested model override`

`Verification` is executable only from fenced shell blocks or `cmd:` lines.

`[docs].required_paths` defaults to:

```toml
required_paths = ["README.md", "AGENTS.md"]
```

## Execution Model

`check`:

- validates required target paths
- validates required control prompt/template files
- guarantees managed labels exist on GitHub

`start`:

- selects the first runnable child
- runs `plan_command`
- runs `agent.command`
- runs parsed verification
- enforces diff budget
- creates a draft PR

`nightly`:

- preserves parent order
- preserves dependency order
- packs the nightly budget with issue-specific estimates
- creates stacked child branches and draft PRs

Retained hardening:

- nightly branches are pushed before PR creation
- later nightly PRs base on the previous successful child branch
- safe stale local branch replacement is allowed when a managed branch name must be recreated

Removed safety/repair layers:

- Copilot request/poll/fix loops
- split-state persistence and advancement
- split docs
- broad issue-label repair loops

Oversize diffs always resolve to `split_required` for the current child only.

## Verification

Repo verification for this blueprint:

```sh
cargo fmt --check
cargo test
```
