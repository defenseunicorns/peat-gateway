---
name: peat-gateway
description: Per-repo skill for the Peat enterprise control plane — Rust binary/library with feature-gated CDC sinks, IDAM federation, and KMS integrations, plus a Svelte admin UI, Helm chart, Zarf manifest, and UDS bundle.
when_to_use: Editing files under peat-gateway/, reviewing peat-gateway PRs, debugging CDC / OIDC / KMS / multi-tenancy / formations API issues, working on the admin UI, or working on the Helm chart / Zarf packaging / UDS bundle.
verifies_with: cargo fmt --check, cargo clippy -- -D warnings, cargo test (default features), per-feature test/build for any changed feature, ui pnpm build for UI changes, helm template for chart changes.
---

# `peat-gateway` SKILL

`peat-gateway` is the enterprise control plane for the Peat mesh. It provides multi-org tenancy, CDC (change-data-capture), IDAM federation (OIDC), envelope encryption (AWS KMS / Vault), and a formations API for managing peer / document / certificate state. The repo combines a Rust binary + library (`peat_gateway`), a Svelte admin UI in `ui/`, a Helm chart, a Zarf manifest, and a UDS bundle. CDC sinks (NATS, Kafka, webhook), identity providers (OIDC), storage backends (Postgres), and KMS integrations are all behind feature flags so deployments compile only what they need — and CI splits the test job by feature to avoid OOM on standard runners.

## When this skill applies

- Editing any file under `src/` (control plane, formations API, CDC dispatcher, OIDC, KMS, tenant manager)
- Editing `ui/` (Svelte admin UI)
- Editing `chart/peat-gateway/` (Helm chart) or `zarf.yaml` (Zarf packaging) or `bundle/uds-bundle.yaml` (UDS bundle)
- Touching the feature-flag matrix in `Cargo.toml`
- Working on a feature-gated sink (NATS, Kafka, webhook), identity provider (OIDC), storage backend (Postgres), or KMS integration (AWS KMS, Vault)
- Bumping the pinned `peat-mesh` version (currently `=0.9.0-rc.1`)

## Scope

**In scope:**
- Multi-org tenancy and tenant manager logic
- CDC watcher state machine and event dispatcher
- Sink implementations (behind feature flags)
- IDAM federation (OIDC, RFC 7662 token introspection)
- Envelope encryption integrations (AWS KMS, Vault)
- Formations API endpoints (peer, document, certificate)
- Admin UI in `ui/` (Svelte + Vite + pnpm)
- Helm chart, Zarf manifest, UDS bundle
- Per-feature CI configuration (the OOM-prevention split)

**Out of scope (route elsewhere):**
- Mesh transport / sync semantics → `peat-mesh/SKILL.md`
- BLE transport → `peat-btle/SKILL.md`
- OCI registry sync → `peat-registry/SKILL.md`
- Top-level shared types/traits — consider whether the change belongs in `peat/peat-protocol` or `peat/peat-schema`
- Production cluster operations / GitOps configs that consume this chart — separate ops repos

## Workflow

1. **Orient.** Read `peat/SKILL.md` (ecosystem) if accessible. Read this file. Read `CONTRIBUTING.md`. Skim relevant `docs/` (architecture, cdc, identity-federation, multi-tenancy, etc.) for the area you're touching. `git status`, `git log -10`.
2. **Locate the spec.** Confirm the task has a GitHub issue with Context / Scope / Acceptance / Constraints / Dependencies. If not, stop and ask the user.
3. **Plan.** Produce a 1–5 step plan. Cross-check against ecosystem hard invariants (transport agnosticism, dependency direction, async runtime is Tokio) and the scope guards below.
4. **Implement.** Branch from `main` per the trunk-based convention. Vertical slices, one concern per commit. Keep new sinks / providers / backends behind their own feature flags.
5. **Verify.** Run every command in the verification checklist below. Capture output.
6. **Hand off.** Open PR against `main` referencing the issue. Single concern per PR — squash-merge applies. **After opening the PR, self-review the full diff against the originating issue/plan and post findings as a PR comment** covering correctness gaps (missing error handling, edge cases, panics), security concerns (secrets in memory, missing zeroization, input validation), missing test coverage, and deviations from issue requirements. Fix actionable findings before declaring the PR ready — the comment serves as a review record.

## Verification (exit criteria)

A session in this repo is not done until each of these produces evidence:

- [ ] `cargo fmt --check` exits 0
- [ ] `cargo clippy -- -D warnings` exits 0 (run with the relevant feature combo for changed code)
- [ ] `cargo test` exits 0 (default features = `nats`, `webhook`, `oidc`)
- [ ] If the change touches code under any non-default feature: `cargo test --features <flag>` for each affected feature (`kafka`, `postgres`, `aws-kms`, `vault`); CI splits these to avoid OOM, run them locally one at a time
- [ ] If the change is broad enough to span features: `cargo build --features full` confirms the all-features build still links
- [ ] If `ui/` was touched: `cd ui && pnpm install && pnpm build` succeeds
- [ ] If `chart/` or `zarf.yaml` or `bundle/uds-bundle.yaml` was touched: `helm template chart/peat-gateway` renders cleanly; for Zarf/UDS bundle changes, run the corresponding packaging command
- [ ] If a feature-gated sink / provider / backend interaction changed: the feature's optional runtime dependency (NATS server, Kafka broker, Postgres, Vault) is exercised via the integration tests, not just the build

"Seems right" or "the diff looks correct" is never sufficient.

## Anti-rationalization

| Excuse | Rebuttal |
|---|---|
| "This change is too small to need a test." | If it's worth changing, it's worth one assertion. Add the test. |
| "I'll fix the clippy warning later." | The CI gate is `-D warnings`. There is no later. |
| "Default features cover most cases — I don't need to test the kafka/postgres/vault paths." | Feature flags exist because deployments differ. Each feature combination is a real production target. Run `cargo test --features <affected>`. |
| "I'll add the new sink straight to the CDC dispatcher inline — it's just one place." | New sinks live behind their own feature flags so deployments stay lean and OOM-on-CI doesn't recur. Add the feature flag, gate the dep, gate the dispatcher branch. |
| "Splitting CI by feature is annoying — I'll combine the jobs." | The split exists because combined builds OOM on standard runners (`efe2e8f`). Don't recombine without a verified solution. |
| "I'll bump `peat-mesh` to the latest RC for the new feature." | The pin (`=0.9.0-rc.1`) is intentional. Bumps need full integration validation and possibly chart/Zarf updates. |
| "Admin UI changes don't need a build — they're just frontend." | The UI ships as static assets via the Helm chart. Build it (`cd ui && pnpm build`) so deploy-time bundling doesn't break. |
| "I'll add the new identity provider as a sibling to OIDC, no feature flag — it's just config." | OIDC is feature-gated for a reason. Identity providers carry transitive security/runtime deps; gate the new one. |
| "I'll inline the KMS call without zeroization — it's only in memory briefly." | KMS material lives in security-sensitive paths. Use the existing zeroization patterns; don't introduce un-zeroized secret handling. |
| "I'll suppress the new clippy warning by adding `#[allow]` everywhere." | Suppression hides the signal. Either fix the underlying issue or, if it's genuinely a false positive, gate with a single `#[allow]` and a comment explaining why. |

## Scope guards

- Touch only files the issue/user asked you to touch.
- Do not edit other peat-* repos. Cross-repo work goes in a separate PR in that repo, linked through a tracking issue.
- Keep new sinks / providers / backends behind their own feature flags. Do not pull optional runtime deps (NATS, Kafka, Postgres, Vault clients) into the default feature set.
- Do not bleed control-plane logic into mesh code (and vice versa). The seam is `peat-mesh`'s public API (used here with `automerge-backend` and `broker` features).
- Do not commit secrets, KMS material, or absolute paths in `chart/`, `zarf.yaml`, `bundle/`, or test fixtures.
- Do not configure git to bypass GPG signing or use `--no-verify` to skip pre-commit hooks.

## Gotchas

Add an entry each time a session produces output that needed correction. One line per gotcha plus a `Why:` line.

- *(none recorded yet)*

## References (read on demand, not by default)

- Ecosystem invariants: `peat/SKILL.md` (sibling repo)
- Build matrix and feature flags: `CONTRIBUTING.md`
- Architecture deep-dives: `docs/architecture.md`, `docs/cdc.md`, `docs/multi-tenancy.md`, `docs/identity-federation.md`, `docs/envelope-encryption.md`, `docs/horizontal-scaling.md`, `docs/performance.md`, `docs/deployment.md`
- Helm chart: `chart/peat-gateway/`
- Zarf packaging: `zarf.yaml`
- UDS bundle: `bundle/uds-bundle.yaml`
- Repo: https://github.com/defenseunicorns/peat-gateway

---
*Last updated: 2026-05-05*
*Maintained by: Kit Plummer, VP Data and Autonomy, Defense Unicorns*
