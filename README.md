# deadwood

Find unused files and dependencies in TypeScript, Go, Rust, and Lua projects.

Deadwood is a Rust CLI built for both humans and agents. JSON is the default
output and is bounded by a token budget so an agent can scan a repository
without blowing its context window.

## Use

```console
deadwood .
deadwood . --max-tokens 4000
deadwood . --format text
deadwood . --include-tests
deadwood . --config deadwood.toml
deadwood . --fail-on high
```

The JSON report includes a version, verdict, summary counts, findings with
confidence and evidence, diagnostics, and the number of findings withheld by
the token budget. TypeScript findings include unreachable files, unused runtime
and development dependencies, optional peer dependencies, unused exports and
types, duplicate exports, unresolved and unlisted references, script binaries,
and pnpm catalog entries.

The analyzer follows configured and inferred entry points, resolves
tree-sitter-discovered module edges, and reports unused files, dependencies,
and symbols for TypeScript, Go, Rust, and Lua. TypeScript has the additional
module-specific export/member categories validated against Knip.

## Development

```bash
cargo test --locked
cargo build --release --locked
mise run coverage
target/release/deadwood src --max-tokens 2000
.pi/verify
.pi/verify-clean
```

The TypeScript comparison fixture in `tests/fixtures/knip-ts` compares
Deadwood with pinned Knip `6.41.0` for unused files, dependencies, exports,
duplicates, optional peers, unresolved and unlisted references, binaries, and
pnpm catalog entries; the test fails on any mismatch. It requires Node.js and
npm.

Deadwood uses Conventional Commits. Releases publish four platform binaries
through the shared matt-riley-ci Release Please and Homebrew workflows.
