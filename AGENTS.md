# Repository Guidelines

## Project Structure & Module Organization

TinyChannels is a Rust 2024 library crate rooted at `Cargo.toml`. Public API
exports live in `src/lib.rs`, with the crate-wide error type in `src/error.rs`.
The initial scaffold reserves two top-level module directories: `src/channel/`
for channel-side messaging abstractions and `src/harness/` for harness-facing
communication boundaries.

Prefer small, focused modules that do one thing clearly. New feature areas
should live in module directories instead of accumulating broad multi-purpose
files. Within each module directory, keep shared type definitions in a
dedicated `types.rs` file and keep module-local unit tests in a dedicated
`test.rs` file. The module root should wire the pieces together and expose the
smallest useful API.

Integration tests belong in `tests/` once behavior exists. Design notes and
module-level specifications live in `docs/`, with `docs/spec/README.md` as the
top-level architecture reference.

## Build, Test, and Development Commands

- `cargo fmt --check`: verify Rust formatting without changing files.
- `cargo fmt`: format the crate before committing.
- `cargo clippy --all-targets -- -D warnings`: run lint checks for the library
  and tests, treating warnings as failures.
- `cargo build --all-targets`: compile all crate targets.
- `cargo test`: run the full test suite.

Run commands from the repository root unless a future workspace layout changes
the crate location.

## Coding Style & Naming Conventions

Use standard `rustfmt` output and Rust 2024 idioms. Module and file names should
be `snake_case`; public types and traits should be `PascalCase`; functions,
methods, fields, and local variables should be `snake_case`. Prefer small typed
APIs with `Result<T>` using the crate error type exported from `src/error.rs`.
Keep public exports centralized in `src/lib.rs` so downstream users have a
predictable surface.

## Testing Guidelines

Place integration tests in `tests/` and use descriptive test names such as
`serializes_channel_message`. Add focused tests when changing serialization,
routing, transport adapters, lifecycle events, or public request/response
shapes.

Add or update tests with every behavior change, and document any intentionally
untested edge case in the PR description.

## Documentation Expectations

Write documentation for public APIs, architecture decisions, examples, and
non-obvious behavior. Keep `README.md`, `docs/spec/README.md`, and module docs
aligned with code changes.

Keep every Markdown file, including `AGENTS.md`, at 500 lines or fewer. When a
topic grows past that limit, split it into focused files and link them from the
module's `README.md`. Complex modules should include a module-level `README.md`
that explains the design, public surface, and important operational
constraints.

## Commit & Pull Request Guidelines

Keep commit subjects concise and imperative. The first line should be specific
to the change and avoid bundling unrelated work.

Pull requests should include a short summary, the commands run locally, and any
API or behavior changes. Link related issues when available. Include updated
examples or docs when public APIs, architecture, or expected usage changes.

Always make small, focused commits. Each commit should cover one logical change,
build independently, and avoid mixing formatting, refactors, and behavior
changes unless they are inseparable.
