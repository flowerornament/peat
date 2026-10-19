#!/usr/bin/env python3
"""Unit tests for release.py's pure text functions.

`release.py verify` runs this file first — the machinery proves itself
before it judges the repo. Keep every test subprocess-free.
"""

import tempfile
import unittest
from pathlib import Path

from release import (
    CHANGELOG_INTRO_MARKER,
    GIT_REDIRECT_VARS,
    changelog_entry_is_ready,
    changelog_entry_text,
    changelog_has_entry,
    changelog_insert_scaffold,
    colocated_refusal,
    main_agreement_refusal,
    release_lease_arg,
    remote_head,
    replace_once,
    scrubbed_env,
)

CHANGELOG = (
    "# Changelog\n\n"
    + CHANGELOG_INTRO_MARKER
    + "## 0.1.0 — 2026-08-18\n\n"
    + "First release.\n\n"
    + "- **Capture**: transcripts ingest into one ledger.\n"
)


class ChangelogTests(unittest.TestCase):
    def test_has_entry_matches_the_peat_heading_shape(self):
        self.assertTrue(changelog_has_entry(CHANGELOG, "0.1.0"))
        self.assertFalse(changelog_has_entry(CHANGELOG, "0.1.1"))
        # the dot in the version must not match as regex-any
        self.assertFalse(changelog_has_entry(CHANGELOG, "0x1x0"))

    def test_entry_text_stops_at_the_next_heading(self):
        text = changelog_insert_scaffold(CHANGELOG, "0.1.1", "2026-08-19")
        entry = changelog_entry_text(text, "0.1.1")
        self.assertIn("TODO", entry)
        self.assertNotIn("0.1.0", entry)
        old = changelog_entry_text(text, "0.1.0")
        self.assertIn("**Capture**", old)

    def test_entry_text_fails_on_missing_version(self):
        with self.assertRaises(ValueError):
            changelog_entry_text(CHANGELOG, "9.9.9")

    def test_insert_scaffold_lands_newest_first(self):
        text = changelog_insert_scaffold(CHANGELOG, "0.1.1", "2026-08-19")
        self.assertLess(text.index("## 0.1.1"), text.index("## 0.1.0"))

    def test_insert_scaffold_is_idempotent(self):
        once = changelog_insert_scaffold(CHANGELOG, "0.1.1", "2026-08-19")
        twice = changelog_insert_scaffold(once, "0.1.1", "2026-08-20")
        self.assertEqual(once, twice)

    def test_insert_scaffold_fails_without_any_section(self):
        with self.assertRaises(ValueError):
            changelog_insert_scaffold("# Changelog\n\nno sections\n", "0.1.1", "2026-08-19")

    def test_bump_retitles_an_unreleased_section(self):
        text = (
            "# Changelog\n\n"
            + CHANGELOG_INTRO_MARKER
            + "Entries prefixed **Hooks:** mean re-sync.\n\n"
            + "## Unreleased\n\n- Accumulated note.\n\n"
            + "## 0.1.0 — 2026-08-18\n\n- Old.\n"
        )
        out = changelog_insert_scaffold(text, "0.1.1", "2026-08-19")
        self.assertNotIn("## Unreleased", out)
        self.assertIn("## 0.1.1 — 2026-08-19\n\n- Accumulated note.", out)
        self.assertTrue(changelog_entry_is_ready(out, "0.1.1"))

    def test_two_unreleased_sections_fail(self):
        text = (
            "# Changelog\n\n"
            + CHANGELOG_INTRO_MARKER
            + "## Unreleased\n\n- a\n\n## Unreleased\n\n- b\n"
        )
        with self.assertRaises(ValueError):
            changelog_insert_scaffold(text, "0.1.1", "2026-08-19")

    def test_scaffold_lands_after_multi_paragraph_intro(self):
        text = (
            "# Changelog\n\n"
            + CHANGELOG_INTRO_MARKER
            + "Entries prefixed **Hooks:** mean re-sync.\n\n"
            + "## 0.1.0 — 2026-08-18\n\n- Old.\n"
        )
        out = changelog_insert_scaffold(text, "0.1.1", "2026-08-19")
        self.assertLess(out.index("**Hooks:**"), out.index("## 0.1.1"))
        self.assertLess(out.index("## 0.1.1"), out.index("## 0.1.0"))

    def test_scaffold_is_not_ready_until_edited(self):
        text = changelog_insert_scaffold(CHANGELOG, "0.1.1", "2026-08-19")
        self.assertFalse(changelog_entry_is_ready(text, "0.1.1"))
        edited = text.replace(
            "- TODO: summarize release changes.", "- Fixed a thing."
        )
        self.assertTrue(changelog_entry_is_ready(edited, "0.1.1"))

    def test_entry_without_bullets_is_not_ready(self):
        text = (
            "# Changelog\n\n"
            + CHANGELOG_INTRO_MARKER
            + "## 0.2.0 — 2026-09-01\n\nprose only, no bullets\n"
        )
        self.assertFalse(changelog_entry_is_ready(text, "0.2.0"))
        self.assertFalse(changelog_entry_is_ready(text, "9.9.9"))


class ReplaceOnceTests(unittest.TestCase):
    def test_replaces_a_single_match(self):
        self.assertEqual(
            replace_once('version = "0.1.0"\n', r'^version = "[^"]+"$', 'version = "0.1.1"'),
            'version = "0.1.1"\n',
        )

    def test_fails_on_zero_matches(self):
        with self.assertRaises(ValueError):
            replace_once("nothing here\n", r"^version = .*$", "x")

    def test_fails_on_two_matches(self):
        with self.assertRaises(ValueError):
            replace_once("a\na\n", r"^a$", "b")


class ColocatedCheckoutTests(unittest.TestCase):
    def test_colocated_checkout_is_allowed(self):
        root = Path("/repo/peat")
        self.assertIsNone(colocated_refusal(root, True, "/repo/peat"))

    def test_toplevel_compares_resolved_paths(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            # macOS /var -> /private/var: git reports the resolved path
            self.assertIsNone(colocated_refusal(root, True, str(root.resolve())))

    def test_workspace_without_git_dir_is_refused(self):
        refusal = colocated_refusal(Path("/repo/peat-1a"), False, None)
        self.assertIsNotNone(refusal)
        self.assertIn("colocated checkout", refusal)

    def test_git_answering_about_an_enclosing_repo_is_refused(self):
        # a .git file (worktree) or a toplevel above root: git is not ours
        refusal = colocated_refusal(Path("/home/u/code/peat"), True, "/home/u")
        self.assertIsNotNone(refusal)
        self.assertIn("colocated checkout", refusal)

    def test_unknown_toplevel_is_refused(self):
        self.assertIsNotNone(colocated_refusal(Path("/repo/peat"), True, None))


class EnvScrubTests(unittest.TestCase):
    def test_scrub_drops_every_git_redirect_var(self):
        env = {name: "/elsewhere" for name in GIT_REDIRECT_VARS}
        env.update({"PATH": "/bin", "GIT_AUTHOR_NAME": "a"})
        self.assertEqual(scrubbed_env(env), {"PATH": "/bin", "GIT_AUTHOR_NAME": "a"})

    def test_scrub_covers_the_four_named_vars(self):
        self.assertEqual(
            set(GIT_REDIRECT_VARS),
            {"GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE", "GIT_COMMON_DIR"},
        )


class MainAgreementTests(unittest.TestCase):
    def test_agreeing_mains_pass(self):
        self.assertIsNone(main_agreement_refusal("abc123", "abc123"))

    def test_lagging_git_ref_is_refused_with_the_fix(self):
        refusal = main_agreement_refusal("abc123", "def456")
        self.assertIsNotNone(refusal)
        self.assertIn("jj git fetch", refusal)

    def test_missing_main_is_refused(self):
        self.assertIsNotNone(main_agreement_refusal("", ""))
        self.assertIsNotNone(main_agreement_refusal("abc123", ""))


class RemoteHeadTests(unittest.TestCase):
    def test_reads_the_named_ref(self):
        out = "1111\trefs/heads/release\n2222\trefs/heads/release-old\n"
        self.assertEqual(remote_head(out, "refs/heads/release"), "1111")

    def test_absent_ref_is_empty(self):
        self.assertEqual(remote_head("", "refs/heads/release"), "")

    def test_a_prefix_is_not_a_match(self):
        # the old check was `startswith(local_main)`, which an empty id passes
        self.assertEqual(remote_head("2222\trefs/heads/mainline\n", "refs/heads/main"), "")


class ReleaseLeaseTests(unittest.TestCase):
    def test_lease_names_ref_and_observed_sha(self):
        self.assertEqual(
            release_lease_arg("1111"), "--force-with-lease=release:1111"
        )

    def test_lease_is_never_bare(self):
        # bare --force-with-lease trusts the local tracking ref, which a
        # fetch can move; an absent remote release must be expected absent
        self.assertNotEqual(release_lease_arg(""), "--force-with-lease")
        self.assertEqual(release_lease_arg(""), "--force-with-lease=release:")


if __name__ == "__main__":
    unittest.main()
