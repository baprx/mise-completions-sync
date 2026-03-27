#!/usr/bin/env python3
# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""
Validates registry.toml entries by testing completion commands.

Usage: uv run scripts/validate-registry.py [--installed-only]

Tests each tool's completion command to verify it works. By default tests
all entries; use --installed-only to skip tools not installed via mise.
"""

import os
import subprocess
import sys
from pathlib import Path

import tomllib


def get_installed_tools() -> set[str]:
    """Get list of tools installed via mise."""
    result = subprocess.run(
        ["mise", "ls", "--installed", "--json"],
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        print(f"Warning: couldn't get mise tools: {result.stderr}", file=sys.stderr)
        return set()

    import json

    try:
        data = json.loads(result.stdout)
        return set(data.keys())
    except json.JSONDecodeError:
        return set()


def load_registry() -> dict[str, dict[str, dict]]:
    """Load registry.toml and expand patterns to get tool completions.

    Returns a dict with tool names as keys and values containing shell-specific
    completion info: {shell: {"command": str, "env": dict or None}}
    """
    registry_path = Path(__file__).parent.parent / "registry.toml"
    with open(registry_path, "rb") as f:
        raw = tomllib.load(f)

    patterns = raw.get("patterns", {})
    tools_raw = raw.get("tools", {})

    expanded = {}
    for tool_name, entry in tools_raw.items():
        if isinstance(entry, str):
            # Pattern reference
            pattern = patterns.get(entry)
            if pattern is None:
                print(
                    f"Warning: unknown pattern '{entry}' for tool '{tool_name}'",
                    file=sys.stderr,
                )
                continue
            # Expand {} placeholder with tool name
            expanded[tool_name] = {
                shell: {"command": cmd.replace("{}", tool_name), "env": None}
                for shell, cmd in pattern.items()
            }
        else:
            # Explicit commands - handle both string and object formats
            expanded[tool_name] = {}
            for shell, value in entry.items():
                if isinstance(value, str):
                    # Simple string format
                    expanded[tool_name][shell] = {"command": value, "env": None}
                elif isinstance(value, dict):
                    # Object format with command and env
                    expanded[tool_name][shell] = {
                        "command": value.get("command", ""),
                        "env": value.get("env"),
                    }
                else:
                    print(
                        f"Warning: invalid format for {tool_name}.{shell}",
                        file=sys.stderr,
                    )

    return expanded


def test_completion(
    tool: str, shell: str, command: str, env_vars: dict[str, str] | None = None
) -> tuple[bool, str]:
    """Test a completion command. Returns (success, error_message)."""
    wrapped = f"mise x {tool} -- {command}"

    # Build environment with optional vars
    env = None
    if env_vars:
        env = os.environ.copy()
        env.update(env_vars)

    result = subprocess.run(
        ["sh", "-c", wrapped],
        capture_output=True,
        text=True,
        timeout=30,
        env=env,
    )

    if result.returncode == 0 and result.stdout.strip():
        return True, ""

    error = result.stderr.strip() or result.stdout.strip() or "empty output"
    return False, error


def main():
    installed_only = "--installed-only" in sys.argv

    registry = load_registry()
    installed = get_installed_tools() if installed_only else set()

    results: dict[str, dict[str, tuple[bool, str]]] = {}

    tools = sorted(registry.keys())
    total = len(tools)

    print(f"Validating {total} tools...\n")

    for i, tool in enumerate(tools, 1):
        if installed_only and tool not in installed:
            continue

        completions = registry[tool]
        results[tool] = {}

        print(f"[{i}/{total}] {tool}...", end=" ", flush=True)
        tool_ok = True

        for shell, completion_data in completions.items():
            command = completion_data["command"]
            env_vars = completion_data.get("env")

            try:
                ok, err = test_completion(tool, shell, command, env_vars)
                results[tool][shell] = (ok, err)
                if not ok:
                    tool_ok = False
            except subprocess.TimeoutExpired:
                results[tool][shell] = (False, "timeout")
                tool_ok = False
            except Exception as e:
                results[tool][shell] = (False, str(e))
                tool_ok = False

        print("✓" if tool_ok else "✗")

    # Summary
    print("\n" + "=" * 60)
    print("SUMMARY")
    print("=" * 60)

    failures: dict[str, list[tuple[str, str, str]]] = {}
    successes = 0
    total_tests = 0

    for tool, shell_results in results.items():
        for shell, (ok, err) in shell_results.items():
            total_tests += 1
            if ok:
                successes += 1
            else:
                if tool not in failures:
                    failures[tool] = []
                # Get the original command for display
                cmd = registry[tool][shell]["command"]
                failures[tool].append((shell, cmd, err))

    print(f"\nPassed: {successes}/{total_tests}")

    if failures:
        print(f"\nFailed tools ({len(failures)}):\n")
        for tool, errs in sorted(failures.items()):
            print(f"  [{tool}]")
            for shell, cmd, err in errs:
                # Truncate long errors
                err_short = err[:60] + "..." if len(err) > 60 else err
                print(f"    {shell}: {err_short}")
            print()

    return 0 if not failures else 1


if __name__ == "__main__":
    sys.exit(main())
