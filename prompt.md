# Goal

Implement the TinyChannels channel layer described in
`docs/spec/tinychannels-execution-plan.md`, phase by phase, and integrate it
into OpenHuman per `../openhuman-4/docs/plans/tinychannels-integration.md`.

The end state: OpenHuman depends on this crate for all portable channel
types (descriptors, envelopes, outbound intents, receipts, capabilities,
session keys, chunking, error taxonomy, adapter trait, harness bridge), the
duplicated copies in openhuman-4 are deleted, and the known channel bugs are
fixed once, here.

## Read first (in this order)

1. `docs/spec/tinychannels-execution-plan.md` — the plan you are executing.
   It contains the current-state gap matrix, the numbered known-bug list,
   the phase definitions, and the test-migration/fixture catalog.
2. `docs/spec/openclaw-hermes-channel-porting.md` — the research spec with
   verified upstream type shapes. When a plan step says "mirror OpenClaw's
   MessageReceipt" or "Hermes' SendResult taxonomy", the exact fields are
   documented there.
3. `docs/porting.md` — what is already ported and the provider portability
   ladder.
4. `../openhuman-4/docs/plans/tinychannels-integration.md` — the app-side
   steps (dependency adoption, duplicate deletion, ChannelBackend impl,
   test split).

## Source-of-truth checkouts

Port against the pinned upstream checkouts only:

- OpenClaw: `/tmp/tinychannels-openclaw-src` @ `6445a063`
- Hermes: `/tmp/tinychannels-hermes-agent-src` @ `10f7cb04`

Do NOT use `~/work/tinyhumansai/references/{openclaw,hermes-agent}` — they
are ~3 months stale and missing the subsystems being ported. If the `/tmp`
checkouts are missing, re-clone upstream (openclaw/openclaw,
NousResearch/hermes-agent), pin the new commits in both spec docs, and
re-verify any type shape you rely on before porting it.

## Execution order

Work the plan's phases strictly in order; do not start a phase before the
previous phase's acceptance gate is green:

- Phase 0: hygiene fixes (plan bugs 6-9, 11). Small, land first.
- Phase 1: core types (`src/channel/`) + session keys + typed
  `ChannelBackend` returns. Gate: session-key and receipt fixture suites.
- Phase 2: text engine (`src/text/`). Gate: UTF-16/fence/`(i/N)` chunking
  suite, including the emoji case where char-count passes but UTF-16 fails.
- Phase 3: adapter trait + harness bridge (`src/harness/`).
- Phase 4: durable delivery queue (`src/delivery/`). Gate:
  backoff/permanent-error/reconciliation state-machine suite.
- Phase 5: local/API/webhook adapter, then relay descriptor + HMAC auth.
  Gate: byte-exact auth vectors matching Hermes `relay/test_auth.py`.
- Phase 6: provider adapters, following the ladder in `docs/porting.md`.
- OpenHuman integration (Steps 1-4 of the openhuman-4 plan) can start as
  soon as Phase 1 lands; Step 1 and Step 2 there should land together.

## Working rules

- Branch per phase off `main` in each repo (e.g. `feat/phase-1-core-types`);
  never commit directly to `main`. Make small, focused commits after each
  validated slice — do not batch a phase into one commit.
- PRs go to the upstream `tinyhumansai/*` remotes, not personal forks. If a
  base branch exists only locally or on a fork, push it upstream first.
- Every phase lands with its tests. `cargo check --all-targets`,
  `cargo clippy` (zero warnings), and `cargo test` must pass before each
  commit. The crate currently has 60 passing tests and zero warnings — keep
  both properties.
- When migrating a test from openhuman-4, delete it there only after it runs
  green here and openhuman-4 depends on this crate; until then duplication
  is acceptable, divergence is not.
- Preserve the audited invariants: no secrets in logs, no blocking calls in
  async paths, media arrays index-aligned, `delivered_via_upstream_relay`
  never serialized.
- Session-key changes alter conversation identity: ship them with the
  legacy-key canonicalization helper and a migration test mapping sample old
  keys to new keys before touching openhuman-4.
- If a plan step contradicts what you find in the code, or an upstream shape
  has drifted from the spec, stop and record the discrepancy in the plan doc
  (update it in the same PR) rather than silently improvising.

## Definition of done

- All phases 0-5 implemented and gated (Phase 6 providers may trail).
- openhuman-4 builds against the crate with duplicates deleted,
  `OpenHumanChannelBackend` implemented, and its channel test suite green.
- Plan bugs 1-11 (tinychannels) and Step 3 items 1-5 (openhuman-4) each
  resolved or explicitly deferred with a written reason in the plan docs.
- Both plan docs updated to reflect what actually shipped.
