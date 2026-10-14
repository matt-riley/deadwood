# deadwood

Find unused files, dependencies and symbols in TypeScript/JavaScript, Go, Rust,
and Lua projects. Deadwood is a Rust CLI for humans and agents: JSON is the
default, with bounded output, evidence, severity and analysis-completeness flags.

## Use

```console
deadwood .
deadwood . --format text
deadwood . --production
deadwood . --strict --workspace 'packages/*'
deadwood . --include unused_export,unused_type --include-entry-exports
deadwood . --exclude duplicate_export --fail-on high
deadwood . --trace-file src/service.ts
deadwood . --trace-export handler
deadwood . --trace-dependency serde
deadwood . --cache --watch
deadwood . --format sarif
```

Tests are included by default; use `--exclude-tests` or
`include_tests = false` in configuration. `--include-tests` overrides that
configuration. Production mode excludes tests and development dependencies.
Strict mode also enables production mode and prevents dependencies leaking
between a workspace and its ancestors.

Entry exports are treated as a public boundary unless
`--include-entry-exports` is set. `--all` enables every issue type and checks
entry exports. Runtime cycles are opt-in through `--include cycle`, `--all`,
or a configured `cycle` rule; type-only imports are not runtime edges.

`--include` and `--exclude` select **issue types**, not file paths. Configure
file selection with `project`, `exclude` and workspace settings below.
Workspace selectors accept package names, directory globs and negation
(`--workspace '!packages/legacy'`); analysis still follows cross-workspace edges.

### Output and exit codes

Formats: `json`, `text`, `compact` (concise text), `markdown`, `sarif`
and `github-actions`. JSON includes `v`, `complete`, `ok`, `summary`,
`findings`, `diagnostics`, `dropped`, and optional `traces`/`fixes`.
Confidence describes certainty; severity controls the configured verdict.

- **0:** no selected failure.
- **1:** findings cross the `--fail-on` threshold (`error` by default;
  alternatives: `warn`, `high`, `none`).
- **2:** invalid input/configuration or incomplete analysis. Missing roots,
  malformed source, unreadable files and unsupported dynamic references are
  not silently treated as clean.

`--fail-on none` does not turn incomplete analysis into a success. Ordinary
unresolved references remain findings; fixes reject them even when filtered out.
JSON defaults to `--max-tokens 4000`; withheld findings are counted.
`--trace` includes all available file/export/dependency reference traces;
the targeted flags above narrow them. Traces are static reference citations,
not runtime execution traces.

`--debug`, `--performance`, `--duration` and `--memory` write diagnostics
to stderr without contaminating JSON. Memory reporting uses `ps` where available.

## Analysis support

| Language | Discovery and resolution |
|---|---|
| TypeScript/JavaScript | ESM/CommonJS, destructured bindings, barrels, namespace imports, aliases, JSONC/inherited tsconfig paths, source-first build outputs, workspace packages and conditional/wildcard package imports/exports |
| Go | go.mod/go.work, local replace directives within the scanned tree, complete package siblings, initializers, build tags and OS/architecture filename suffixes |
| Rust | Cargo workspace members and custom targets, build dependencies, nested modules, path attributes, grouped/aliased uses, self/super/crate paths, public library boundaries, static cfg and default/selected features |
| Lua | require/dofile/loadfile, returned module tables, configurable package paths and rock-to-module providers, Neovim/LÖVE entry conventions |

All four languages report unused files/dependencies/symbols and unresolved or
unlisted references. TypeScript additionally reports unused exports/types,
class/enum/namespace members, duplicate exports, optional peers, script binaries,
and pnpm catalog entries/references. Dependency usage is package-owned and
restricted to reachable files, not every import found anywhere in the tree.

Literal `import.meta.glob`, worker URL and Rust include paths create source
edges. Nonliteral module references produce incomplete-analysis diagnostics
instead of unsafe deletion candidates. Go test harnesses, exported receiver
methods, Rust trait implementations/tests and LÖVE callbacks are conservatively
preserved.

Built-in entry adapters cover Next, Remix/React Router, Astro, SvelteKit, Nuxt,
Storybook and common test/config files. They activate from package dependencies;
`plugins = ["neovim", "love"]` enables Lua conventions explicitly. Adapters
are package-relative, and runtime framework entries remain active in production.

### Native build profiles

```console
deadwood . --features fast --no-default-features
deadwood . --all-features --target aarch64-apple-darwin
deadwood . --go-tags integration --target linux/amd64
```

Rust target triples and Go OS/architecture pairs select static source conditions.
Unknown Rust cfg predicates are kept rather than guessed away.

## Configuration

An optional `deadwood.toml` is loaded from the project root, or use `--config`.
Paths are root-relative; workspace paths/entry patterns are package-relative.
Globs support braces and negation in `entries`/`project`. Gitignore rules
are respected unless `--no-gitignore` is set.

```toml
entries = ["src/index.ts", "scripts/*.ts", "!scripts/obsolete.ts"]
project = ["src/**/*.{ts,tsx}", "scripts/*.ts"]
exclude = ["generated", "**/*.generated.ts"]
include_tests = false
ignore_dependencies = ["@types/**"]
ignore_binaries = ["local-tool"]
ignore_unresolved = ["virtual:*"]
ignore_members = ["Internal.*"]
ignore_exports_used_in_file = true
tags = ["-internal"]

[paths]
"@app/*" = ["src/*"]

[rules]
unused_export = "warn"
duplicate_export = "off"
cycle = "warn"

[ignore_issues]
"src/compat.ts" = ["unused_export"]

[workspaces."packages/*"]
entries = ["src/index.ts"]
project = ["src/**"]
ignore_dependencies = ["workspace-tool"]
include_entry_exports = true

[lua_modules]
socket = "luasocket"
```

Other settings include `strict`, `production`, `include`,
`include_entry_exports`, `ignore_workspaces`, `lua_paths`, `plugins`,
`features`, `all_features`, `no_default_features`, `go_tags`, `target`,
and `compilers`. Unknown keys, invalid patterns, issue types and severities fail
validation. Suppressions on a finding's line or the preceding line accept
`deadwood-ignore` or `deadwood-ignore: unused_export`.
Tag filters use adjacent documentation tags, e.g. `/** @internal */`.

## Compiler-backed Go

The default scan stays parser-only. `--semantic-go` opts into Go's compiler
bindings and a shared declaration/reference graph:

```console
deadwood . --semantic-go --go-downloads
deadwood . --semantic-go
deadwood . --semantic-go --go-tags integration --go-target linux/amd64 --production
```

Requires Go **1.26+**. The Rust binary embeds the small Go adapter and pinned
module files; no separate helper is needed. Normal semantic scans are offline.
`--go-downloads` permits missing module downloads; toolchain auto-downloads stay
disabled. Compilation/loading is bounded by `--go-timeout` (default 120 seconds,
range 1–3600).

This is trusted toolchain execution, including cgo—not a sandbox. Application
code, test functions and `go:generate` are not run. Compiler/dependency errors,
invalid adapter output and timeouts produce incomplete reports (exit 2), never
heuristic fallback, and block fixes.

The adapter honors modules/local replacements, root `go.work`, build tags,
OS/architecture, `CGO_ENABLED`, production/test profiles and embedded files.
Compiler identities distinguish shadowing and same-named methods. Roots cover
initialization, main/test harnesses, configured entries and public library APIs.
`include_entry_exports = true` checks those APIs rather than assuming external
consumers. Interface dispatch and external value escapes are conservative;
references inside dead declarations no longer keep unreachable cycles alive.

Reports add bounded `analysis` metadata: effective compiler profile, completeness
and graph counts. Symbol traces follow declaration edges. Generated declarations
are not removal candidates. Results are recomputed every scan; only parsing and
Go's build caches are reused. Semantic Go file removal additionally requires
`--verify-command`. Go symbols are reported, not automatically deleted.

This backend covers package declarations and methods, not local/field cleanup
or flow-sensitive SSA/pointer analysis. Arbitrary reflection, plugins and foreign
entry points still need configured entries. Reachability applies to the selected
profile, not every deployment. Other languages' compiler backends are deferred.

### Explicit extension commands

`--preprocessor 'command args'` accepts a v1 JSON report on stdin and must
return a valid v1 report; it can be repeated. `--reporter 'command args'`
accepts that report and returns presentation text. Neither can override the
exit status determined from the original analysis.

Configured compilers map file extensions to commands that consume source on
stdin and emit TypeScript/JavaScript. They run only with `--allow-compilers`;
original source identities are retained and generated output is not auto-fixed.

```toml
[compilers]
widget = "my-widget-compiler --emit-typescript"
```

Commands use explicit argv parsing, not an implicit shell. Use a script for
pipelines. These are trusted local commands, not a sandbox.

## Guarded fixes

```console
deadwood . --fix --dry-run
deadwood . --fix
deadwood . --fix --allow-remove-files
deadwood . --fix --verify-command 'cargo test --all-features --target-dir target'
```

Fixes edit unused npm dependencies/catalog entries and remove unused TypeScript
export wrappers/specifiers while retaining declarations and their side effects.
Ambiguous transforms, generated sources and re-exports are skipped. Unreachable
file deletion requires `--allow-remove-files`. Only error-severity findings
selected by configuration/workspace filters are eligible.

Cargo/go.mod/rockspec dependency edits require an explicit `--verify-command`;
without one they are skipped. Choose verification covering the profiles you
intend to support. Default/anonymous expressions and native symbols are not
blindly deleted.

The planner rejects incomplete/unresolved analysis, stale snapshots, symlinks
and escaped paths. Apply creates byte-exact backups under
`.deadwood-backup/<run>/`, writes atomically, verifies when requested and
rescans. Failed writes/verification roll back planned edits; concurrent external
changes prevent rollback overwrite and leave the backup available. Arbitrary
side effects of your verification command are outside that transaction.
`--dry-run` writes nothing.

The content-aware parse cache defaults to `.deadwood-cache`, or use
`--cache-location`. Discovery and the graph are rebuilt on every run.
Watch mode polls content changes, including configuration, manifests and
created/deleted files; each run emits a fresh report.

## Parity boundaries

The pinned Knip differential fixture verifies the TypeScript categories it
covers. This is **not a claim of complete Knip ecosystem parity**:

- The full Knip plugin/configuration-extractor catalog is not ported. Built-in
  adapters discover conventional entries, not every framework configuration
  string. Use explicit entries/paths/ignores or trusted compilers for the rest.
- Non-source component formats require a compiler; there are no bundled Vue,
  Svelte or Astro component compilers.
- Parser-only native usage is conservative static analysis. Opt-in Go semantics
  adds compiler binding/type checking, but not full flow/reflection analysis.
  Rust macro expansion and full Cargo feature/target unification remain out of
  scope. Class-member matching is not a TypeScript type checker.
- Runtime-generated module names require configuration; unknown dynamic
  references block destructive fixes.
- LSP, editor integrations and MCP are deliberately out of scope.

## Development

```bash
cargo test --locked
cargo build --release --locked
mise run coverage
crap src --coverage lcov.info --diff --fail-above
crap backends/go --coverage backends/go/coverage.out --diff --fail-above
target/release/deadwood . --semantic-go --fail-on high
.pi/verify
.pi/verify-clean
```

`tests/fixtures/knip-ts` compares findings with pinned Knip `6.41.0`;
any mismatch fails the test. Node.js/npm are required for that comparison.
Regression fixtures exercise all supported languages, actual CLI reporters,
cache invalidation, watch mode, suppressions, fixes and rollback. Rust native
fix verification runs Cargo offline; Go compilation is also checked when Go
is installed. Semantic Go fixtures require Go; `mise run install` primes the\npinned adapter dependency cache before offline tests.

Deadwood uses Conventional Commits. Releases publish four platform binaries
through the shared matt-riley-ci Release Please and Homebrew workflows.
