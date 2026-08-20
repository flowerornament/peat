#!/usr/bin/env python3
"""Release helper for peat: bump / verify / notes / tag.

A slim port of the nx-rs/anneal release machinery, minus their Nix-cache
half (peat publishes GitHub Release binaries only) and jj-aware where they
are git-aware. Version-bearing files: Cargo.toml, Cargo.lock, flake.nix
(peatVersion), CHANGELOG.md. `just release <v>` runs `tag`, which runs the
full `verify` first; nothing tags unverified.
"""

from __future__ import annotations

import argparse
import re
import subprocess
import sys
import tomllib
from datetime import date
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SEMVER_RE = re.compile(r"^\d+\.\d+\.\d+$")

# CHANGELOG headings look like `## 0.1.0 — 2026-08-18` (no v prefix, em
# dash) — the same shape release.yml's awk extracts GitHub notes from.
CHANGELOG_INTRO_MARKER = (
    "All notable changes to peat. The ledger is the API to our past; "
    "so is this file.\n\n"
)


def fail(message: str) -> None:
    print(f"error: {message}", file=sys.stderr)
    raise SystemExit(1)


def run(cmd: list[str]) -> None:
    print(f"+ {' '.join(cmd)}", flush=True)
    subprocess.run(cmd, cwd=ROOT, check=True)


def capture(cmd: list[str]) -> str:
    result = subprocess.run(
        cmd, cwd=ROOT, check=True, stdout=subprocess.PIPE, text=True
    )
    return result.stdout.strip()


# ------------------------------------------------------------- versions


def cargo_toml_version() -> str:
    data = tomllib.loads((ROOT / "Cargo.toml").read_text())
    return data["package"]["version"]


def cargo_lock_version() -> str:
    text = (ROOT / "Cargo.lock").read_text()
    match = re.search(r'name = "peat"\nversion = "([^"]+)"', text)
    if match is None:
        fail("could not find the peat package entry in Cargo.lock")
    return match.group(1)


def flake_version() -> str:
    text = (ROOT / "flake.nix").read_text()
    match = re.search(r'(?m)^\s*peatVersion = "([^"]+)";$', text)
    if match is None:
        fail("could not find peatVersion in flake.nix")
    return match.group(1)


# -------------------------------------------------------------- targets


def workflow_targets() -> list[str]:
    text = (ROOT / ".github/workflows/release.yml").read_text()
    return re.findall(r"- target: ([^\n]+)", text)


def installer_targets() -> list[str]:
    text = (ROOT / "install.sh").read_text()
    match = re.search(
        r"SUPPORTED_RELEASE_TARGETS=\(\n(?P<body>(?:\s+\"[^\"]+\"\n)+)\)", text
    )
    if match is None:
        fail("could not find SUPPORTED_RELEASE_TARGETS in install.sh")
    return re.findall(r'"([^"]+)"', match.group("body"))


# ------------------------------------------------------------ changelog
# Pure text functions (unit-tested in test_release.py); file IO stays in
# the thin wrappers below them.


def changelog_heading_re(version: str) -> str:
    return rf"(?m)^## {re.escape(version)} — \d{{4}}-\d{{2}}-\d{{2}}$"


def changelog_has_entry(text: str, version: str) -> bool:
    return re.search(changelog_heading_re(version), text) is not None


def changelog_entry_text(text: str, version: str) -> str:
    heading = re.search(changelog_heading_re(version), text)
    if heading is None:
        raise ValueError(f"CHANGELOG.md has no entry for {version}")
    next_heading = re.search(r"(?m)^## ", text[heading.end() :])
    end = len(text) if next_heading is None else heading.end() + next_heading.start()
    entry = text[heading.end() : end].strip()
    if not entry:
        raise ValueError(f"CHANGELOG.md entry for {version} is empty")
    return f"{entry}\n"


def changelog_entry_is_ready(text: str, version: str) -> bool:
    try:
        entry = changelog_entry_text(text, version)
    except ValueError:
        return False
    if "TODO" in entry or "TBD" in entry:
        return False
    return re.search(r"(?m)^- ", entry) is not None


def changelog_insert_scaffold(text: str, version: str, today: str) -> str:
    if changelog_has_entry(text, version):
        return text
    # an `## Unreleased` section is notes accumulated between releases —
    # bump retitles it to the new version instead of scaffolding a TODO
    unreleased, count = re.subn(
        r"(?m)^## Unreleased[ \t]*$", f"## {version} — {today}", text
    )
    if count == 1:
        return unreleased
    if count > 1:
        raise ValueError("CHANGELOG.md has more than one ## Unreleased section")
    # newest-first: the scaffold goes right before the newest section, after
    # the intro prose (which may span several paragraphs)
    first = re.search(r"(?m)^## ", text)
    if first is None:
        raise ValueError("could not find a CHANGELOG.md section to insert before")
    scaffold = f"## {version} — {today}\n\n- TODO: summarize release changes.\n\n"
    return text[: first.start()] + scaffold + text[first.start() :]


def replace_once(text: str, pattern: str, replacement: str) -> str:
    # count matches before editing: subn(count=1) caps its count at 1, so
    # it cannot see an ambiguous pattern that matched more than once
    matches = list(re.finditer(pattern, text, flags=re.MULTILINE))
    if len(matches) != 1:
        raise ValueError(
            f"pattern matched {len(matches)} times, expected exactly once: {pattern}"
        )
    return re.sub(pattern, replacement, text, count=1, flags=re.MULTILINE)


# ---------------------------------------------------------------- verbs


def bump(version: str) -> None:
    if SEMVER_RE.fullmatch(version) is None:
        fail("version must be semver like 0.1.1")
    try:
        for path, pattern, replacement in [
            (ROOT / "Cargo.toml", r'^version = "[^"]+"$', f'version = "{version}"'),
            (
                ROOT / "Cargo.lock",
                r'(name = "peat"\nversion = )"[^"]+"',
                rf'\1"{version}"',
            ),
            (
                ROOT / "flake.nix",
                r'^(\s*)peatVersion = "[^"]+";$',
                rf'\1peatVersion = "{version}";',
            ),
        ]:
            path.write_text(replace_once(path.read_text(), pattern, replacement))
        changelog = ROOT / "CHANGELOG.md"
        changelog.write_text(
            changelog_insert_scaffold(
                changelog.read_text(), version, date.today().isoformat()
            )
        )
    except ValueError as error:
        fail(str(error))
    print(f"set release version to {version} in:")
    for name in ("Cargo.toml", "Cargo.lock", "flake.nix", "CHANGELOG.md"):
        print(f"  - {name}")
    print("now: replace the CHANGELOG TODO with real notes, then `just release-verify`")


def working_copy_is_clean() -> bool:
    return capture(["jj", "log", "--no-graph", "-r", "@ & ~empty()", "-T", "commit_id"]) == ""


def verify() -> str:
    # the machinery proves itself before it judges anything else
    run([sys.executable, "scripts/test_release.py"])

    versions = {
        "Cargo.toml": cargo_toml_version(),
        "Cargo.lock": cargo_lock_version(),
        "flake.nix": flake_version(),
    }
    if len(set(versions.values())) != 1:
        fail(
            "release versions do not match: "
            + ", ".join(f"{k}={v}" for k, v in versions.items())
            + " — run `just release-bump <version>`"
        )
    version = versions["Cargo.toml"]

    if not changelog_entry_is_ready((ROOT / "CHANGELOG.md").read_text(), version):
        fail(
            f"CHANGELOG.md needs a `## {version} — <date>` entry with at "
            "least one bullet and no TODO/TBD placeholders"
        )

    workflow = workflow_targets()
    installer = installer_targets()
    if workflow != installer:
        fail(
            "release targets differ between release.yml and install.sh: "
            f"workflow={workflow}, installer={installer}"
        )

    if not working_copy_is_clean():
        fail("jj working copy is not empty — land first")

    run(["just", "check"])
    run(["cargo", "build", "--release"])
    reported = capture(["./target/release/peat", "--version"])
    if version not in reported:
        fail(f"release binary reports {reported!r}, expected {version}")

    print(f"release verification passed for {version}")
    print(f"release targets: {', '.join(workflow)}")
    return version


def tag(version: str) -> None:
    if SEMVER_RE.fullmatch(version) is None:
        fail("version must be semver like 0.1.1")
    verified = verify()
    if verified != version:
        fail(f"repo is at version {verified}, expected {version}")

    tag_name = f"v{version}"
    if capture(["git", "tag", "--list", tag_name]):
        fail(f"tag {tag_name} already exists")

    # tag what is published: local main must be what origin serves
    local_main = capture(["jj", "log", "--no-graph", "-r", "main", "-T", "commit_id"])
    remote = capture(["git", "ls-remote", "origin", "refs/heads/main"])
    if not remote.startswith(local_main):
        fail("local main differs from origin/main — `just land` first")

    run(["git", "tag", "-a", tag_name, "-m", f"peat {tag_name}", "main"])
    run(["git", "push", "origin", tag_name])
    # `release` is the moving branch release-tracking flake inputs follow
    run(["git", "push", "origin", "main:release", "--force-with-lease"])
    print(f"released {tag_name} — release branch moved; CI publishes binaries")


def notes(version_or_tag: str) -> None:
    version = version_or_tag.removeprefix("v")
    if SEMVER_RE.fullmatch(version) is None:
        fail("version must be semver like 0.1.1 or a tag like v0.1.1")
    try:
        print(changelog_entry_text((ROOT / "CHANGELOG.md").read_text(), version), end="")
    except ValueError as error:
        fail(str(error))


def main() -> None:
    parser = argparse.ArgumentParser(description="Release helper for peat")
    sub = parser.add_subparsers(dest="command", required=True)
    sub.add_parser("bump", help="set the release version everywhere").add_argument(
        "version"
    )
    sub.add_parser("verify", help="run release readiness checks")
    sub.add_parser("tag", help="verify, tag, and publish a release").add_argument(
        "version"
    )
    sub.add_parser(
        "notes", help="render one CHANGELOG entry as GitHub release notes"
    ).add_argument("version")

    args = parser.parse_args()
    if args.command == "bump":
        bump(args.version)
    elif args.command == "verify":
        verify()
    elif args.command == "tag":
        tag(args.version)
    else:
        notes(args.version)


if __name__ == "__main__":
    main()
