# nightloop blueprint

This file is a compact product summary.
Canonical requirements, ontology, lifecycle semantics, and authoritative-state rules live in [docs/requirements.md](requirements.md).

## Product

`nightloop` is a Codex-first minimal parent/child issue runner.

Fixed shape:

- Parent Issue = campaign container with ordered child references
- Child Issue = executable work unit and future contract
- `start` = single-child deep-run policy
- `nightly` = ordered multi-child budget policy inside a selected 2–6 hour window

The core stays narrow and shells out to:

- `gh`
- `git`
- configured planning and implementation commands

`start` and `nightly` are run policies, not ontology terms.

## Public Surface

Supported commands:

- `nightloop init`
- `nightloop check`
- `nightloop lint`
- `nightloop estimate --basis template|local|hybrid`
- `nightloop start`
- `nightloop nightly`

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
- validates runtime and authoring control assets from one shared manifest
- guarantees managed labels exist on GitHub
- safely creates any missing managed labels

Current `Source of truth` support is local-only:

- repo-relative paths
- absolute local paths
- remote URLs are rejected during lint

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

Oversize diffs always resolve to `split_required` for the current child only.

## Verification

Repo verification for this blueprint:

```sh
cargo fmt --check
cargo test
```
