# Shared graph and Go semantic analysis

## Playbook
- [x] 1. Read the code it touches and trace the real flow end to end. Reuse before you write.
- [x] 2. Name the data shape first, and the structure that holds it (typed model, table, state machine) over scattered conditionals.
- [x] 3. Climb the lazy ladder: does it need building, does it exist here, stdlib, platform, installed dep, one line.
- [x] 4. Build in small units that each end in a check; run it before starting the next.
- [x] 5. Verify the behaviour on the real surface; use .pi/verify.
- [x] 6. Report what you built, choices, skipped scope and evidence.

## Implementation / checks
- [x] 7. TDD shared declaration IDs/spans/reference edges/roots/completeness and graph validation.
- [x] 8. TDD Go helper real compiler bindings and tests for reachability, methods, init, generics, interfaces, build/test profiles, loading failures.
- [x] 9. TDD integration, CLI config and opt-in execution, offline policy, timeouts, tracing and fix gating.
- [x] 10. Documentation, Go coverage, Rust coverage, clean-copy verification, self-scan and CRAP.

## Decisions
- Keep the Rust CLI; embed a small Go helper and pinned go/packages dependency.
- `--semantic-go` explicitly trusts Go execution. Offline by default; downloads require `--go-downloads`. Bound execution with `--go-timeout`.
- Shared IR: declaration IDs, byte spans, owners, reference edges, roots, package files and analysis profiles/completeness. Rust validates it and computes reachability.
- Load compiler-selected files and generated test harnesses; no semantic-result cache (Go's compiler cache handles rebuilding).
- Conservative type-based interface dispatch, function-value references and external escapes; skip flow-sensitive SSA/pointer analysis for this first backend. No other language backend in this task.
- Callers: discovery -> resolve::build (CLI normal/fix rescan and library tests) -> report::build/output::traces/fix::plan_with. Preserve previous uncommitted work.
- Verification: unit graph tests, Go fixtures/coverage, subprocess CLI tests (failures/timeouts/profiles/cache), clean-copy .pi/verify, Rust LCOV/CRAP and nonempty Rust/Go self-scans.

## Verification — measured
- 92 Rust tests and 11 Go fixture tests pass; new regressions were run red before fixes.
- Clean-copy `.pi/verify-clean` passes formatting, lint/vet, tests, Knip parity, workflow checks, release configuration, tag dispatch and packaging.
- Rust line coverage: 90.11% overall; shared IR 98.44%; Go bridge 90.31%. Go adapter statement coverage: 88.8%.
- Coverage-backed CRAP diff checks pass separately for `src` (LCOV) and `backends/go` (Go coverprofile), with zero offenders.
- Real CLI self-scans pass with nonempty semantic graphs and no findings: full project (31 files), isolated Go backend (4 files).
- Safety regressions cover inherited-pipe timeouts, offline cache misses, inactive malformed files, workspace API policy, cgo policy/generated code, local replacement initialization, embedded/cross-language assets, invalid/incomplete IR and guarded file removal.
- Earlier CLI parity work is preserved and included with the implementation after explicit commit/push approval.
