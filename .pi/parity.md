# CLI parity work

Scope: TypeScript, Go, Rust, Lua. No LSP, editor integration or MCP. Implementation completed before commit/push approval.

## Playbook
- [x] Read the code it touches and trace the real flow end to end. Reuse before you write.
- [x] Name the data shape first, and the structure that holds it (typed model, table, state machine) over scattered conditionals.
- [x] Climb the lazy ladder: does it need building, does it exist here, stdlib, platform, installed dep, one line.
- [x] Build in small units that each end in a check; run it before starting the next.
- [x] Verify the behaviour on the real surface; use `.pi/verify`.
- [x] Report what you built, choices, skipped scope and evidence.

## Implementation / verification
- [x] Model + regressions: package-owned files/manifests and symbol bindings; `cargo test --test parity` initially fails the measured regressions, then checks positive/negative findings and nonzero source counts.
- [x] Discovery/resolution: `discover` -> `roots` -> resolve helpers -> dependency ownership in `report`; aliases, workspaces, tests, Go siblings, Rust nested/build dependencies, Lua module mapping regression tests.
- [x] Symbol graph + cycles: `parse` -> `resolve::build` -> `report::build`; Knip differential fixtures and native symbol tests, nonempty expected categories.
- [x] Configuration/reporting/modes: `Config` -> CLI -> renderers; subprocess tests for production, strict ownership, globs, suppressions, severity, exit codes, trace, JSON/SARIF and incomplete analysis.
- [x] Guarded fixes: CLI -> fix planner -> preview/apply -> rescan; byte-exact temporary projects, stale-source and incomplete-analysis refusal, side effects, deletion opt-in and native compiler checks.
- [x] Cache/watch/debug: discovery cache + CLI loop; cached/uncached equality after content/config/manifest/create/delete changes; bounded watch subprocess test.
- [x] Final verification/documentation: `.pi/verify-clean`, LCOV coverage, `crap lcov.info --diff`, rebuilt nonempty self-scan, CLI examples and capability/limitation documentation.

## Measured evidence
- Clean-copy `.pi/verify-clean` passed: formatting, strict Clippy, 77 tests, release build, pinned Knip differential comparison, workflow/release validation and packaging.
- Rebuilt full-project CLI scan: 21 files, 11 roots, all reachable, complete/ok, zero findings or diagnostics. Deliberately faulty fixture sources are excluded in `deadwood.toml`.
- LCOV generated with `cargo llvm-cov --lcov --output-path lcov.info`: 89.48% line coverage (88.60% region coverage).
- `crap . --coverage lcov.info --diff --fail-above`: zero offenders, worst score 29.7 at threshold 30.
- Native fix regression runs offline Cargo compilation and, when available, Go compilation; both ran successfully locally.
- Commit/push subsequently approved by the user.

## Explicit boundaries
- skip: full Knip framework plugin/configuration-extractor catalog; only the documented conventional entry adapters are implemented.
- skip: bundled non-source component compilers; trusted user-supplied compiler commands are supported instead.
- skip: compiler-level native type checking/macro expansion/full feature unification and TypeScript class type checking; analysis remains conservative and destructive fixes require complete, unresolved-free analysis.
- skip: LSP, editors and MCP, per scope.

Configuration/reporting/fix/cache capabilities and these boundaries are documented in `README.md`.
