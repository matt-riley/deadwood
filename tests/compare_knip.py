#!/usr/bin/env python3
"""Compare Deadwood's TypeScript file/dependency findings with Knip."""

from __future__ import annotations

import json
import os
import subprocess
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
FIXTURE = ROOT / "tests" / "fixtures" / "knip-ts"
KNIP_VERSION = "6.41.0"
DEADWOOD = Path(
    os.environ.get("DEADWOOD_BIN", str(ROOT / "target" / "release" / "deadwood"))
)


def run_json(command: list[str], cwd: Path) -> dict:
    result = subprocess.run(command, cwd=cwd, text=True, capture_output=True)
    try:
        document = json.loads(result.stdout)
    except json.JSONDecodeError as error:
        raise SystemExit(
            f"command did not produce JSON (exit {result.returncode}): "
            f"{' '.join(command)}\nstdout={result.stdout!r}\nstderr={result.stderr!r}"
        ) from error
    return document


def main() -> int:
    deadwood = run_json(
        [str(DEADWOOD), str(FIXTURE), "--max-tokens", "10000"],
        ROOT,
    )
    knip = run_json(
        [
            "npm",
            "exec",
            "--yes",
            f"--package=knip@{KNIP_VERSION}",
            "--",
            "knip",
            "--reporter",
            "json",
        ],
        FIXTURE,
    )

    deadwood_files = {
        finding["file"]
        for finding in deadwood["findings"]
        if finding["kind"] == "unused_file"
    }
    deadwood_dependencies = {
        finding["name"]
        for finding in deadwood["findings"]
        if finding["kind"] == "unused_dependency"
    }
    def deadwood_set(kind):
        return {
            (finding["file"], finding["name"])
            for finding in deadwood["findings"]
            if finding["kind"] == kind
        }

    deadwood_exports = deadwood_set("unused_export")
    deadwood_types = deadwood_set("unused_type")
    deadwood_enum_members = deadwood_set("unused_enum_member")
    deadwood_namespace_members = deadwood_set("unused_namespace_member")
    deadwood_unlisted = deadwood_set("unlisted_dependency")
    deadwood_binaries = deadwood_set("unlisted_binary")
    deadwood_unresolved = deadwood_set("unresolved")
    deadwood_optional_peers = deadwood_set("optional_peer_dependency")
    deadwood_duplicates = deadwood_set("duplicate_export")
    deadwood_catalog = deadwood_set("unused_catalog")
    deadwood_catalog_references = deadwood_set("catalog_reference")
    deadwood_dev_dependencies = {
        finding["name"]
        for finding in deadwood["findings"]
        if finding["kind"] == "unused_dev_dependency"
    }

    knip_files = {
        issue["file"]
        for issue in knip["issues"]
        if issue.get("file") and issue.get("files")
    }
    knip_dependencies = {
        dependency["name"]
        for issue in knip["issues"]
        for dependency in issue.get("dependencies", [])
    }
    knip_dev_dependencies = {
        dependency["name"]
        for issue in knip["issues"]
        for dependency in issue.get("devDependencies", [])
    }
    knip_exports = {
        (issue["file"], export["name"])
        for issue in knip["issues"]
        for export in issue.get("exports", [])
    }
    knip_types = {
        (issue["file"], item["name"])
        for issue in knip["issues"]
        for item in issue.get("types", [])
    }
    knip_enum_members = {
        (issue["file"], f"{item['namespace']}.{item['name']}")
        for issue in knip["issues"]
        for item in issue.get("enumMembers", [])
    }
    knip_namespace_members = {
        (issue["file"], f"{item['namespace']}.{item['name']}")
        for issue in knip["issues"]
        for item in issue.get("namespaceMembers", [])
    }
    knip_unlisted = {
        (issue["file"], item["name"])
        for issue in knip["issues"]
        for item in issue.get("unlisted", [])
    }
    knip_binaries = {
        (issue["file"], item["name"])
        for issue in knip["issues"]
        for item in issue.get("binaries", [])
    }
    knip_unresolved = {
        (issue["file"], item["name"])
        for issue in knip["issues"]
        for item in issue.get("unresolved", [])
    }
    knip_optional_peers = {
        (issue["file"], item["name"])
        for issue in knip["issues"]
        for item in issue.get("optionalPeerDependencies", [])
    }
    knip_duplicates = {
        (issue["file"], "|".join(item["name"] for item in group))
        for issue in knip["issues"]
        for group in issue.get("duplicates", [])
    }
    knip_catalog = {
        (issue["file"], item["name"])
        for issue in knip["issues"]
        for item in issue.get("catalog", [])
    }
    knip_catalog_references = {
        (issue["file"], item["name"])
        for issue in knip["issues"]
        for item in issue.get("catalogReferences", [])
    }

    if deadwood_files != knip_files:
        raise SystemExit(
            f"TypeScript unused-file mismatch: deadwood={sorted(deadwood_files)}, "
            f"knip={sorted(knip_files)}"
        )
    if deadwood_dependencies != knip_dependencies:
        raise SystemExit(
            f"TypeScript unused-dependency mismatch: "
            f"deadwood={sorted(deadwood_dependencies)}, "
            f"knip={sorted(knip_dependencies)}"
        )
    comparisons = [
        ("unused-export", deadwood_exports, knip_exports),
        ("unused-type", deadwood_types, knip_types),
        ("enum-member", deadwood_enum_members, knip_enum_members),
        ("namespace-member", deadwood_namespace_members, knip_namespace_members),
        ("unlisted-dependency", deadwood_unlisted, knip_unlisted),
        ("unlisted-binary", deadwood_binaries, knip_binaries),
        ("unresolved", deadwood_unresolved, knip_unresolved),
        ("optional-peer-dependency", deadwood_optional_peers, knip_optional_peers),
        ("duplicates", deadwood_duplicates, knip_duplicates),
        ("catalog", deadwood_catalog, knip_catalog),
        ("catalog-reference", deadwood_catalog_references, knip_catalog_references),
        ("unused-dev-dependency", deadwood_dev_dependencies, knip_dev_dependencies),
    ]
    for label, actual, expected in comparisons:
        if actual != expected:
            raise SystemExit(
                f"TypeScript {label} mismatch: deadwood={sorted(actual)}, "
                f"knip={sorted(expected)}"
            )

    print(
        "Knip comparison passed: "
        f"files={sorted(deadwood_files)}, "
        f"dependencies={sorted(deadwood_dependencies)}, "
        f"devDependencies={sorted(deadwood_dev_dependencies)}, "
        f"exports={sorted(deadwood_exports)}, "
        f"types={sorted(deadwood_types)}, "
        f"enumMembers={sorted(deadwood_enum_members)}, "
        f"namespaceMembers={sorted(deadwood_namespace_members)}, "
        f"unlisted={sorted(deadwood_unlisted)}, "
        f"binaries={sorted(deadwood_binaries)}, "
        f"unresolved={sorted(deadwood_unresolved)}, "
        f"optionalPeers={sorted(deadwood_optional_peers)}, "
        f"duplicates={sorted(deadwood_duplicates)}, "
        f"catalog={sorted(deadwood_catalog)}, "
        f"catalogReferences={sorted(deadwood_catalog_references)}"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
