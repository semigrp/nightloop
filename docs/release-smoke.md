# Release smoke

Use a disposable GitHub sandbox repo for the final public v0 smoke pass. This flow is manual on purpose and covers the shipped surface only: `init`, `check`, `start`, and `nightly --dry-run`.

## Sandbox setup

Create an empty sandbox repo on GitHub, clone it locally, and make one seed commit with `README.md` and `AGENTS.md` present in the target repo.

From the `nightloop` control repo, generate a target config:

```sh
nightloop init \
  sandbox \
  YOUR_ORG/nightloop-sandbox \
  /absolute/path/to/nightloop-sandbox \
  --agent-command "codex exec --full-auto" \
  --plan-command "codex exec --full-auto"
```

Expected stdout fields:

- `ok=true`
- `target=sandbox`
- `config_path=.../targets/sandbox.toml`

Run the bootstrap check:

```sh
nightloop check --target sandbox
```

Expected stdout fields:

- `ok=true`
- `missing_count=0`
- `labels_created=...`
- `labels_existing=...`

`check` may safely create missing managed labels on the sandbox repo.

## Manual issue setup

Create one parent issue and at least two child issues in the sandbox repo.

Parent issue:

- add an ordered child checklist under `## Ordered child Issues`
- reference the child issue numbers in execution order

Each child issue must include the required sections:

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

Label each runnable child with:

- `night-run`
- `agent:ready`

Leave the second child dependent on the first if you want the dry run to show one selected child and one skipped child.

## Smoke flow

Run single-child execution first:

```sh
nightloop start PARENT_ISSUE --target sandbox
```

Expected stdout fields:

- top-level line with `ok=true` and `dry_run=false`
- child line with `child_issue=...`
- child line with `status=success`
- child line with `pr_url=https://...`

Then run a nightly dry run:

```sh
nightloop nightly PARENT_ISSUE --target sandbox --hours 4 --dry-run
```

Expected stdout fields:

- top-level line with `ok=true` and `dry_run=true`
- one line per child with `child_issue=...`
- child status values such as `status=selected` or `status=skipped`
- `reasons=...` on skipped children when they are not eligible

The smoke pass is complete when `check` succeeds, `start` creates a draft PR for the first runnable child, and `nightly --dry-run` reports a sensible selection/skipping result for the remaining ordered child list.
