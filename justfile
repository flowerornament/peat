# The quality gate `land` runs: format, lint, test
check:
    cargo fmt --check
    cargo clippy --all-targets -- -D warnings
    cargo test

fmt:
    cargo fmt

# Publish: gate, then move the bookmark and push — one verb, no drift
land bookmark="main":
    #!/usr/bin/env bash
    set -euo pipefail
    b="{{ bookmark }}"
    # The work is @ after `jj describe`, @- after `jj commit` (which opens a fresh @).
    if [ -n "$(jj log --no-graph -r '@ & ~empty()' -T commit_id)" ]; then tgt=@
    elif [ -n "$(jj log --no-graph -r '@- & ~empty()' -T commit_id)" ]; then tgt=@-
    else echo "land: nothing to publish" >&2; exit 2; fi
    if [ -z "$(jj log --no-graph -r "$tgt & ~description(exact:'')" -T commit_id)" ]; then
        echo "land: $tgt has no description — jj describe first" >&2; exit 2
    fi
    # Pin to a commit id: the gated tree and the pushed commit must be the same object.
    tgt="$(jj log --no-graph -r "$tgt" -T commit_id)"
    ancestor_or_refuse() {
        if [ -z "$(jj log --no-graph -r "$b & ::$tgt" -T commit_id)" ]; then
            echo "land: $b is not an ancestor of $tgt — trunk advanced; jj rebase -d $b and retry" >&2
            exit 2
        fi
    }
    ancestor_or_refuse
    just check
    # Trunk can advance inside the gate window; ask again.
    ancestor_or_refuse
    jj bookmark move "$b" --to "$tgt"
    if [ -n "$(jj git remote list)" ]; then jj git push -b "$b"
    else echo "land: no git remote — bookmark moved, push skipped" >&2; fi

# Set the release version in Cargo.toml/Cargo.lock/flake.nix + scaffold CHANGELOG
release-bump version:
    python3 scripts/release.py bump {{ quote(version) }}

# Release readiness checks without tagging (self-tests the machinery first)
release-verify:
    python3 scripts/release.py verify

# Cut a release: verify everything, tag main, push, move `release` — CI publishes
release version:
    python3 scripts/release.py tag {{ quote(version) }}

# Preview the CHANGELOG entry CI will publish as GitHub release notes
release-notes version:
    python3 scripts/release.py notes {{ quote(version) }}
