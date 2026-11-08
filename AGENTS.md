# Agent guide (EasyTier)

Short router for coding agents. **Do not duplicate long specs here** — follow the linked sources of truth.

## Read first

1. [`docs/README.md`](docs/README.md) — doc layers (`current` / `roadmap` / `ops` / `archive`)
2. [`docs/current/system-overview.md`](docs/current/system-overview.md) — how the system fits together
3. Task-specific Current or binding Roadmap (ask the user if unclear)

**Brand:** do not put upstream/fork/source-project identity into `docs/current|ops|roadmap`, root READMEs, or product help i18n. Details: `docs/README.md`.

## Windows build / test (local)

| Need | Entry |
|------|--------|
| Daily lint gate (≈ CI check-\*) | `script/easytier-test.cmd fast` or `script/easytier-test-fast.cmd` |
| Broader local tests | `script/easytier-test.cmd full` (three_node needs Linux/WSL) |
| GUI + headless + web package | `script/easytier-windows-*.cmd` |
| Web console only | `script/easytier-web-*.cmd` |
| Script index | [`script/README.md`](script/README.md) |

- **MSVC:** `script/build-common.ps1` auto-loads `vcvars64` (PATH + INCLUDE + LIB). Do **not** permanently stuff only `Hostx64\x64` into user PATH. Details: [`docs/ops/windows-msvc-local-build.md`](docs/ops/windows-msvc-local-build.md).
- Packaging also uses short `CARGO_TARGET_DIR` under `.cache/cargo-target` and a writable `.cache/tmp` (D8050 / esbuild).
- GitHub **ET Test** = [`.github/workflows/test.yml`](.github/workflows/test.yml). Local Fast ≠ full CI (no `--features full` clippy / no three_node / no bench gate on Windows).

## Before commit / push / PR / ET Test

1. [`docs/ops/ci-failure-log.md`](docs/ops/ci-failure-log.md) — §1 self-check + §3 still-valid traps
2. If any `.rs` changed: `cargo fmt --all -- --check`, then `cargo metadata --format-version 1 --locked`
3. Prefer `script/easytier-test-fast.cmd` (or matching clippy from the log) before push
4. New CI/packaging failure → append to that log (§4 template → §5)

Cursor always-on rule: [`.cursor/rules/ci-failure-log-presubmit.mdc`](.cursor/rules/ci-failure-log-presubmit.mdc)

## Doc updates after a task

When the user says done / commit / PR and behavior changed:

1. [`docs/ops/doc-update-after-task.md`](docs/ops/doc-update-after-task.md) — decision table
2. **Strict Current gate:** unaccepted behavior → Roadmap/Ops only; **never** write unverified behavior into `docs/current/`
3. No binding doc → propose 1–2 candidates; do not scatter-edit Current

Cursor always-on rule: [`.cursor/rules/doc-update-after-task.mdc`](.cursor/rules/doc-update-after-task.mdc)

## Git / hygiene

- Commit only when the user asks; no force-push to main/master; no amending others’ commits
- Do **not** stage `.workbuddy/memory/` with product code (keep agent notes out of feature commits)
- Prefer splitting behavioral commits (`feat` / `fix`) from `docs(roadmap)` when releasing notes matter (e.g. breaking gateway changes)

## What not to invent

- Do not treat Roadmap / Archive as shipped Current behavior
- Do not skip fmt/lockfile “because CI will catch it”
- Do not claim local Windows Full ET Test equals GitHub ET Test
