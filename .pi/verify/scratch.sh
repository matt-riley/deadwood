#!/usr/bin/env sh
# Tiny single-purpose projects for the deadwood features the four-language
# fixture cannot express. Prints the project path.
#
#   scratch.sh <compilers|entry-exports|plugins|workspace>
#   scratch.sh --remove DIR
set -eu

remove() {
    case "$1" in
        */deadwood-scratch.*) rm -rf -- "$1" ;;
        *) echo "refusing to remove $1: not a deadwood-scratch project" >&2; exit 2 ;;
    esac
}

case "${1:-}" in
    --remove)
        [ $# -eq 2 ] || { echo "usage: scratch.sh --remove DIR" >&2; exit 2; }
        remove "$2"
        exit 0
        ;;
    compilers | entry-exports | plugins | workspace) ;;
    *) echo "usage: scratch.sh <compilers|entry-exports|plugins|workspace>" >&2; exit 2 ;;
esac

root=$(mktemp -d "${TMPDIR:-/tmp}/deadwood-scratch.XXXXXX")
mkdir -p "$root/src"

case "$1" in
    compilers)
        # A non-source extension resolved through a trusted external command.
        cat >"$root/deadwood.toml" <<'EOF'
entries = ["src/index.widget"]

[compilers]
widget = "cat"
EOF
        cat >"$root/src/index.widget" <<'EOF'
import { used } from './leaf';
console.log(used);
EOF
        cat >"$root/src/leaf.ts" <<'EOF'
export const used = 1;
EOF
        ;;
    entry-exports)
        # A configured entry whose exports are an API boundary by default.
        cat >"$root/package.json" <<'EOF'
{"private":true}
EOF
        cat >"$root/deadwood.toml" <<'EOF'
entries = ["src/index.ts"]
EOF
        cat >"$root/src/index.ts" <<'EOF'
export const api = 1;
EOF
        ;;
    plugins)
        # LÖVE conventions only activate when the plugin is configured.
        cat >"$root/deadwood.toml" <<'EOF'
plugins = ["love"]
EOF
        cat >"$root/conf.lua" <<'EOF'
function love.draw() end
EOF
        cat >"$root/dead.lua" <<'EOF'
return 1
EOF
        ;;
    workspace)
        mkdir -p "$root/packages/a/src" "$root/packages/b/src"
        cat >"$root/package.json" <<'EOF'
{"private":true,"workspaces":["packages/*"],"dependencies":{"shared":"*"}}
EOF
        cat >"$root/packages/a/package.json" <<'EOF'
{"name":"a"}
EOF
        cat >"$root/packages/a/src/index.ts" <<'EOF'
import 'shared';
EOF
        cat >"$root/packages/b/package.json" <<'EOF'
{"name":"b"}
EOF
        cat >"$root/packages/b/src/index.ts" <<'EOF'
console.log('b');
EOF
        cat >"$root/packages/b/src/dead.ts" <<'EOF'
export const dead = 1;
EOF
        ;;
esac

echo "$root"
