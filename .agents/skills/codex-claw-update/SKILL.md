---
name: "codex-claw-update"
description: "Use when modifying codex-claw itself in this repository: verify repo scope, implement changes, run checks, then ask operator to trigger deployment."
---

# Codex Claw Update Skill

## Goal
Safely modify `codex-claw`, verify quality, and let the operator decide when to deploy.

## Repository Scope
This skill is repository-scoped and must only be used inside the repo that contains this file.

Before editing or building, verify scope:

1. `pwd`
2. `git rev-parse --show-toplevel`
3. Confirm `<repo>/.agents/skills/codex-claw-update/SKILL.md` exists.
4. Confirm `<repo>/Cargo.toml` contains package name `codex-claw`.

If any check fails, stop and report:
"当前仓库不是 codex-claw 仓库，拒绝执行更新流程。"

## Required Flow
1. Confirm requested change scope and impacted modules.
2. Implement minimal code changes in this repository.
3. Run `cargo fmt`.
4. Run `cargo test` (or explain why not possible).
5. Summarize behavior changes, risks, and verification evidence.
6. Ask operator to trigger deployment explicitly (`/self-update` or manual restart flow).

## Constraints
- Do not modify/build/deploy unrelated repositories.
- Do not self-deploy silently.
- Preserve runtime data under `~/.codex-claw/data`.
- Prefer small, reversible diffs.
