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

## Prerequisites

Before starting, verify:
1. Working tree has staged or unstaged changes (or user explicitly says to proceed)
2. Current branch is `dev`
3. Server `Cargo.toml` has a valid semver version
4. `CHANGELOG.md` has a `## [Unreleased]` section with content

If working tree is clean and user didn't explicitly ask to release, stop and ask.

## Step 1: Promote CHANGELOG [Unreleased]

Read the server version from `crates/server/Cargo.toml` (the `version = "X.Y.Z"` line). This is the release version `<VER>`.

In `CHANGELOG.md`:
1. Rename `## [Unreleased]` to `## [<VER>] - <today's date YYYY-MM-DD>`
2. Insert a new empty `## [Unreleased]` line above it (for the next cycle)

Example result:
```
## [Unreleased]

## [0.1.264] - 2026-06-28

### Your feature notes...
```

Stage `CHANGELOG.md` so it lands in the release commit.

## Step 2: Commit

If there are unstaged changes, stage the relevant files and commit. Use the project's commit message conventions:
- `fix(scope): description` for bugfixes
- `feat(scope): description` for new features
- `refactor(scope): description` for refactors
- `chore(scope): description` for maintenance

The pre-commit hook auto-bumps the server version in `Cargo.toml`.

If user already committed, skip this step.

## Step 3: Push dev

```bash
git push origin dev
```

## Step 4: Merge to main

```bash
git checkout main && git merge dev --no-edit
```

## Step 5: Push main

```bash
git push origin main
```

## Step 6: Tag and push

Use the version `<VER>` read in Step 1.

```bash
git tag v<VER> && git push origin v<VER>
```

## Step 7: Return to dev

```bash
git checkout dev
```

## Output

Report a summary table:

```
| Step         | Result                              |
|--------------|-------------------------------------|
| changelog    | [Unreleased] → [<VER>] - <date>     |
| commit       | <hash> <message>                    |
| push dev     | <range>                             |
| merge        | <hash> (N files, +A/-D)             |
| push main    | <range>                             |
| tag          | v<VER>                              |
| branch       | dev                                 |
```

## Error handling

- If any step fails, stop and report the error. Do not attempt subsequent steps.
- If tag already exists, report and ask user whether to skip or delete and re-tag.
- Never force push. If push is rejected, report and let user decide.
