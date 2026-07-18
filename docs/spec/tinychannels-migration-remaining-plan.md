# TinyChannels Migration — Remaining-Work Plan

Source-of-truth plan for **finishing** the OpenHuman channel migration into the
`tinychannels` crate. Derived from a verified 2026-07-17 audit of both repos and
CI. Supersedes the current-state claims in
[`tinychannels-execution-plan.md`](tinychannels-execution-plan.md) and
[`../porting.md`](../porting.md), both of which predate the
`feat/host-boundary-provider-migration` work and understate progress. Rewriting
those two docs to match is itself a tracked slice (G1).

> Line/anchor references below are "as of the 2026-07-17 audit"; re-confirm
> before editing, since unrelated commits may shift them.

## Verified current state — the migration is ~90% done

The bulk of "move the messaging channels into this crate" has already landed.

- **Phases 0–5 complete**: core `channel/` types (descriptor, envelope, intent,
  receipt, capabilities, session, error taxonomy), the `text/` chunker
  (UTF-16/fence/`(i/N)`), the `channel/adapter.rs` + `harness/` bridge, the
  `delivery/` durable queue, and the `relay/` descriptor + HMAC auth + framed
  transport + feature-gated WebSocket dialer.
- **Phase 6 (providers) mostly complete**: 16 providers live under
  `src/providers/` (dingtalk, discord, email, imessage, irc, lark, linq,
  mattermost, qq, signal, slack, telegram, whatsapp, whatsapp_web, yuanbao).
  In OpenHuman the corresponding files are now **1-line re-exports**
  (e.g. `channels/providers/discord.rs` = `pub use tinychannels::providers::discord::*;`).
- **Host boundary landed**: `src/host/` (mod/dispatch/services/noop) is the seam
  ported providers use to call back into the app for side effects (HTTP proxy
  client, approval, voice/STT, pairing, conversation memory).
- **OpenHuman consumes the crate**: `tinychannels = { path = "vendor/tinychannels",
  features = ["relay-websocket"] }`; `vendor/tinychannels` tracks the same HEAD
  as the standalone repo (no drift).
- **Telegram is split correctly**: transport/session_store/pairing moved to
  `tinychannels::providers::telegram`; only host-coupled glue
  (`remote_control`, `bus`, `approval_surface`) remains in OpenHuman.

### Out of scope by design (NOT porting targets)

- The OpenHuman `web` provider and the `runtime/` dispatch engine are
  **consumers** of this crate. Moving them would drag the whole app in and
  create a dependency cycle. They stay in OpenHuman. So do the `ChannelBackend`
  *implementation*, the event-bus wiring, and channel-specific host glue.

## Toolchain note

Local `rustc 1.94.0` cannot build `libsqlite3-sys 0.38.1` (`cfg_select!`). Use
the installed **`rustc 1.96.1`** toolchain locally (`cargo +1.96.1 …`); CI runs
`stable` and is the gate of record.

## Ranked remaining work

Each row is an independently-mergeable PR with its own acceptance gate. Branch
per slice off `main`; tests ship with code; no push without explicit approval.

| ID | Slice | Repo | Effort | Class |
| --- | --- | --- | --- | --- |
| G0 | Repair red `main` (hmac crypto-stack break) | tinychannels | S | Blocker |
| G7 | Session-key `scope_id` switchover for non-relay providers (bug 2) | both | M | Correctness |
| G5 | WhatsApp-Web durable session storage (rusqlite backend) | tinychannels | L | Functional gap |
| G2 | Delivery-durability negotiation tests (all 13 capability keys) | tinychannels | M | Test gate |
| G3 | Controller definitions↔schema parity test | tinychannels | M | Guardrail |
| G4 | Relay WebSocket loopback acceptance tests | tinychannels | M | Phase-5 gate |
| G6 | Delete OpenHuman-side portable duplicate tests | openhuman | M | Cleanup |
| G1 | Rewrite stale plan/porting docs to match reality | both | M | Docs |

### G0 — Repair red `main` (do first; unblocks everything)

**Problem.** Dependabot merged `hmac 0.12.1 → 0.13.0` (PR #4) without adapting
the code or the hash stack. `hmac 0.13` requires `digest 0.11`, but `sha2` and
`sha1` are still `0.10` (`digest 0.10`), so `Hmac<Sha256>` / `Hmac<Sha1>` fail
their trait bounds, and all call sites still use the `0.12` `new_from_slice`
API. `Cargo.lock` carries **both** `hmac 0.12.1` and `0.13.0`.

**Evidence.** CI on the latest `main` HEAD (merge PR #8) is a **failure**; the
run downloads `hmac 0.13.0` + `sha2 0.10.9`. Local `cargo +1.96.1 build --lib`
reports 25 errors. Affected sites: `src/relay/auth.rs:19,33`,
`src/providers/linq.rs:396`, `src/providers/yuanbao/sign.rs:64`,
`src/providers/yuanbao/cos.rs:21`.

**Fix (minimal, correct).** Revert `Cargo.toml` `hmac` to `"0.12"` and update
`Cargo.lock`; the code was written for `0.12` and never adapted. Close/ignore
the dependabot `hmac 0.13` PR until the whole SHA stack can move to `digest 0.11`
together (a separate, larger change — see open questions). Do **not** paper over
it by pinning a nightly or disabling the check.

**Acceptance gate.** `cargo +1.96.1 build --all-targets`, `cargo clippy
--all-targets -- -D warnings`, `cargo test` all green; `Cargo.lock` no longer
lists `hmac 0.13.0`; CI green on the branch.

### G7 — Session-key `scope_id` for non-relay providers (known bug 2)

**Problem.** Relay inbound already preserves `scope_id`, but non-relay providers
build legacy string session keys that omit guild/team/tenant scope, so
conversations in different workspaces can collapse onto one key.

**Files.** `openhuman/src/openhuman/channels/bus.rs` (legacy key construction,
~1005–1042), `tinychannels/src/channel/session.rs` (canonical grammar +
legacy-key canonicalization helper).

**Approach.** Feed provider-sourced scope facts into native envelope
construction; keep the legacy-key canonicalization path so existing OpenHuman
session state still resolves. Ship the migration test mapping sample old keys →
new keys **before** touching identity, per `prompt.md`.

**Acceptance gate.** Cross-workspace non-relay inbound get distinct keys; legacy
conversations still resolve; `openhuman` `tests/memory` + session-key suites
green.

### G5 — WhatsApp-Web durable session storage

**Problem.** The provider uses `wacore::store::InMemoryBackend`, so the linked
WhatsApp session is lost on restart and must be re-paired. Upstream's Diesel
`sqlite-storage` feature conflicts with the `rusqlite 0.40` baseline.

**Evidence.** `src/providers/whatsapp_web.rs` migration note (lines ~34–40) and
backend construction (~294–304).

**Approach.** Implement a `rusqlite`-backed `wacore::store::traits::Backend` so
session state persists at the reserved `session_path`, avoiding the Diesel link
chain. Behind the existing `whatsapp-web` feature.

**Acceptance gate.** Session survives restart without re-linking; no Diesel dep;
`cargo test --features whatsapp-web` green.

### G2 — Delivery-durability negotiation tests

**Problem.** `DurableFinalDeliveryCapability` has **13** variants
(`src/channel/capabilities.rs:86–100`) but the negotiation tests exercise only
~4 (Text/Media/ReplyTo/Thread) at `src/delivery/test.rs:454–504`; policy at
`src/delivery/policy.rs`.

**Acceptance gate.** Every variant exercised — unsupported downgrades to
best-effort, supported preserves required; `cargo test --all-features` green.

### G3 — Controller definitions↔schema parity test

**Problem.** Nothing ties `ChannelAuthMode` / `AuthModeSpec`
(`src/controllers/definitions.rs`) to the connect schema
(`src/controllers/schemas.rs:88–104`); a mismatch surfaces only at runtime.

**Acceptance gate.** Adding a `ChannelAuthMode` without updating definitions +
schema fails a test; `cargo test` green.

### G4 — Relay WebSocket loopback acceptance tests

**Problem.** `WebSocketRelayIo` (`src/relay/websocket.rs`) has only 3 unit tests
(`src/relay/test.rs:753–804`); integration coverage uses the `MemoryRelayIo`
mock, so real framed I/O + reconnect + signed-delivery accept/reject is
unproven.

**Acceptance gate.** Loopback tests assert frame sequencing, reconnect, and
signed-delivery accept/reject; `cargo test --features relay-websocket` green.

### G6 — Delete OpenHuman-side portable duplicate tests

**Problem.** Integration Step 4 deletes tests now owned by the crate.

**Files.** `openhuman/src/openhuman/channels/controllers/schemas_tests.rs`
(remove), `.../host/tests.rs` (remove), `.../controllers/ops_tests.rs` (keep
only the backend/REST-wiring halves).

**Acceptance gate.** Duplicates removed; OpenHuman CI green; no coverage loss on
retained app-side wiring.

### G1 — Rewrite stale docs

Update `tinychannels-execution-plan.md`, `porting.md`, `AGENTS.md`,
`Cargo.toml` feature comments, and `openhuman/docs/plans/tinychannels-integration.md`
to reflect: providers ported, `security.rs` documented, `relay-websocket`
feature semantics, and known-bugs 2/3/5 marked resolved/deferred with reasons.
Fold this into whichever code slice touches the same area, or land standalone.

## Suggested sequencing

1. **G0** first — nothing else can be validated on a red `main`.
2. Then the correctness/functional items: **G7**, **G5** (independent; either
   order).
3. Test-gate hardening: **G2**, **G3**, **G4** (independent, parallelizable).
4. **G6** once the crate suites are the source of truth.
5. **G1** doc rewrite last (or piecewise alongside each slice).

## Open questions for maintainers

- **Crypto stack direction (affects G0):** revert `hmac` to `0.12` now (minimal,
  recommended), or move the whole SHA stack to `digest 0.11`
  (`hmac 0.13` + `sha2 0.11` + `sha1 0.11`) as a coordinated bump? The latter is
  larger and gated on stable releases of the `0.11` hashes.
- **Bug 3 (`conversation_memory_key` uses `msg.id`):** keep per-turn keying and
  rename to `turn_memory_key`, or coalesce per conversation? OpenHuman product
  decision, per the execution plan.
- **G5 scope:** is durable WhatsApp-Web session persistence in-scope for this
  crate, or should the host own that store via a backend trait?

## Non-goals (unchanged)

Provider SDK internals, OpenHuman UI widgets, model/agent execution, credential
storage implementations, provider daemons, the `web` provider, and the
`runtime/` dispatch engine stay out of this crate.
