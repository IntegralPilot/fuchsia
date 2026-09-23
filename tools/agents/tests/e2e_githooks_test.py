#!/usr/bin/env fuchsia-vendored-python
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""End-to-end Git hooks integration test suite validating real platform tools."""

from __future__ import annotations

import os
import pathlib
import shutil
import subprocess
import sys
import tempfile
import unittest

from agents.lib import githooks
from agents.lib.paths import find_fuchsia_dir


def _find_bundled_tool(rel_path: str) -> pathlib.Path | None:
    """Locates a runtime dependency tool from the Fuchsia checkout."""
    try:
        fuchsia_dir = find_fuchsia_dir()
        cand = fuchsia_dir / rel_path
        return cand if cand.is_file() else None
    except RuntimeError:
        return None


MOCK_FX_WRAPPER_TEMPLATE = r"""#!/usr/bin/env fuchsia-vendored-python
import os
import sys
import subprocess
from pathlib import Path

args = sys.argv[1:]
if not args:
    sys.exit(0)

if args[0] == "format-code":
    files = [f for a in args if a.startswith("--files=") for f in a[8:].split(",") if f]
    json_fmt = os.environ.get("REAL_JSON_FMT")

    json_files = []
    for f in files:
        p = Path(f)
        if not p.is_file():
            continue
        ext = p.suffix.lower()
        if ext in (".json", ".config"):
            json_files.append(str(p))

    failed = False
    for jf in json_files:
        if json_fmt and Path(json_fmt).is_file():
            res = subprocess.run(
                [sys.executable, json_fmt, "--no-sort-keys", "--ignore-errors", "--quiet", jf],
                capture_output=True,
                text=True,
            )
            if res.returncode != 0:
                sys.stderr.write(res.stderr or res.stdout)
                failed = True

    if failed:
        sys.exit(1)
    sys.exit(0)
"""

MOCK_JIRI_DISPATCHER_TEMPLATE = (
    "#!/bin/sh\n"
    'HOOK_NAME="{hook_name}"\n'
    'GIT_HOOKS_DIR="$(git rev-parse --git-path hooks 2>/dev/null || echo "$PWD/.git/hooks")"\n'
    'HOOK_D="$GIT_HOOKS_DIR/${{HOOK_NAME}}.d"\n'
    'if [ -d "$HOOK_D" ]; then\n'
    '  for script in "$HOOK_D"/*; do\n'
    '    [ -f "$script" ] || continue\n'
    '    case "$script" in\n'
    "      *~ | *.bak | *.bak.* | *.tmp | *.swp | *.sample | *.disabled) continue ;;\n"
    "    esac\n"
    '    if [ -x "$script" ]; then\n'
    '      "$script" "$@" || exit $?\n'
    "    fi\n"
    "  done\n"
    "fi\n"
    "exit 0\n"
)


def _init_git_repo(
    repo_dir: pathlib.Path,
    user_name: str,
    user_email: str,
    install_jiri_dispatchers: bool = True,
) -> None:
    """Initializes a git repository with standard test config and mock dispatchers."""
    git_env = {
        **os.environ,
        "GIT_CONFIG_GLOBAL": "/dev/null",
        "GIT_CONFIG_NOSYSTEM": "1",
    }
    for cmd in (
        ["git", "init", "--template=", "-b", "main"],
        ["git", "config", "user.name", user_name],
        ["git", "config", "user.email", user_email],
        ["git", "config", "commit.gpgsign", "false"],
    ):
        subprocess.run(
            cmd, cwd=repo_dir, check=True, capture_output=True, env=git_env
        )

    if install_jiri_dispatchers:
        hooks_dir = repo_dir / ".git" / "hooks"
        hooks_dir.mkdir(parents=True, exist_ok=True)
        for h in ("pre-commit", "commit-msg"):
            hook_file = hooks_dir / h
            hook_file.write_text(
                MOCK_JIRI_DISPATCHER_TEMPLATE.format(hook_name=h),
                encoding="utf-8",
            )
            hook_file.chmod(0o755)


class RealHooksWorkspaceFixture:
    """Fixture creating an isolated Git workspace configured with real platform tools."""

    def __init__(self) -> None:
        self.commit_msg_checker = _find_bundled_tool(
            "scripts/shac/commit_msg_checker.py"
        )
        self.json_fmt = _find_bundled_tool("scripts/style/json-fmt.py")

        self.template_dir_clean = pathlib.Path(
            tempfile.mkdtemp(prefix="real_hooks_clean_template_")
        )
        self.template_dir_with_hooks = pathlib.Path(
            tempfile.mkdtemp(prefix="real_hooks_installed_template_")
        )
        try:
            self._setup_clean_template()
            self._setup_with_hooks_template()
        except Exception:
            self.cleanup()
            raise

    def cleanup(self) -> None:
        if self.template_dir_clean.exists():
            shutil.rmtree(self.template_dir_clean, ignore_errors=True)
        if self.template_dir_with_hooks.exists():
            shutil.rmtree(self.template_dir_with_hooks, ignore_errors=True)

    def _setup_clean_template(self) -> None:
        root = self.template_dir_clean
        (root / ".jiri_root").mkdir(parents=True, exist_ok=True)
        (root / ".fx-root").touch(exist_ok=True)

        git_env = {
            **os.environ,
            "GIT_CONFIG_GLOBAL": "/dev/null",
            "GIT_CONFIG_NOSYSTEM": "1",
        }

        _init_git_repo(root, "Test Author", "test@example.com")

        (root / ".gitignore").write_text(
            "/.fx-root\n/bin/\n/scripts/\n/tools/\n/vendor/\n/.worktrees/\n/worktree-*/\n",
            encoding="utf-8",
        )

        src_dir = root / "src"
        src_dir.mkdir(parents=True, exist_ok=True)
        (src_dir / "foo.json").write_text(
            '{\n    "name": "foo"\n}\n', encoding="utf-8"
        )

        subprocess.run(
            ["git", "add", ".gitignore", "src/"],
            cwd=root,
            check=True,
            env=git_env,
        )
        subprocess.run(
            [
                "git",
                "commit",
                "-m",
                "[root] Initial baseline\n\nBug: 123\nTest: none\nChange-Id: I0000000000000000000000000000000000000001\n",
            ],
            cwd=root,
            check=True,
            env=git_env,
        )

        vendor_dir = root / "vendor" / "google"
        vendor_dir.mkdir(parents=True, exist_ok=True)
        _init_git_repo(vendor_dir, "Vendor Author", "vendor@example.com")

        (vendor_dir / "lib.json").write_text(
            '{\n    "name": "lib"\n}\n', encoding="utf-8"
        )
        subprocess.run(
            ["git", "add", "lib.json"], cwd=vendor_dir, check=True, env=git_env
        )
        subprocess.run(
            [
                "git",
                "commit",
                "-m",
                "[vendor] Baseline\n\nBug: 123\nTest: none\nChange-Id: I0000000000000000000000000000000000000002\n",
            ],
            cwd=vendor_dir,
            check=True,
            env=git_env,
        )

        scripts_dir = root / "scripts"
        scripts_dir.mkdir(parents=True, exist_ok=True)
        bin_dir = root / "bin"
        bin_dir.mkdir(parents=True, exist_ok=True)

        fx_script = bin_dir / "fx"
        fx_script.write_text(MOCK_FX_WRAPPER_TEMPLATE, encoding="utf-8")
        fx_script.chmod(0o755)

        python_bin = scripts_dir / "fuchsia-vendored-python"
        python_bin.symlink_to(sys.executable)

        shac_dir = scripts_dir / "shac"
        shac_dir.mkdir(parents=True, exist_ok=True)
        if self.commit_msg_checker and self.commit_msg_checker.is_file():
            (shac_dir / "commit_msg_checker.py").symlink_to(
                self.commit_msg_checker
            )

        tools_dir = root / "tools"
        tools_dir.mkdir(parents=True, exist_ok=True)
        fuchsia_root = find_fuchsia_dir()
        shutil.copytree(
            fuchsia_root / "tools" / "agents",
            tools_dir / "agents",
            ignore=shutil.ignore_patterns("__pycache__", "*.pyc"),
        )

    def _setup_with_hooks_template(self) -> None:
        shutil.copytree(
            self.template_dir_clean,
            self.template_dir_with_hooks,
            dirs_exist_ok=True,
            symlinks=True,
        )
        res = githooks.install_git_hooks(self.template_dir_with_hooks)
        if res.failed:
            raise RuntimeError(
                f"Failed to pre-install dev hooks in template: {res.failed}"
            )

    def create_isolated_workspace(
        self, prefix: str = "real_hooks_test_", with_hooks: bool = True
    ) -> pathlib.Path:
        temp_dir = pathlib.Path(tempfile.mkdtemp(prefix=prefix))
        src_template = (
            self.template_dir_with_hooks
            if with_hooks
            else self.template_dir_clean
        )
        shutil.copytree(
            src_template, temp_dir, dirs_exist_ok=True, symlinks=True
        )
        return temp_dir


@unittest.skipUnless(
    shutil.which("git") is not None,
    "git binary not available in test environment",
)
class E2EGitHooksTest(unittest.TestCase):
    """End-to-end test cases verifying real format-code and commit_msg_checker."""

    fixture: RealHooksWorkspaceFixture

    @classmethod
    def setUpClass(cls) -> None:
        if not shutil.which("git"):
            raise unittest.SkipTest(
                "git binary not available in test environment"
            )
        super().setUpClass()
        cls.fixture = RealHooksWorkspaceFixture()
        cls.addClassCleanup(cls.fixture.cleanup)
        if not cls.fixture.commit_msg_checker:
            raise unittest.SkipTest(
                "commit_msg_checker.py could not be located in test dependencies or checkout"
            )
        if not cls.fixture.json_fmt:
            raise unittest.SkipTest(
                "json-fmt.py could not be located in test dependencies or checkout"
            )

    def _run_git(
        self,
        *args: str,
        cwd: pathlib.Path,
        check: bool = False,
        extra_env: dict[str, str] | None = None,
        remove_env: tuple[str, ...] = (),
    ) -> subprocess.CompletedProcess[str]:
        env = dict(os.environ)
        try:
            top = find_fuchsia_dir(cwd)
        except RuntimeError:
            try:
                res = subprocess.run(
                    ["git", "rev-parse", "--git-common-dir"],
                    cwd=cwd,
                    capture_output=True,
                    text=True,
                    check=True,
                )
                common = pathlib.Path(res.stdout.strip())
                if not common.is_absolute():
                    common = (cwd / common).resolve()
                top = find_fuchsia_dir(common.parent)
            except Exception:
                top = cwd

        bin_dir = str(top / "bin")
        scripts_dir = str(top / "scripts")
        env["PATH"] = f"{bin_dir}:{scripts_dir}:{env.get('PATH', '')}"

        if self.fixture.json_fmt:
            env["REAL_JSON_FMT"] = str(self.fixture.json_fmt)

        sanitize_vars = (
            "FUCHSIA_SKIP_HOOKS",
            "ANTIGRAVITY_AGENT",
            "GEMINI_CLI",
            "ANTIGRAVITY_EDITOR_APP_ROOT",
            "GIT_CONFIG_PARAMETERS",
            "GIT_DIR",
            "GIT_WORK_TREE",
            "GIT_INDEX_FILE",
            "GIT_OBJECT_DIRECTORY",
            "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        )
        for var in sanitize_vars:
            env.pop(var, None)

        env["GIT_CONFIG_GLOBAL"] = "/dev/null"
        env["GIT_CONFIG_NOSYSTEM"] = "1"
        env["FUCHSIA_DIR"] = str(top)

        for key in remove_env:
            env.pop(key, None)
        if extra_env:
            env.update(extra_env)

        return subprocess.run(
            ["git", *args],
            cwd=cwd,
            env=env,
            capture_output=True,
            text=True,
            check=check,
            timeout=30,
        )

    def test_e2e_pre_commit_formats_and_restages(self) -> None:
        """Verifies that unformatted code is auto-formatted and re-staged on git commit."""
        workspace = self.fixture.create_isolated_workspace()
        self.addCleanup(shutil.rmtree, workspace, ignore_errors=True)

        foo_json = workspace / "src" / "foo.json"
        unformatted_content = '{\n  "unformatted":   true,\n "key": "value" \n}'
        foo_json.write_text(unformatted_content, encoding="utf-8")
        self._run_git("add", "src/foo.json", cwd=workspace, check=True)

        msg = (
            "[test] Multi-language format\n\n"
            "Bug: 1234\n"
            "Test: none\n"
            "Change-Id: I0000000000000000000000000000000000000020\n"
        )
        res = self._run_git("commit", "-m", msg, cwd=workspace)
        self.assertEqual(
            res.returncode,
            0,
            f"Commit should succeed with auto-formatting: {res.stderr}\n{res.stdout}",
        )

        expected_formatted = (
            '{\n    "unformatted": true,\n    "key": "value"\n}\n'
        )
        show_json = self._run_git(
            "show", "HEAD:src/foo.json", cwd=workspace, check=True
        )
        self.assertEqual(show_json.stdout, expected_formatted)
        self.assertEqual(
            foo_json.read_text(encoding="utf-8"), expected_formatted
        )

        status_res = self._run_git(
            "status", "--porcelain", cwd=workspace, check=True
        )
        self.assertEqual(status_res.stdout.strip(), "")

    def test_e2e_pre_commit_blocks_partial_staging_conflict(self) -> None:
        """Verifies that a partially staged file with formatting violations blocks the commit."""
        workspace = self.fixture.create_isolated_workspace()
        self.addCleanup(shutil.rmtree, workspace, ignore_errors=True)

        foo_json = workspace / "src" / "foo.json"
        staged_content = '{\n  "unformatted":   true,\n "key": "foo" \n}'
        foo_json.write_text(staged_content, encoding="utf-8")
        self._run_git("add", "src/foo.json", cwd=workspace, check=True)

        unstaged_content = (
            '{\n  "unformatted":   true,\n "key": "foo",\n "extra": 1 \n}'
        )
        foo_json.write_text(unstaged_content, encoding="utf-8")

        msg = (
            "[test] Partial staging conflict\n\n"
            "Bug: 1234\n"
            "Test: none\n"
            "Change-Id: I0000000000000000000000000000000000000031\n"
        )
        res = self._run_git("commit", "-m", msg, cwd=workspace)
        self.assertEqual(
            res.returncode,
            1,
            "Commit must fail when partially staged file has formatting violations",
        )

        self.assertEqual(foo_json.read_text(encoding="utf-8"), unstaged_content)
        show_index = self._run_git(
            "show", ":src/foo.json", cwd=workspace, check=True
        )
        self.assertEqual(show_index.stdout, staged_content)
        self.assertIn(
            "Formatting or check failure on partially staged files",
            res.stderr + res.stdout,
        )

    def test_e2e_commit_msg_enforcement(self) -> None:
        """Verifies commit_msg_checker runs: warnings pass for humans, fail closed for agents."""
        workspace = self.fixture.create_isolated_workspace()
        self.addCleanup(shutil.rmtree, workspace, ignore_errors=True)

        warning_msg = (
            "[tools][agents] This commit message subject is excessively long and exceeds the sixty-five character limit\n\n"
            "This line is intentionally crafted to exceed the maximum seventy-two character limit for body wrapping.\n\n"
            "Bug: 1234\n"
            "Test: none\n"
            "Change-Id: I0000000000000000000000000000000000000040\n"
            "TAG=agy\n"
        )

        # Human developer (is_agent=False): commit passes with advisory warnings
        res_human = self._run_git(
            "commit", "--allow-empty", "-m", warning_msg, cwd=workspace
        )
        self.assertEqual(
            res_human.returncode,
            0,
            f"Human commit should pass with advisory warnings: {res_human.stderr}",
        )
        output_human = res_human.stdout + res_human.stderr
        self.assertIn("Subject line exceeds 65 characters", output_human)
        self.assertIn("exceeds 72 characters", output_human)

        # Agent developer (ANTIGRAVITY_AGENT=1): commit fails closed on style warnings
        res_agent = self._run_git(
            "commit",
            "--allow-empty",
            "-m",
            warning_msg,
            cwd=workspace,
            extra_env={"ANTIGRAVITY_AGENT": "1"},
        )
        self.assertEqual(
            res_agent.returncode,
            1,
            "Agent commit should fail strictly on style violations",
        )
        output_agent = res_agent.stdout + res_agent.stderr
        self.assertIn("Subject line exceeds 65 characters", output_agent)

    def test_e2e_worktree_execution(self) -> None:
        """Verifies that hooks function properly in a linked Git worktree."""
        workspace = self.fixture.create_isolated_workspace()
        self.addCleanup(shutil.rmtree, workspace, ignore_errors=True)

        wt_temp = pathlib.Path(tempfile.mkdtemp(prefix="e2e_wt_"))
        self.addCleanup(shutil.rmtree, wt_temp, ignore_errors=True)
        wt_dir = wt_temp / "wt1"
        self._run_git(
            "worktree",
            "add",
            str(wt_dir),
            "-b",
            "wt-branch",
            cwd=workspace,
            check=True,
        )

        wt_json = wt_dir / "src" / "foo.json"
        unformatted_content = '{\n  "unformatted":   true,\n "key": "value" \n}'
        wt_json.write_text(unformatted_content, encoding="utf-8")
        self._run_git("add", "src/foo.json", cwd=wt_dir, check=True)

        msg = (
            "[wt] Worktree commit\n\n"
            "Bug: 1234\n"
            "Test: none\n"
            "Change-Id: I0000000000000000000000000000000000000060\n"
        )
        res = self._run_git(
            "commit",
            "-m",
            msg,
            cwd=wt_dir,
            remove_env=("FUCHSIA_DIR",),
        )
        self.assertEqual(
            res.returncode,
            0,
            f"Worktree commit failed: {res.stderr}\n{res.stdout}",
        )

        expected_formatted = (
            '{\n    "unformatted": true,\n    "key": "value"\n}\n'
        )
        show_res = self._run_git(
            "show", "HEAD:src/foo.json", cwd=wt_dir, check=True
        )
        self.assertEqual(show_res.stdout, expected_formatted)
        self.assertEqual(
            wt_json.read_text(encoding="utf-8"), expected_formatted
        )
        status_res = self._run_git(
            "status", "--porcelain", cwd=wt_dir, check=True
        )
        self.assertEqual(status_res.stdout.strip(), "")

    def test_e2e_multi_repo_setup_and_reset(self) -> None:
        """Verifies that install_git_hooks configures root and vendor repos, and uninstall removes them."""
        workspace = self.fixture.create_isolated_workspace(with_hooks=False)
        self.addCleanup(shutil.rmtree, workspace, ignore_errors=True)

        repos = [workspace, workspace / "vendor" / "google"]

        status_init = githooks.get_git_hooks_status(workspace)
        self.assertEqual(status_init.total_repos, 2)
        self.assertEqual(status_init.configured_repos, 0)

        res_install = githooks.install_git_hooks(workspace)
        self.assertEqual(res_install.failed, [])
        self.assertTrue(len(res_install.modified) > 0)

        for repo in repos:
            hooks_dir = repo / ".git" / "hooks"
            self.assertTrue(
                (hooks_dir / "pre-commit.d" / "10-fuchsia-agent.sh").is_file()
            )
            self.assertTrue(
                (hooks_dir / "commit-msg.d" / "10-fuchsia-agent.sh").is_file()
            )

        status_installed = githooks.get_git_hooks_status(workspace)
        self.assertEqual(status_installed.total_repos, 2)
        self.assertEqual(status_installed.configured_repos, 2)

        res_uninstall = githooks.uninstall_git_hooks(workspace)
        self.assertEqual(res_uninstall.failed, [])
        self.assertTrue(len(res_uninstall.modified) > 0)

        for repo in repos:
            hooks_dir = repo / ".git" / "hooks"
            self.assertFalse(
                (hooks_dir / "pre-commit.d" / "10-fuchsia-agent.sh").exists()
            )
            self.assertFalse(
                (hooks_dir / "commit-msg.d" / "10-fuchsia-agent.sh").exists()
            )

        status_uninstalled = githooks.get_git_hooks_status(workspace)
        self.assertEqual(status_uninstalled.total_repos, 2)
        self.assertEqual(status_uninstalled.configured_repos, 0)


if __name__ == "__main__":
    unittest.main()
