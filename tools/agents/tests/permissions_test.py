#!/usr/bin/env fuchsia-vendored-python
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Unit tests for permissions definitions and expansion logic."""

from __future__ import annotations

import re
import unittest

from agents.lib import permissions
from agents_testing.base import BaseTestCase


class PermissionsTest(BaseTestCase):
    """Tests for permissions module functions."""

    def setUp(self) -> None:
        super().setUp()
        self.mock_root = self.test_dir

    def test_expand_empty_or_comment(self) -> None:
        self.assertEqual(permissions.expand_command_variants(""), [])
        self.assertEqual(permissions.expand_command_variants("   "), [])
        self.assertEqual(
            permissions.expand_command_variants("# this is a comment"), []
        )
        # Inline comments should be stripped
        grants = permissions.expand_command_variants(
            "fx test  # run unit tests"
        )
        self.assertEqual(len(grants), 1)
        self.assertTrue(grants[0].startswith("command(regex:"))
        pattern = grants[0][14:-1]
        self.assertTrue(re.fullmatch(pattern, "fx test"))

    def test_expand_comment_with_quoted_hash(self) -> None:
        # Hash inside quotes must not be stripped as a comment
        grants = permissions.expand_command_variants(
            'git commit -m "fix #1234"  # inline comment'
        )
        self.assertEqual(len(grants), 1)
        pattern = grants[0][14:-1]
        self.assertTrue(re.fullmatch(pattern, 'git commit -m "fix #1234"'))

    def test_expand_explicit_syntax(self) -> None:
        self.assertEqual(
            permissions.expand_command_variants("command(foo bar)"),
            ["command(foo bar)"],
        )
        self.assertEqual(
            permissions.expand_command_variants("command(regex:foo.*)"),
            ["command(regex:foo.*)"],
        )

    def test_expand_git_subcommand_regex(self) -> None:
        grants = permissions.expand_command_variants("git status")
        self.assertEqual(len(grants), 1)
        self.assertTrue(grants[0].startswith("command(regex:"))
        pattern = grants[0][14:-1]

        # Test pattern matching
        self.assertTrue(re.fullmatch(pattern, "git status"))
        self.assertTrue(re.fullmatch(pattern, "/usr/bin/git status"))
        self.assertTrue(re.fullmatch(pattern, "git -C /some/dir status"))
        self.assertTrue(
            re.fullmatch(pattern, 'git -C "/dir with spaces" status')
        )
        self.assertTrue(re.fullmatch(pattern, "git --no-pager status"))
        self.assertTrue(
            re.fullmatch(pattern, 'git -c user.name="John Doe" status')
        )
        self.assertTrue(re.fullmatch(pattern, "FOO=bar git status"))
        self.assertTrue(re.fullmatch(pattern, "git status --short"))

        # Boundary checks
        self.assertFalse(re.fullmatch(pattern, "git-lfs status"))

    def _single_pattern(self, template: str) -> str:
        grants = permissions.expand_command_variants(template)
        self.assertEqual(len(grants), 1)
        self.assertTrue(grants[0].startswith("command(regex:"))
        return grants[0][14:-1]

    def test_expand_git_bare(self) -> None:
        pattern = self._single_pattern("git pull")

        # Basic subcommand and path variants
        self.assertTrue(re.fullmatch(pattern, "git pull"))
        self.assertTrue(re.fullmatch(pattern, "/usr/bin/git pull"))
        self.assertTrue(re.fullmatch(pattern, "git pull origin main"))
        self.assertTrue(re.fullmatch(pattern, "/usr/bin/git pull origin main"))
        self.assertFalse(re.fullmatch(pattern, "git-lfs pull"))

    def test_expand_git_config_scoped(self) -> None:
        cases = [
            (
                "git config --global",
                [
                    "git config --global user.email 'a@b.com'",
                    "git -C /dir config --global -l",
                ],
            ),
            (
                "git config --system",
                [
                    "git config --system user.email 'a@b.com'",
                    "git -C /dir config --system -l",
                ],
            ),
            (
                "git config --file",
                [
                    "git config --file ~/.gitconfig user.email 'a@b.com'",
                    "git config --file=/etc/gitconfig user.email 'a@b.com'",
                    "git -C /dir config --file ~/.gitconfig -l",
                ],
            ),
            (
                "git config -f",
                [
                    "git config -f ~/.gitconfig user.email 'a@b.com'",
                ],
            ),
        ]
        for template, positive_cases in cases:
            with self.subTest(template=template):
                pattern = self._single_pattern(template)
                for cmd in positive_cases:
                    self.assertTrue(re.fullmatch(pattern, cmd), cmd)
                self.assertFalse(
                    re.fullmatch(pattern, "git config user.email 'a@b.com'")
                )
                self.assertFalse(
                    re.fullmatch(pattern, "git config --get user.email")
                )

    def test_expand_git_force_push_matching(self) -> None:
        pattern = self._single_pattern("git push --force")
        self.assertTrue(re.fullmatch(pattern, "git push --force"))
        self.assertTrue(re.fullmatch(pattern, "git push origin main --force"))
        self.assertTrue(
            re.fullmatch(pattern, 'git push "my branch with spaces" --force')
        )
        self.assertTrue(
            re.fullmatch(pattern, "git -C /dir push origin HEAD --force")
        )
        self.assertFalse(re.fullmatch(pattern, "git push origin main"))

        # Test --force-with-lease matching including =<ref> syntax
        lease_pattern = self._single_pattern("git push --force-with-lease")
        for cmd in (
            "git push --force-with-lease",
            "git push origin main --force-with-lease",
            "git push origin main --force-with-lease=main",
            "git -C /dir push -u origin HEAD --force-with-lease=refs/heads/main:refs/heads/main",
        ):
            self.assertTrue(re.fullmatch(lease_pattern, cmd), cmd)
        self.assertFalse(re.fullmatch(lease_pattern, "git push origin main"))

    def test_expand_git_clean_force(self) -> None:
        pattern = self._single_pattern("git clean -f")
        # All flag variations and permutations must match
        for cmd in (
            "git clean -f",
            "git clean -fd",
            "git clean -df",
            "git clean -fdx",
            "git clean -fxd",
            "git clean -dfx",
            "git clean -dxf",
            "git clean -xfd",
            "git clean -xdf",
            "git clean -fX",
            "git clean -Xf",
            "git clean --force",
            "git clean --force=true",
            "git clean -d -x -f",
            "git -C /dir clean -fxd",
            "git --git-dir=/dir clean -fxd",
        ):
            self.assertTrue(re.fullmatch(pattern, cmd), cmd)

        # Non-destructive clean commands must NOT match
        for cmd in (
            "git clean",
            "git clean -n",
            "git clean -nd",
            "git clean -n -d -x",
            "git clean -i",
            "git clean --dry-run",
        ):
            self.assertFalse(re.fullmatch(pattern, cmd), cmd)

    def test_expand_git_quoted_arguments(self) -> None:
        grants = permissions.expand_command_variants(
            'git commit -m "fix -f bug"'
        )
        self.assertTrue(len(grants) >= 1)
        patterns = [g[14:-1] for g in grants if g.startswith("command(regex:")]
        self.assertTrue(
            any(re.fullmatch(p, 'git commit -m "fix -f bug"') for p in patterns)
        )

    def test_expand_quoted_binary_path(self) -> None:
        grants = permissions.expand_command_variants(
            '"/custom/path to/git" status'
        )
        self.assertTrue(len(grants) >= 1)
        self.assertTrue(any("git" in g and "status" in g for g in grants))

    def test_expand_fx_variants(self) -> None:
        grants = permissions.expand_command_variants("fx test")
        self.assertEqual(len(grants), 1)
        self.assertTrue(grants[0].startswith("command(regex:"))
        pattern = grants[0][14:-1]

        # Positive matching
        self.assertTrue(re.fullmatch(pattern, "fx test"))
        self.assertTrue(re.fullmatch(pattern, "fx -t target-name test"))
        self.assertTrue(
            re.fullmatch(pattern, "fx -t target-name test //foo:bar")
        )
        self.assertTrue(
            re.fullmatch(pattern, "fx -t [fe80::1%eth0]:8022 test //foo:bar")
        )
        self.assertTrue(re.fullmatch(pattern, 'fx -t="my-device" test'))
        self.assertTrue(re.fullmatch(pattern, "fx --dir out/default test"))
        self.assertTrue(
            re.fullmatch(
                pattern,
                "fx --dir=out/default -t my-dev -i -x test //pkg:test",
            )
        )
        self.assertTrue(re.fullmatch(pattern, "scripts/fx -t my-device test"))
        self.assertTrue(
            re.fullmatch(pattern, "./scripts/fx --enable=incremental test")
        )
        self.assertTrue(re.fullmatch(pattern, "tools/fx test"))
        self.assertTrue(re.fullmatch(pattern, ".jiri_root/bin/fx test"))
        self.assertTrue(re.fullmatch(pattern, "ENV_VAR=1 fx -t my-dev test"))

        # Negative matching
        self.assertFalse(re.fullmatch(pattern, "fx build"))
        self.assertFalse(re.fullmatch(pattern, "fx testing"))
        self.assertFalse(re.fullmatch(pattern, "fxx test"))

    def test_expand_fx_empty_args(self) -> None:
        grants = permissions.expand_command_variants("fx")
        self.assertEqual(len(grants), 1)
        self.assertTrue(grants[0].startswith("command(regex:"))
        pattern = grants[0][14:-1]
        self.assertTrue(re.fullmatch(pattern, "fx"))
        self.assertTrue(re.fullmatch(pattern, "fx -x"))
        self.assertTrue(re.fullmatch(pattern, "scripts/fx -t my-device build"))

    def test_expand_ffx_variants(self) -> None:
        grants = permissions.expand_command_variants("ffx target list")
        self.assertEqual(len(grants), 1)
        self.assertTrue(grants[0].startswith("command(regex:"))
        pattern = grants[0][14:-1]

        # Positive matching
        self.assertTrue(re.fullmatch(pattern, "ffx target list"))
        self.assertTrue(re.fullmatch(pattern, "ffx -t my-device target list"))
        self.assertTrue(re.fullmatch(pattern, "ffx --machine json target list"))
        self.assertTrue(
            re.fullmatch(
                pattern,
                "ffx -c log.level=Debug -t my-device --machine json target list",
            )
        )
        self.assertTrue(re.fullmatch(pattern, "ffx -v --strict target list"))
        self.assertTrue(re.fullmatch(pattern, "fx ffx target list"))
        self.assertTrue(
            re.fullmatch(pattern, "fx -t my-dev ffx --machine json target list")
        )
        self.assertTrue(
            re.fullmatch(
                pattern, "scripts/fx -t my-dev ffx -t my-dev2 target list"
            )
        )
        self.assertTrue(
            re.fullmatch(pattern, "ENV_VAR=1 ffx -t dev target list")
        )

        # Negative matching
        self.assertFalse(re.fullmatch(pattern, "ffx target show"))
        self.assertFalse(re.fullmatch(pattern, "ffx emu start"))

    def test_expand_ffx_empty_args(self) -> None:
        grants = permissions.expand_command_variants("ffx")
        self.assertEqual(len(grants), 1)
        self.assertTrue(grants[0].startswith("command(regex:"))
        pattern = grants[0][14:-1]
        self.assertTrue(re.fullmatch(pattern, "ffx"))
        self.assertTrue(re.fullmatch(pattern, "ffx -v"))
        self.assertTrue(re.fullmatch(pattern, "fx ffx target list"))
        self.assertTrue(
            re.fullmatch(pattern, "scripts/fx -t my-dev ffx target list")
        )

    def test_expand_jiri_variants(self) -> None:
        grants = permissions.expand_command_variants("jiri status")
        self.assertEqual(len(grants), 1)
        self.assertTrue(grants[0].startswith("command(regex:"))
        pattern = grants[0][14:-1]

        # Positive matching
        self.assertTrue(re.fullmatch(pattern, "jiri status"))
        self.assertTrue(re.fullmatch(pattern, "jiri -v status"))
        self.assertTrue(re.fullmatch(pattern, "jiri -vv status"))
        self.assertTrue(re.fullmatch(pattern, "jiri -j 50 status"))
        self.assertTrue(
            re.fullmatch(
                pattern, "jiri -root /path/to/root -color never status"
            )
        )
        self.assertTrue(
            re.fullmatch(
                pattern, ".jiri_root/bin/jiri -show-progress=false status"
            )
        )
        self.assertTrue(re.fullmatch(pattern, "ENV_VAR=1 jiri -q status"))
        self.assertTrue(re.fullmatch(pattern, "jiri -time status"))
        self.assertTrue(re.fullmatch(pattern, "jiri --time status"))
        self.assertTrue(re.fullmatch(pattern, "jiri --quiet status"))
        self.assertTrue(re.fullmatch(pattern, "jiri --show-progress status"))
        self.assertTrue(
            re.fullmatch(pattern, "jiri -time-log-threshold 5s status")
        )
        self.assertTrue(
            re.fullmatch(pattern, "jiri -timefile /tmp/timing.txt status")
        )
        self.assertTrue(
            re.fullmatch(pattern, "jiri -progress-window 10 status")
        )

        # Negative matching
        self.assertFalse(re.fullmatch(pattern, "jiri update"))
        self.assertFalse(re.fullmatch(pattern, "jiri snapshot"))

    def test_expand_jiri_empty_args(self) -> None:
        grants = permissions.expand_command_variants("jiri")
        self.assertEqual(len(grants), 1)
        self.assertTrue(grants[0].startswith("command(regex:"))
        pattern = grants[0][14:-1]
        self.assertTrue(re.fullmatch(pattern, "jiri"))
        self.assertTrue(re.fullmatch(pattern, "jiri -v"))
        self.assertTrue(re.fullmatch(pattern, "jiri -q status"))
        self.assertTrue(
            re.fullmatch(pattern, ".jiri_root/bin/jiri -j 10 update")
        )

    def test_tool_spec_custom_tool(self) -> None:
        spec = permissions.ToolSpec(
            binary_prefix=r"(\S+/)?custom",
            global_flags_pattern=r"(\s+--flag)*",
            allow_flags_anywhere=True,
        )
        grants = spec.expand("build --opt")
        self.assertEqual(len(grants), 1)
        pattern = grants[0][14:-1]
        self.assertTrue(re.fullmatch(pattern, "custom --flag build --opt"))
        self.assertTrue(
            re.fullmatch(pattern, "/path/custom build file.txt --opt")
        )

    def test_expand_python_interpreter_variants(self) -> None:
        py_grants = permissions.expand_command_variants("python3 script.py")
        self.assertIn("command(python script.py)", py_grants)
        self.assertIn("command(python3 script.py)", py_grants)
        self.assertIn("command(fuchsia-vendored-python script.py)", py_grants)
        self.assertIn(
            "command(scripts/fuchsia-vendored-python script.py)", py_grants
        )

    def test_expand_sed_inplace_regex(self) -> None:
        pattern = self._single_pattern("sed -i 's/foo/bar/g'")

        self.assertTrue(re.fullmatch(pattern, "sed -i 's/foo/bar/g' file.txt"))
        self.assertTrue(re.fullmatch(pattern, "/usr/bin/sed -i.bak 's/a/b/' f"))
        self.assertTrue(re.fullmatch(pattern, "sed -E -i '' 's/a/b/' f"))
        self.assertTrue(
            re.fullmatch(pattern, "sed --in-place=suffix 's/a/b/' file.txt")
        )

        # Read-only stream commands containing '-i' in expressions must not trigger in-place regex
        readonly_pattern = self._single_pattern(
            "sed 's/foo -i bar/baz/' file.txt"
        )
        self.assertTrue(
            re.fullmatch(readonly_pattern, "sed 's/foo -i bar/baz/' file.txt")
        )
        self.assertTrue(
            re.fullmatch(
                readonly_pattern, "/usr/bin/sed 's/foo -i bar/baz/' file.txt"
            )
        )
        self.assertFalse(re.fullmatch(readonly_pattern, "sed -i file.txt"))

    def test_expand_system_binary_regex(self) -> None:
        # Bare command without arguments
        cat_pattern = self._single_pattern("cat")
        for cmd in (
            "cat",
            "cat file.txt",
            "/bin/cat file.txt",
            "/usr/bin/cat file.txt",
            "LC_ALL=C cat file.txt",
        ):
            self.assertTrue(re.fullmatch(cat_pattern, cmd), cmd)
        for cmd in ("catalog file.txt", "cat_file"):
            self.assertFalse(re.fullmatch(cat_pattern, cmd), cmd)

        # Commands with arguments and trailing path wildcards
        cases = [
            (
                "ls -la",
                [
                    "ls -la",
                    "ls -la /some/dir",
                    "/bin/ls -la",
                    "/usr/bin/ls -la",
                    "FOO=bar ls -la",
                ],
                [
                    "ls -l",
                    "ls",
                ],
            ),
            (
                "rm -rf /",
                [
                    "rm -rf /",
                    "rm -rf //",
                    "/bin/rm -rf /",
                    "rm -rf / home",
                ],
                [
                    "rm -rf /var",
                    "rm -rf /tmp/foo",
                    "rm -rf",
                ],
            ),
            (
                "rm -rf ~",
                [
                    "rm -rf ~",
                    "rm -rf ~/",
                    "rm -rf ~//",
                    "rm -rf ~ home",
                ],
                [
                    "rm -rf ~/dir",
                    "rm -rf ~other",
                    "rm -rf ~/fuchsia/out",
                ],
            ),
        ]
        for template, positive_cases, negative_cases in cases:
            with self.subTest(template=template):
                pattern = self._single_pattern(template)
                for cmd in positive_cases:
                    self.assertTrue(re.fullmatch(pattern, cmd), cmd)
                for cmd in negative_cases:
                    self.assertFalse(re.fullmatch(pattern, cmd), cmd)

    def test_expand_trusted_help_variants(self) -> None:
        # ToolSpec tool (ffx) with global flags and subcommands
        pattern = self._single_pattern("ffx help")
        for cmd in (
            "ffx --help",
            "ffx help target",
            "ffx target --help",
            "ffx component list --help",
            "ffx -t dev target --help",
        ):
            self.assertTrue(re.fullmatch(pattern, cmd), cmd)

        for cmd in (
            "ffx target",
            "ffx target reboot",
            "ffx target reboot now",
        ):
            self.assertFalse(re.fullmatch(pattern, cmd), cmd)

        # Non-ToolSpec tool (cipd)
        cipd_pattern = self._single_pattern("cipd help")
        self.assertTrue(re.fullmatch(cipd_pattern, "cipd --help"))
        self.assertTrue(re.fullmatch(cipd_pattern, "cipd help ensure"))
        self.assertTrue(re.fullmatch(cipd_pattern, "cipd ensure --help"))
        self.assertFalse(re.fullmatch(cipd_pattern, "cipd ensure"))

        # Untrusted tools do not get help expansion
        echo_pattern = self._single_pattern("echo help")
        self.assertTrue(re.fullmatch(echo_pattern, "echo help"))
        self.assertFalse(re.fullmatch(echo_pattern, "echo --help"))

    def test_read_command_list_file(self) -> None:
        list_file = self.mock_root / "commands.txt"
        list_file.write_text(
            "# A comment\n"
            "git status\n"
            "\n"
            "command(explicit_rule)\n"
            "fx build\n",
            encoding="utf-8",
        )
        grants = permissions.read_command_list_file(list_file)
        self.assertTrue(
            any(
                g.startswith("command(regex:") and "git" in g and "status" in g
                for g in grants
            )
        )
        self.assertIn("command(explicit_rule)", grants)
        self.assertTrue(
            any(
                g.startswith("command(regex:") and "fx" in g and "build" in g
                for g in grants
            )
        )

    def test_read_command_list_file_nonexistent(self) -> None:
        self.assertEqual(
            permissions.read_command_list_file(
                self.mock_root / "nonexistent.txt"
            ),
            [],
        )

    def test_load_profile_grants(self) -> None:
        fuchsia_dir = self.mock_root
        perm_dir = fuchsia_dir / ".agents" / "config" / "permissions"
        perm_dir.mkdir(parents=True, exist_ok=True)
        (perm_dir / "read_only.txt").write_text(
            "git status\nfx status\n", encoding="utf-8"
        )
        (perm_dir / "local_changes.txt").write_text(
            "git commit\n", encoding="utf-8"
        )
        (perm_dir / "external_changes.txt").write_text(
            "git push\n", encoding="utf-8"
        )
        (perm_dir / "never_allow.txt").write_text(
            "git reset --hard\n", encoding="utf-8"
        )
        (perm_dir / "device_ops.txt").write_text("fx ota\n", encoding="utf-8")
        (perm_dir / "cache_destruction.txt").write_text(
            "fx clean\n", encoding="utf-8"
        )
        (perm_dir / "batch_execution.txt").write_text(
            "find\n", encoding="utf-8"
        )

        grants = permissions.load_profile_grants(fuchsia_dir, "read-only")
        self.assertTrue(any("git" in g and "status" in g for g in grants.allow))
        self.assertTrue(any("git" in g and "reset" in g for g in grants.deny))
        self.assertFalse(any("git" in g and "commit" in g for g in grants.deny))
        self.assertFalse(
            any("git" in g and "commit" in g for g in grants.allow)
        )
        self.assertTrue(any("git" in g and "push" in g for g in grants.ask))
        self.assertTrue(any("fx" in g and "ota" in g for g in grants.ask))
        self.assertTrue(any("find" in g for g in grants.ask))

        grants_local = permissions.load_profile_grants(
            fuchsia_dir, "local-changes"
        )
        self.assertTrue(
            any("git" in g and "commit" in g for g in grants_local.allow)
        )
        self.assertTrue(
            any("git" in g and "push" in g for g in grants_local.ask)
        )
        self.assertTrue(
            any("git" in g and "reset" in g for g in grants_local.deny)
        )
        self.assertTrue(any("find" in g for g in grants_local.ask))

        grants_ext = permissions.load_profile_grants(
            fuchsia_dir, "external-changes"
        )
        self.assertTrue(
            any("git" in g and "push" in g for g in grants_ext.allow)
        )
        self.assertTrue(any("find" in g for g in grants_ext.ask))


if __name__ == "__main__":
    unittest.main()
