# CLAUDE.md — `peat-gateway`

Before doing any work in this repo, read **both** of:

1. `SKILL.md` (this repo) — the per-repo workflow, verification checklist, and scope guards.
2. `peat/SKILL.md` (in the sibling `peat` repo, if checked out alongside) — the ecosystem skill: hard invariants, FFI conventions, the skill router across all peat-* repos.

If `peat/SKILL.md` isn't accessible, say so before proceeding — most architectural invariants live there, not here.

## Quick orientation

- **Repo role:** Enterprise control plane for Peat mesh — multi-org tenancy, CDC (change data capture), IDAM federation. Rust binary plus a reusable library crate (`peat_gateway`), with feature-gated sinks (NATS, Kafka, webhook), identity providers (OIDC), storage backends (Postgres), and KMS integrations (AWS KMS, Vault). Ships with a Svelte admin UI (`ui/`), a Helm chart (`chart/peat-gateway/`), a Zarf manifest, and a UDS bundle.
- **Primary languages:** Rust (control plane); TypeScript/Svelte (admin UI under `ui/`, built with pnpm + Vite).
- **Cheap sanity check:** `cargo build` (default features = `nats`, `webhook`, `oidc`).

## Hard rule

A task in this repo is not done until the verification checklist in `SKILL.md` produces evidence. "Seems right" or "the diff looks correct" is never sufficient.

The PR self-review convention previously captured in this file is preserved verbatim in `SKILL.md` Workflow step 6 — read it before opening any PR.

GPG-signed commits are required by repo policy. Cross-repo changes require one PR per repo, linked through a tracking issue — not a single PR that reaches across repos.
