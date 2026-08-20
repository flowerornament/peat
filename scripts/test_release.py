#!/usr/bin/env python3
"""Unit tests for release.py's pure text functions.

`release.py verify` runs this file first — the machinery proves itself
before it judges the repo. Keep every test subprocess-free.
"""

import unittest

from release import (
    CHANGELOG_INTRO_MARKER,
    changelog_entry_is_ready,
    changelog_entry_text,
    changelog_has_entry,
    changelog_insert_scaffold,
    replace_once,
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

    def test_insert_scaffold_fails_without_the_intro_marker(self):
        with self.assertRaises(ValueError):
            changelog_insert_scaffold("# Changelog\n\nno marker\n", "0.1.1", "2026-08-19")

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


if __name__ == "__main__":
    unittest.main()
