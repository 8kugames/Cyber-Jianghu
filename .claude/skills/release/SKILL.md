---
name: release
description: >
  Execute the full release pipeline: commit staged changes, push dev, merge to main,
  push main, tag with server version, push tag, switch back to dev.
  Use when: user says 'release', 'ship it', 'deploy', 'publish', 'go live',
  '打版本', '发版', '上线', or explicitly asks to run the release workflow.
  Also trigger when user describes the full sequence like 'commit and push and merge and tag'.
---

# Release Pipeline

Fixed-sequence git workflow. No deviation.

## Version Alignment Invariant

The pre-commit hook (`githooks/pre-commit`) bumps the server patch version whenever a commit stages `crates/**` `.rs/.toml/.yaml` changes. To keep `tag v<VER>` == `Cargo.toml at the tagged commit` == `CHANGELOG [<VER>]`, the order below is mandatory: commit all code first, read the settled version, then a CHANGELOG-only release commit (no `crates/**` changes staged, so no bump), then tag.

## Prerequisites

Before starting, verify:

1. Working tree has staged or unstaged changes (or user explicitly says to proceed)
2. Current branch is `dev`
3. Server `Cargo.toml` has a valid semver version
4. `CHANGELOG.md` has a `## [Unreleased]` section with content

If working tree is clean and user didn't explicitly ask to release, stop and ask.

## Step 1: Commit all pending work

If there are staged or unstaged changes, stage the relevant files and commit them, using the project's commit message conventions:

- `fix(scope): description` for bugfixes
- `feat(scope): description` for new features
- `refactor(scope): description` for refactors
- `chore(scope): description` for maintenance

The pre-commit hook auto-bumps the server version on these commits — expected. All bumps must settle here. If the user already committed everything, skip this step.

## Step 2: Promote CHANGELOG [Unreleased]

Read the server version from `crates/server/Cargo.toml` (the `version = "X.Y.Z"` line). All hook bumps are settled at this point — this is the release version `<VER>`.

In `CHANGELOG.md`:

1. Rename `## [Unreleased]` to `## [<VER>] - <today's date YYYY-MM-DD>`
2. Insert a new empty `## [Unreleased]` line above it (for the next cycle)

Example result:

```
## [Unreleased]

## [0.1.264] - 2026-06-28

### Your feature notes...
```

## Step 3: Release commit (CHANGELOG only)

Stage `CHANGELOG.md` and verify it is the ONLY staged file. If any `crates/**` change is still pending, go back to Step 1 — committing it here would let the hook bump past `<VER>` and break the alignment invariant.

```bash
git commit -m "chore(release): v<VER>"
```

Only `CHANGELOG.md` is staged, so the hook does not trigger and the server version stays `<VER>`.

## Step 4: Push dev

```bash
git push origin dev
```

## Step 5: Merge to main

```bash
git checkout main && git merge dev --no-edit
```

## Step 6: Push main

```bash
git push origin main
```

## Step 7: Tag and push

Use the version `<VER>` read in Step 2. The alignment invariant guarantees `v<VER>` equals the Cargo.toml version at the tagged commit.

```bash
git tag v<VER> && git push origin v<VER>
```

## Step 8: Return to dev

```bash
git checkout dev
```

## Output

Report a summary table:

```
| Step           | Result                              |
|----------------|-------------------------------------|
| work commits   | <hashes/messages>                   |
| changelog      | [Unreleased] → [<VER>] - <date>     |
| release commit | <hash> chore(release): v<VER>       |
| push dev       | <range>                             |
| merge          | <hash> (N files, +A/-D)             |
| push main      | <range>                             |
| tag            | v<VER> (= Cargo.toml at tag)        |
| branch         | dev                                 |
```

## Error handling

- If any step fails, stop and report the error. Do not attempt subsequent steps.
- If tag already exists, report and ask user whether to skip or delete and re-tag.
- Never force push. If push is rejected, report and let user decide.
