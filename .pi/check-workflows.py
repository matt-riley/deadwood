#!/usr/bin/env python3
"""Check the reusable-workflow calls in .github/workflows.

actionlint validates syntax and action pins, but it cannot know what inputs a
reusable workflow accepts. A typo there is invisible until the release run
fails, so this asks the pinned commit what it declares and compares.

Reads the callee definition from GitHub, so it needs `gh` authenticated and a
network connection; it says so and passes when they are unavailable.
"""

import base64
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
WORKFLOWS = ROOT / ".github" / "workflows"

# Keys GitHub allows on a job that calls a reusable workflow.
# https://docs.github.com/en/actions/reference/workflows-and-actions/reusing-workflow-configurations
CALL_JOB_KEYS = {
    "name",
    "uses",
    "with",
    "secrets",
    "strategy",
    "needs",
    "if",
    "concurrency",
    "permissions",
    "cache-mode",
}

KEY = re.compile(r"^(?P<indent> *)(?P<key>[A-Za-z_][\w.-]*):(?P<rest>.*)$")


def indent_of(line: str) -> int:
    return len(line) - len(line.lstrip(" "))


def mapping_keys(lines: list[str], start: int, indent: int) -> set[str]:
    """Keys of the mapping that begins at `start`, one level in from `indent`."""
    keys: set[str] = set()
    for line in lines[start:]:
        if not line.strip() or line.lstrip().startswith("#"):
            continue
        if indent_of(line) <= indent:
            break
        match = KEY.match(line)
        if match and indent_of(line) == indent + 2:
            keys.add(match.group("key"))
    return keys


def section_keys(lines: list[str], name: str, indent: int) -> set[str]:
    """Keys of the `name:` mapping nested under a parent at `indent`."""
    for index, line in enumerate(lines):
        match = KEY.match(line)
        if match and indent_of(line) == indent and match.group("key") == name:
            return mapping_keys(lines, index + 1, indent)
    return set()


REPO = "matt-riley/matt-riley-ci"


def callee_source(workflow: str, sha: str) -> str | None:
    """The callee workflow at that commit, or None when gh is unavailable."""
    try:
        raw = subprocess.run(
            ["gh", "api", f"repos/{REPO}/contents/.github/workflows/{workflow}?ref={sha}", "--jq", ".content"],
            capture_output=True,
            text=True,
            check=True,
        ).stdout
    except FileNotFoundError:
        return None
    except subprocess.CalledProcessError as error:
        raise SystemExit(
            f"check-workflows: cannot resolve {workflow} at {sha[:8]}: "
            f"{(error.stderr or '').strip().splitlines()[:1]}"
        ) from error
    return base64.b64decode(raw).decode()


def call_contract(workflow: str, sha: str) -> tuple[set[str], set[str], set[str]]:
    """Inputs, secrets and outputs the callee declares at that commit."""
    lines = callee_source(workflow, sha).splitlines()
    # workflow_call sits at indent 2 under `on:`; its keys at indent 4.
    workflow_call = next(
        (i for i, l in enumerate(lines) if KEY.match(l) and l.strip().startswith("workflow_call:")),
        None,
    )
    if workflow_call is None:
        return set(), set(), set()
    body = lines[workflow_call:]
    return (
        section_keys(body, "inputs", 4),
        section_keys(body, "secrets", 4),
        section_keys(body, "outputs", 4),
    )


def calls() -> list[tuple[str, str, str, dict, str]]:
    """Every `uses: matt-riley/matt-riley-ci/...@sha` job, with its mapping."""
    found = []
    for workflow in sorted(WORKFLOWS.glob("*.y*ml")):
        text = workflow.read_text()
        lines = text.splitlines()
        for index, line in enumerate(lines):
            match = KEY.match(line)
            if not match or match.group("key") != "uses":
                continue
            target = match.group("rest").strip().strip("'\"")
            # Drop a trailing `# v4.0.0` style comment from the ref.
            target = re.split(r"\s+#", target, maxsplit=1)[0].strip()
            callee, _, sha = target.partition("@")
            if not callee.startswith("matt-riley/matt-riley-ci/"):
                continue
            indent = indent_of(line)
            job = {key: section_keys(lines[index:], key, indent) for key in ("with", "secrets")}
            found.append(
                (workflow.name, callee.split("/.github/workflows/")[-1], sha, job, text)
            )
    return found


def referenced_outputs(text: str, job_name: str) -> set[str]:
    """`needs.<job>.outputs.<name>` read anywhere in a caller."""
    pattern = rf"needs\.{re.escape(job_name)}\.outputs\.([\w-]+)"
    return set(re.findall(pattern, text))


def callee_job_name(text: str, callee: str, sha: str) -> str:
    """The job id of the call that targets `callee@sha`, so needs.* can be resolved."""
    lines = text.splitlines()
    current = ""
    for line in lines:
        match = KEY.match(line)
        if not match:
            continue
        if indent_of(line) == 2:
            current = match.group("key")
        if match.group("key") == "uses" and callee in line and sha in line:
            return current
    return ""


def main() -> int:
    try:
        found = calls()
    except subprocess.CalledProcessError as error:
        print(f"check-workflows: gh query failed: {error}")
        return 1

    if not found:
        print("check-workflows: no reusable-workflow calls found")
        return 0

    failures: list[str] = []
    for filename, callee, sha, job, text in found:
        if not re.fullmatch(r"[0-9a-f]{40}", sha):
            failures.append(f"{filename}: {callee} is not pinned to a full commit SHA ({sha})")
            continue
        inputs, secrets, outputs = call_contract(callee, sha)
        for name in sorted(job["with"] - inputs):
            failures.append(
                f"{filename}: input '{name}' is not declared by {callee}"
                f" at {sha[:8]} (declares {len(inputs)})"
            )
        for name in sorted(job["secrets"] - secrets):
            failures.append(
                f"{filename}: secret '{name}' is not declared by {callee}"
                f" at {sha[:8]} (declares {len(secrets)})"
            )
        # An unknown output does not error in GitHub, it resolves empty — which
        # would silently skip every job gated on it.
        job_name = callee_job_name(text, callee, sha)
        for name in sorted(referenced_outputs(text, job_name) - outputs):
            failures.append(
                f"{filename}: needs.{job_name}.outputs.{name} is not declared by"
                f" {callee} at {sha[:8]} (declares {sorted(outputs) or 'none'})"
            )

    for filename, callee, sha, job, _ in found:
        print(
            f"  {filename:12} {callee:26} {len(job['with']):2} inputs, "
            f"{len(job['secrets']):2} secrets @ {sha[:8]}"
        )

    if failures:
        print("\ncheck-workflows failed:")
        for failure in failures:
            print(" -", failure)
        return 1
    print(f"\n{len(found)} reusable calls checked")
    return 0


if __name__ == "__main__":
    sys.exit(main())
