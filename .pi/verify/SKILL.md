---
name: verify
description: Prove a deadwood change works in the real CLI before claiming it is done — build the release binary, then drive scans, reporters, traces, scopes, rules, fixes, watch mode and the compiler-backed Go backend against a throwaway project, and report what was observed. Use when verifying a deadwood change, before a release, or whenever deadwood's output must be trusted outside its test suite.
---

# Verifying deadwood

Deadwood is one Rust binary that reads a project and prints a report. It has no
server, no ports, no user config directory and no daemon. Verification means
building the real binary and driving it against a throwaway project, because the
parts most likely to break are exactly the ones unit tests cannot see:

- the Go compiler adapter (`--semantic-go`) shells out to a real toolchain,
  real module cache and real build profile;
- `--fix` writes to disk, and `--allow-remove-files` deletes files in the
  scanned project;
- `--watch` only proves itself across a real edit;
- `--preprocessor`, `--reporter` and configured compilers run commands the
  user trusts.

`cargo test` and `.pi/verify/run.sh` cover the Rust API and repo invariants
offline. They are a prerequisite, not the proof; the drives below are the proof.

## Target

Record what revision the evidence describes before the first drive:

```sh
git rev-parse HEAD
git status --porcelain=v1 --untracked-files=all   # untracked files are part of the change
git diff HEAD | shasum -a 256                     # staged + unstaged content
```

Any edit after a drive invalidates that drive for the final state. Report which
revision the observations belong to.

## Launch

Tools: Rust 1.99 and Go 1.26+ (mise pins both; plain `cargo`/`go` on PATH is
enough). Node/npm are needed only by `tests/compare_knip.py` inside
`.pi/verify/run.sh`; python3 only by the assertions here and that script. No
environment variable is required to run deadwood. No ports are opened.

```sh
cd "$(git rev-parse --show-toplevel)"
D=./target/release/deadwood
FIXTURE=$(.pi/verify/fixture.sh)   # prints a path, nothing else
```

`$FIXTURE` is a four-language project (TypeScript, Go, Rust, Lua) with planted
dead files, symbols, exports and dependencies. `.pi/verify/scratch.sh
<compilers|entry-exports|plugins|workspace>` builds the small single-purpose
projects that the fixture cannot express; `.pi/verify/run.sh` is the repo-wide
check and rebuilds the binary, and `.pi/verify-clean` runs it in a throwaway
copy of the checkout.

**Isolation.** Deadwood writes `.deadwood-cache` and `.deadwood-backup`
inside the project it scans, and `--fix --allow-remove-files` deletes files
there. Never point `--fix`, `--watch`, `--cache-location` or
`--allow-remove-files` at a real repository. Every drive below runs against
`$FIXTURE`, a scratch project, or a copy of the fixture. Drives read no user
configuration and write nothing outside those directories.

## Doctor

```sh
.pi/verify/doctor.sh
```

Observed on a ready checkout:

```text
doctor: ok   binary version 0.1.1 matches Cargo.toml
doctor: ok   go 1.27
doctor: ok   adapter dependencies are cached for offline builds
doctor: ok   --semantic-go fixture scan: 6 declarations, 1 references, 2 roots, toolchain go1.27.1
doctor: ready
```

It rebuilds the release binary (so a stale `target/` cannot masquerade as the
change), refuses a Go toolchain older than 1.26, proves the adapter's modules
still vet with `GOPROXY=off`, and fails unless a fixture scan really
produces a complete Go graph. It is not vacuous: given a module with a compile
error it fails and prints the adapter's diagnostics. It must not write into the
tree — `git status --porcelain -uall` is measured unchanged across a run, and
it vets the adapter instead of `go build ./...`, which once left a 7.8 MB
`go` binary in `backends/go/`. Run it before the first drive and again after
any failed drive.

## Features

`$D` and `$FIXTURE` are set as in Launch. "Observed" values were measured by
driving this revision; they are observations, not assertions another agent can
assume without re-running.

| Feature | Drive | What to observe | Prerequisites |
| --- | --- | --- | --- |
| Scan and exit codes | `$D "$FIXTURE"`; `--fail-on none`; `$D /nonexistent` | exit **1** / **0** / **2**; JSON on stdout with `v`, `complete`, `ok`, `summary`, `findings`, `diagnostics`, `dropped`. `--fail-on none` still reports `complete: true`, `ok: false` | none |
| Incomplete analysis | `$D "$FIXTURE" --config /nope.toml` | exit **2**, `deadwood: configuration file not found: /nope.toml`; a broken config never reports clean | none |
| Issue selection | `--include unused_file`; `--exclude unused_export` | baseline 12 findings across 5 kinds; `--include unused_file` leaves 4; `--exclude unused_export` leaves 11 | none |
| `--all` | `--all` on the entry-exports scratch | alias for `--include-entry-exports` (`src/main.rs:302`); no effect on the fixture (its entry file exports nothing) | none |
| Rules: severity and suppression | drive below | `[rules] unused_export = "warn"` prints `severity: warn` and keeps exit 0 under the default `--fail-on error`; `= "error"` exits 1; `unused_file = "off"` drops the finding | none |
| Reporters | `--format json\|text\|compact\|markdown\|sarif\|github-actions` | text: `unlisted_dependency src/extra_mod.rs:1 missing_crate [high]` (the bracket is **confidence**, not severity, `src/report.rs:447`); markdown starts `\| Rule \| File \| Name \| Severity \|`; SARIF `2.1.0` with 12 results; github-actions `::error file=…,line=…,title=…` | none |
| — `compact` | `--format compact` | **identical to `text`**: `Format::Text \| Format::Compact` share one renderer (`src/main.rs:395`), verified byte-identical | none |
| Token budget | `--max-tokens 4000\|300\|100` | `dropped.not_shown` grows as findings are suppressed by the budget: 12 findings / 0 dropped at the default 4000, 4 / 8 at 300, 0 / 12 at 100 | none |
| Traces | `--trace-file`, `--trace-export`, `--trace-dependency`, `--trace` | `traces[]` with `references`: `--trace-file src/leaf.ts` gives 3 traces (`src/index.ts`, `src/leaf.test.ts`); `--trace-export used` 1; `--trace-dependency unused-dep` 1; `--trace` 21 | none |
| Scope: tests | `--exclude-tests`; `--include-tests`; `--production` | baseline `roots 5`, `files 15`; `--exclude-tests` roots 4; `--production` roots 4 and files 14 (test file dropped) | none |
| Scope: workspaces | workspace scratch (`-W` is `--workspace`): `--strict`; `--strict -W a`; `--strict -W '!packages/a'` | `--strict` surfaces `unlisted_dependency shared`, `unused_dependency shared` and `packages/b`'s dead file; `-W a` leaves only `packages/a`'s finding; the `!` form leaves only `packages/b`'s | none |
| Scope: gitignore | drive below | inside a git work tree, an ignored `junk/j.ts` is excluded (15 files) and included with `--no-gitignore` (16); without `.git` it is included either way | **a git work tree**; avoid `vendor/`, `target/`, `node_modules/` — always skipped (`src/discover.rs:433`) |
| Native build profiles | `--no-default-features`; `--features extra`; `--all-features`; `--go-tags extra`; `--target linux/amd64` | `unused_files` 4 → **5** without the default feature (the cfg-gated module leaves the graph); `files` 15 → **16** with the Go build tag or with a linux target | none |
| Compiler-backed Go | `$D "$FIXTURE" --semantic-go --fail-on none` | exit **0**; `analysis[0]` is `language: Go`, `complete: true`, declarations/references/roots > 0, `profile` filled with `target` and `toolchain`; Go findings (`unused_file internal/dead/dead.go`, `unused_symbol leaf.go deadFunc`) come from the compiler | Go 1.26+ and cached adapter modules — doctor proves both |
| — Go failure modes | `--semantic-go` without a `go.mod`; a module with a compile error | exit **2**, `analysis[0].complete: false`, `diagnostics[].kind: semantic_error`, and **no** heuristic Go findings as a fallback | Go 1.26+ |
| — Go downloads | `--go-downloads` without `--semantic-go` | clap exits 2: `--semantic-go` is required | none |
| — Go timeout | `--go-timeout 1` ; `--go-timeout 0` | accepts `1..=3600`; `0` is rejected by clap (exit 2, stderr, no JSON). A one-second budget still completes on a warm cache, so it is not a failure probe | Go 1.26+ |
| — Go report shape on failure | the two failure modes above | exit 2 still prints a JSON report (`complete: false`, `analysis[0].profile: {}`, `diagnostics[]`); only clap argument errors leave stdout empty | none |
| Extension commands | `--preprocessor cat`; `--reporter cat`; `--preprocessor "printf '{}'"` | identity commands keep the 12 findings and exit 0; an invalid report exits **2** (`deadwood: invalid preprocessor report: missing field…`) even with `--fail-on none` | none |
| Configured compilers | compilers scratch, with and without `--allow-compilers` | exit **2** `custom compilers require --allow-compilers (they execute configured commands)`; with the flag `reachable_files: 2`, `unused_files: 0` | trusting a command the project configured |
| Guarded fixes | `--fix --dry-run`; `--fix`; `--fix --verify-command true`; `--fix --allow-remove-files` | dry run leaves the tree byte-identical (sha256 of a recursive listing) and lists `fixes[{file,action,names}]`; `--fix` rewrites `package.json` and writes `.deadwood-backup/<id>/`; the Rust dependency is edited **only** with `--verify-command`; only `--allow-remove-files` deletes `src/dead.ts` and `src/dead_mod.rs` | a disposable copy of the fixture |
| — fix refusal | `--fix` with a dynamic import present | exit **2** `deadwood: refusing fixes: analysis is incomplete or has unresolved references`; no file is touched | none |
| Cache | `--cache --cache-location DIR`, twice | JSON identical to the uncached run and to the second cached run; 17 files appear in DIR | writable DIR |
| Watch | `--watch --watch-count 2 --watch-interval 20` with a real edit | two JSON lines on stdout; after making `src/dead.ts` used, `unused_files` 4 → **3** and its finding disappears; exits 0 | a fixture copy; the hidden count flags (`src/main.rs:77`) end the loop |
| Diagnostics | `--debug --performance --duration --memory` | stderr only: `packages=3 files=15 roots=5 reachable=11`, `entry src/index.ts: conventional package entry`, `discover=… resolve=… report=…`, `duration=…`, `rss_kib=…`; stdout stays parseable JSON | none |
| Language coverage | one fixture scan | `unused_file` for `src/dead.ts`, `internal/dead/dead.go`, `src/dead_mod.rs`, `scripts/dead.lua`; `unused_symbol` for `leaf.go deadFunc`, `module.lua unused`, `src/extra_mod.rs extra`, `src/main.rs extra_mod`; `unused_export src/leaf.ts unusedExport`; `unused_dependency` for `package.json unused-dep` and `Cargo.toml serde` | none |
| Lua entry plugins | plugins scratch, with and without `plugins = ["love"]` | configured: `roots 1`, `unused_files 1`, complete; without it `conf.lua` stops being an entry, exit **2**, `complete: false`, diagnostics `no_entrypoints` | none |
| Entry-export boundary | entry-exports scratch, with and without `--include-entry-exports` | no findings by default; `unused_export api` when entry exports are checked | none |
| Repo-wide check | `.pi/verify/run.sh` (or `.pi/verify-clean`) | fmt, gofmt, `go vet`/`go test`, clippy, `cargo test`, release build, self-scan, Knip parity, workflow/release/package checks all pass; `self-scan ok: 31 files` | Node/npm for Knip, actionlint for workflows; `run.sh` deletes `target/` and rebuilds |

### Drives that need more than one command

```sh
# Rules, severity and suppression. The suppression comment applies to its own
# line AND the next one (src/report.rs:356-364), so space findings apart to see
# which one a comment actually controls.
S=$(mktemp -d); mkdir -p "$S/src"
printf '{"private":true}\n' >"$S/package.json"
printf "import { used } from './leaf';\nconsole.log(used);\n" >"$S/src/index.ts"
printf 'export const used = 1;\nexport const quiet = 2; // deadwood-ignore: unused_export\nexport const adjacent = 3;\nexport const spaced = 4;\n\nexport const survive = 5;\n' >"$S/src/leaf.ts"
printf 'export const dead = 1;\n' >"$S/src/dead.ts"
printf '[rules]\nunused_export = "warn"\n' >"$S/deadwood.toml"
$D "$S" --fail-on none
# Observes: findings for "spaced" and "survive" with severity "warn"; "quiet"
# (same line) and "adjacent" (next line) suppressed; default --fail-on exits 0.

# Fixes: prove nothing is written on a dry run, then what each flag adds.
C=$(mktemp -d); cp -R "$FIXTURE/." "$C/"
snap() { (cd "$1" && find . -type f | sort | xargs shasum -a 256 | shasum -a 256); }
before=$(snap "$C"); $D "$C" --fail-on none --fix --dry-run >/tmp/dry.json 2>&1
[ "$before" = "$(snap "$C")" ] && echo "dry run wrote nothing"
$D "$C" --fail-on none --fix >/dev/null 2>&1          # package.json edited, Cargo.toml not
grep -c serde "$C/Cargo.toml"                          # 1
$D "$C" --fail-on none --fix --verify-command true >/dev/null 2>&1
grep -c serde "$C/Cargo.toml"                          # 0

# gitignore: the directory must not be one deadwood always skips.
G=$(mktemp -d); cp -R "$FIXTURE/." "$G/"; mkdir -p "$G/junk"; echo 'export const j = 1;' >"$G/junk/j.ts"
(cd "$G" && git init -q . && printf 'junk/\n' >.gitignore)
$D "$G" --fail-on none | python3 -c 'import json,sys;print(json.load(sys.stdin)["summary"]["files"])'                    # 15
$D "$G" --fail-on none --no-gitignore | python3 -c 'import json,sys;print(json.load(sys.stdin)["summary"]["files"])'      # 16

# Watch: a second report only proves something if the edit changes the answer.
W=$(mktemp -d); cp -R "$FIXTURE/." "$W/"
( sleep 3; printf "import { dead } from './dead';\nconsole.log(dead);\n" >"$W/src/index.ts" ) &
$D "$W" --fail-on none --watch --watch-count 2 --watch-interval 20
# Observes: two JSON documents, unused_files 4 then 3, exit 0.
```

## Cleanup

```sh
.pi/verify/fixture.sh --remove "$FIXTURE"   # refuses anything not a deadwood-verify.* dir
.pi/verify/scratch.sh --remove "$S"         # one call per scratch project, same guard
```

The recipes above create their own `mktemp -d` directories; remove those too.
Cleanup covers the cache directory, the `.deadwood-backup` left by a `--fix`
drive and any `--watch` process — a watch drive started without
`--watch-count` never exits on its own, so kill it. No drive modifies the
repository or user configuration. Keep the JSON reports you asserted on, the
doctor output and the recorded revision as evidence.

## Known broken, and what this recipe does not cover

- **`[ignore_issues]` does not work** (measured; not fixed here, and not a
  reason to soften this recipe). The shape README documents at
  `README.md:127`, `"src/dead.ts" = ["unused_file"]`, exits 2 with
  `unknown issue type: src/dead.ts` because `validate_config` checks the
  map's *file* keys against `ISSUE_TYPES` (`src/discover.rs:515`). The
  inverted shape, `unused_file = ["src/dead.ts"]`, validates and exits 0 but
  never matches a finding. No test mentions `ignore_issues`. Use
  `deadwood-ignore` comments or `[rules] = "off"` until it is fixed.
- **`--format compact` is a synonym for `text`**, byte-identical output
  (`src/main.rs:395`). Either it is kept deliberately or the distinct compact
  renderer was never wired up; the recipe cannot tell which, so it must not
  claim the two differ.
- **A suppression comment leaks onto the next line.** `src/report.rs:356-364`
  tests both the finding's own line and the previous one, so
  `export const quiet = 2; // deadwood-ignore: unused_export` also hides an
  `unused_export` on the following line. Measured; harmless for the
  comment-above-the-code style it was written for, confusing otherwise.
- `.gitignore` is honoured only inside a git work tree: a copied project
  without `.git` ignores its `.gitignore` (measured). Verify gitignore
  behaviour in a git work tree or not at all.
- Not covered: editor/LSP/MCP integration (deadwood has none), real Go module
  downloads (`--go-downloads` is exercised only as an argument contract), cgo
  linking, Windows/other-platform behaviour beyond `--target` selection, and
  anything requiring a live network.
