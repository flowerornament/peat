# The quality gate `land` runs: format, lint, test
check:
    cargo fmt --check
    cargo clippy --all-targets -- -D warnings
    cargo test

fmt:
    cargo fmt

# Publish the described jj change: run `just check` on exactly it, then push it
land *args:
    scripts/jj-land.sh {{ args }}

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
