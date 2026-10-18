#!/usr/bin/env sh
# Health check for a deadwood verification run.
# Run it before the first drive and again after anything surprising.
set -eu
cd "$(dirname "$0")/../.."

problems=0
fail() { echo "doctor: FAIL $1"; problems=$((problems + 1)); }
ok() { echo "doctor: ok   $1"; }

# 1. The binary under test must be this revision, not a stale build.
if ! command -v cargo >/dev/null 2>&1; then
    fail "cargo is not on PATH"
elif ! cargo build --release --locked >/tmp/deadwood-doctor-build.log 2>&1; then
    fail "cargo build --release --locked failed: tail of /tmp/deadwood-doctor-build.log"
    tail -5 /tmp/deadwood-doctor-build.log
elif [ ! -x target/release/deadwood ]; then
    fail "target/release/deadwood missing after a successful build"
else
    expected=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
    built=$(target/release/deadwood --version | awk '{print $2}')
    if [ "$built" = "$expected" ]; then
        ok "binary version $built matches Cargo.toml"
    else
        fail "binary reports $built but Cargo.toml is $expected"
    fi
fi

# 2. Compiler-backed Go analysis needs a toolchain and a warm offline module cache.
if ! command -v go >/dev/null 2>&1; then
    fail "go is not on PATH (--semantic-go drives need Go 1.26+)"
else
    version=$(go env GOVERSION | sed 's/^go//' | cut -d. -f1-2)
    major=$(echo "$version" | cut -d. -f1)
    minor=$(echo "$version" | cut -d. -f2)
    if [ "$major" -gt 1 ] || { [ "$major" -eq 1 ] && [ "$minor" -ge 26 ]; }; then
        ok "go $version"
    else
        fail "go $version is older than the 1.26 the adapter requires"
    fi
    # Offline on purpose: a cached cache passes, a missing dependency is reported here
    # rather than as a confusing adapter timeout later.
    # vet, not build: `go build ./...` in a main package writes a binary into the
    # repository, and the doctor must leave the tree exactly as it found it.
    if (cd backends/go && GOFLAGS=-mod=readonly GOPROXY=off go vet ./...) >/tmp/deadwood-doctor-go.log 2>&1; then
        ok "adapter dependencies are cached for offline builds"
    else
        fail "adapter dependencies are missing offline; run: mise run install"
    fi
fi

# 3. End-to-end: the adapter must still produce a complete Go graph on a fixture.
if [ "$problems" -eq 0 ]; then
    fixture=$(.pi/verify/fixture.sh)
    trap '.pi/verify/fixture.sh --remove "$fixture"' EXIT
    if target/release/deadwood "$fixture" --semantic-go --fail-on none >/tmp/deadwood-doctor-scan.json 2>/tmp/deadwood-doctor-scan.err; then
        python3 -c '
import json, sys
doc = json.load(open("/tmp/deadwood-doctor-scan.json"))
analysis = doc["analysis"][0]
assert doc["complete"], doc["diagnostics"]
assert analysis["language"] == "Go" and analysis["complete"], analysis
assert analysis["declarations"] > 0 and analysis["roots"] > 0, analysis
print("doctor: ok   --semantic-go fixture scan: "
      f"{analysis["declarations"]} declarations, {analysis["references"]} references, "
      f"{analysis["roots"]} roots, toolchain {analysis["profile"].get("toolchain")}")
' || fail "adapter produced a report that failed the completeness assertions"
    else
        fail "adapter scan exited non-zero: $(tail -2 /tmp/deadwood-doctor-scan.err)"
    fi
fi

if [ "$problems" -ne 0 ]; then
    echo "doctor: $problems problem(s); drives below would be misleading"
    exit 1
fi
echo "doctor: ready"
