#!/usr/bin/env python3
"""Release helper for peat: bump / verify / notes / tag.

A slim port of the nx-rs/anneal release machinery, jj-aware where they are
git-aware. CI publishes GitHub Release binaries on the tag, and
nix-cache.yml pushes the aarch64-darwin flake output to Cachix when
`release` moves. Version-bearing files: Cargo.toml, Cargo.lock, flake.nix
(peatVersion), CHANGELOG.md. `just release <v>` runs `tag`, which runs the
full `verify` first; nothing tags unverified.
"""

from __future__ import annotations

import argparse
import os
import re
import subprocess
import sys
import tomllib
from datetime import date
from collections.abc import Mapping
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


# git variables that redirect git away from the directory it runs in; an
# inherited one would make every git call below act on some other repo
GIT_REDIRECT_VARS = ("GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE", "GIT_COMMON_DIR")


def scrubbed_env(env: Mapping[str, str]) -> dict[str, str]:
    return {k: v for k, v in env.items() if k not in GIT_REDIRECT_VARS}


def run(cmd: list[str]) -> None:
    print(f"+ {' '.join(cmd)}", flush=True)
    subprocess.run(cmd, cwd=ROOT, check=True, env=scrubbed_env(os.environ))


def capture(cmd: list[str]) -> str:
    result = subprocess.run(
        cmd,
        cwd=ROOT,
        check=True,
        stdout=subprocess.PIPE,
        text=True,
        env=scrubbed_env(os.environ),
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


# ------------------------------------------------------------ git guards
# Pure decisions (unit-tested); the git/jj calls that feed them live in tag().
# A jj workspace has no .git, and ~/.git exists, so git run from a workspace
# silently answers about the home repo: git runs only in the colocated checkout.


def colocated_refusal(root: Path, git_dir_is_dir: bool, toplevel: str | None) -> str | None:
    """Why git must not run at `root`, or None when root is the colocated checkout.

    `toplevel` is `git -C root rev-parse --show-toplevel`, asked only once
    `root/.git` is known to be a directory (None when it was not asked).
    """
    where = (
        "run releases from the colocated checkout (the one holding .git and .jj, "
        "e.g. ~/code/peat), not a jj workspace"
    )
    if not git_dir_is_dir:
        return f"{root} has no .git directory, so git would act on an enclosing repo — {where}"
    if toplevel is None or Path(toplevel).resolve() != root.resolve():
        return f"git's toplevel is {toplevel!r}, not {root} — {where}"
    return None


def main_agreement_refusal(jj_main: str, git_main: str) -> str | None:
    if not jj_main:
        return "jj has no `main` bookmark"
    if jj_main != git_main:
        return (
            f"jj has main at {jj_main} but git has {git_main or 'nothing'} — "
            "run `jj git fetch` in the colocated checkout (it also imports), then retry"
        )
    return None


def remote_head(ls_remote_output: str, ref: str) -> str:
    """The sha `git ls-remote` reports for `ref`, or "" when the remote lacks it."""
    for line in ls_remote_output.splitlines():
        parts = line.split()
        if len(parts) == 2 and parts[1] == ref:
            return parts[0]
    return ""


def release_lease_arg(observed_release: str) -> str:
    # an empty expectation tells git the remote ref must not exist yet
    return f"--force-with-lease=release:{observed_release}"


def require_colocated_checkout() -> None:
    git_dir_is_dir = (ROOT / ".git").is_dir()
    toplevel = None
    if git_dir_is_dir:
        try:
            toplevel = capture(["git", "-C", str(ROOT), "rev-parse", "--show-toplevel"])
        except subprocess.CalledProcessError:
            toplevel = None
    refusal = colocated_refusal(ROOT, git_dir_is_dir, toplevel)
    if refusal is not None:
        fail(refusal)


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
    # before any git call: git outside the colocated checkout hits ~/.git
    require_colocated_checkout()
    # the lease expects the release tip seen now, not whatever origin/release
    # the local tracking ref happens to hold when the push runs
    observed_release = remote_head(
        capture(["git", "ls-remote", "origin", "refs/heads/release"]),
        "refs/heads/release",
    )
    verified = verify()
    if verified != version:
        fail(f"repo is at version {verified}, expected {version}")

    tag_name = f"v{version}"
    if capture(["git", "tag", "--list", tag_name]):
        fail(f"tag {tag_name} already exists")

    # jj and git must name the same main, or git would tag a stale ref
    jj_main = capture(
        ["jj", "--ignore-working-copy", "log", "--no-graph", "-r", "main", "-T", "commit_id"]
    )
    try:
        git_main = capture(["git", "rev-parse", "--verify", "refs/heads/main^{commit}"])
    except subprocess.CalledProcessError:
        git_main = ""
    refusal = main_agreement_refusal(jj_main, git_main)
    if refusal is not None:
        fail(refusal)

    # tag what is published: local main must be what origin serves
    remote = capture(["git", "ls-remote", "origin", "refs/heads/main"])
    if remote_head(remote, "refs/heads/main") != jj_main:
        fail("local main differs from origin/main — `just land` first")

    # tag the commit jj reported, by id, so the tag cannot point elsewhere
    run(["git", "tag", "-a", tag_name, "-m", f"peat {tag_name}", jj_main])
    run(["git", "push", "origin", tag_name])
    # `release` is the moving branch release-tracking flake inputs follow
    run(["git", "push", "origin", "main:release", release_lease_arg(observed_release)])
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
