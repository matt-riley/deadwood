#!/usr/bin/env sh
# Throwaway four-language project for driving the real deadwood binary.
# Never point drives at a real repository: --fix deletes files.
#
#   fixture.sh              create a fixture and print its path
#   fixture.sh --remove DIR delete a fixture created by this script
set -eu

case "${1:-}" in
    --remove)
        [ $# -eq 2 ] || { echo "usage: fixture.sh --remove DIR" >&2; exit 2; }
        case "$2" in
            */deadwood-verify.*) rm -rf -- "$2" ;;
            *) echo "refusing to remove $2: not a deadwood-verify fixture" >&2; exit 2 ;;
        esac
        exit 0
        ;;
    "") ;;
    *) echo "usage: fixture.sh [--remove DIR]" >&2; exit 2 ;;
esac

root=$(mktemp -d "${TMPDIR:-/tmp}/deadwood-verify.XXXXXX")
mkdir -p "$root/src" "$root/internal/dead" "$root/scripts"

cat >"$root/package.json" <<'EOF'
{"private":true,"dependencies":{"unused-dep":"*"},"devDependencies":{"vitest":"*"}}
EOF

cat >"$root/src/index.ts" <<'EOF'
import { used } from './leaf';
console.log(used);
EOF

cat >"$root/src/leaf.ts" <<'EOF'
export const used = 1;
export const unusedExport = 2;
EOF

# Test scope: dropped by --exclude-tests and --production.
cat >"$root/src/leaf.test.ts" <<'EOF'
import { test } from 'vitest';
import { used } from './leaf';

test('leaf', () => used);
EOF

# Never imported: the TypeScript unused-file case.
cat >"$root/src/dead.ts" <<'EOF'
export const dead = 1;
EOF

cat >"$root/Cargo.toml" <<'EOF'
[package]
name = "verify-fixture"
version = "0.0.0"
edition = "2021"

[[bin]]
name = "verify-fixture"
path = "src/main.rs"

[features]
default = ["extra"]
extra = []

[dependencies]
serde = "1"
EOF

cat >"$root/src/main.rs" <<'EOF'
mod used_mod;

#[cfg(feature = "extra")]
mod extra_mod;

fn main() {
    used_mod::used();
}
EOF

# Only reachable with the "extra" feature: the Rust cfg-profile case.
cat >"$root/src/extra_mod.rs" <<'EOF'
use missing_crate::Thing;

pub fn extra() {}
EOF

cat >"$root/src/used_mod.rs" <<'EOF'
pub fn used() {}
EOF

# Never declared: the Rust unused-file case.
cat >"$root/src/dead_mod.rs" <<'EOF'
pub fn dead() {}
EOF

cat >"$root/go.mod" <<'EOF'
module fixture.test/app

go 1.24
EOF

cat >"$root/main.go" <<'EOF'
package main

func main() {
	helper()
}
EOF

cat >"$root/leaf.go" <<'EOF'
package main

func helper() {}

func deadFunc() {}
EOF

# Only scanned when --target selects linux: the Go OS-suffix case.
cat >"$root/platform_linux.go" <<'EOF'
package main

func linuxOnly() {}
EOF

# Only scanned with --go-tags extra: the Go build-profile case.
cat >"$root/tagged.go" <<'EOF'
//go:build extra

package main

func tagged() {}
EOF

# Never imported: the Go unused-package case.
cat >"$root/internal/dead/dead.go" <<'EOF'
package dead

func Unused() {}
EOF

cat >"$root/main.lua" <<'EOF'
local m = require('module')
m.used()
dofile('scripts/setup.lua')
EOF

cat >"$root/module.lua" <<'EOF'
local M = {}
function M.used() return 1 end
function M.unused() return 2 end
return M
EOF

cat >"$root/scripts/setup.lua" <<'EOF'
print('setup')
EOF

# Never required or dofile'd: the Lua unused-file case.
cat >"$root/scripts/dead.lua" <<'EOF'
print('dead')
EOF

echo "$root"
