# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Tests for sync_workspace."""

import hashlib
import os
import shlex
import stat
import tempfile
import unittest
from pathlib import Path
from typing import Any
from unittest.mock import MagicMock, patch

import logger
import mock_fs
import preflight
import sync_workspace
import workspace


class TestWorkspaceSyncService(unittest.TestCase):
    """Tests for WorkspaceSyncService."""

    def setUp(self) -> None:
        self.temp_dir = Path(tempfile.mkdtemp())
        self.fake_git_citc = self.temp_dir / "git-citc"
        self.signal_file = self.temp_dir / "concurrency_signal"

        # Create fake git-citc
        with open(self.fake_git_citc, "w") as f:
            f.write(self._get_fake_git_citc_script())
        self.fake_git_citc.chmod(
            self.fake_git_citc.stat().st_mode | stat.S_IEXEC
        )

        # Add to PATH
        self.original_path = os.environ.get("PATH", "")
        os.environ["PATH"] = f"{self.temp_dir}:{self.original_path}"

        # Set signal file env var
        os.environ["CONCURRENCY_SIGNAL_FILE"] = str(self.signal_file)

    def tearDown(self) -> None:
        os.environ["PATH"] = self.original_path
        os.environ.pop("CONCURRENCY_SIGNAL_FILE", None)
        import shutil

        shutil.rmtree(self.temp_dir)

    def _get_fake_git_citc_script(self) -> str:
        return """#!/usr/bin/env python3
import os
import sys
import time

args = sys.argv[1:]
cmd = args[0] if args else ""

signal_file = os.environ.get("CONCURRENCY_SIGNAL_FILE")

def log_concurrency(event):
    if signal_file:
        with open(signal_file, "a") as f:
            f.write(f"{os.getpid()} {event} {time.time()}\\n")

if cmd == "api.get-modified-repos":
    fake_repos = os.environ.get("FAKE_MODIFIED_REPOS")
    if fake_repos is not None:
        if fake_repos:
            for repo in fake_repos.split(","):
                print(repo)
        else:
            print("No modified repo paths")
    else:
        output_type = os.environ.get("GIT_CITC_OUTPUT_TYPE", "typical")
        if output_type == "typical":
            print("fuchsia")
            print("fuchsia/third_party/some_dep/src")
        elif output_type == "superproject":
            print("superproject")
            print("superproject/fuchsia")
            print("superproject/fuchsia/third_party/some_dep/src")
            print("superproject/integration")
            print("superproject/vendor/company")

elif cmd == "api.get-repo-states":
    repo = args[1]
    print("GetRepoStates() = response_base { returned_snapshot_version: 19 }")
    print("repo_states {")
    print(f'  repo_root: "{repo}"')
    print('  current_branch: "refs/heads/main"')
    print('  entities { path_in_repo: "placeholder.txt" state: INDEX_MODIFIED }')
    print('  gob_repo_name: "fuchsia/fuchsia"')
    print('  base_commit_hash: "6224216ffe756b3682bc3910fc624f22bab449e0"')
    print("}")

elif cmd == "cli.diff":
    log_concurrency("start")
    time.sleep(0.2)
    log_concurrency("end")

    fake_diff_cog = os.environ.get("FAKE_CLI_DIFF_COG")
    fake_diff_cartfs = os.environ.get("FAKE_CLI_DIFF_CARTFS")
    fake_diff = os.environ.get("FAKE_CLI_DIFF")

    cwd = os.getcwd()
    if fake_diff_cog and "cartfs" not in cwd:
        print(fake_diff_cog)
    elif fake_diff_cartfs and "cartfs" in cwd:
        print(fake_diff_cartfs)
    elif fake_diff:
        print(fake_diff)
    else:
        print("Repo root: fuchsia")
        print("Diffing 6224216ffe756b3682bc3910fc624f22bab449e0..@")
        print("[M] placeholder.txt")

else:
    print(f"Unknown command: {cmd}", file=sys.stderr)
    sys.exit(1)
"""

    def test_affected_files_typical_fuchsia(self) -> None:
        """Test affected_files in typical Fuchsia case."""
        with mock_fs.FileSystemTestHelper() as fs:
            # Setup real config file content in mock fs
            config_path = fs.repo_dir / "scripts" / "cog" / "repo_config.json"
            config_path.parent.mkdir(exist_ok=True, parents=True)
            with open(config_path, "w") as f:
                f.write(
                    """{
  "repo": {
    "ignored": [],
    "fuchsia": "fuchsia",
    "integration": null,
    "stripSrcPrefix": "fuchsia/",
    "destSubdir": "fuchsia"
  }
}"""
                )

            # Create directories for mock repos
            (fs.repo_dir / "third_party" / "some_dep" / "src").mkdir(
                parents=True, exist_ok=True
            )

            mock_ws = MagicMock()
            mock_ws.workspace_root = fs.repo_dir.parent
            mock_ws.cartfs_dir = fs.cartfs_dir
            mock_ws.config = {
                "repo": {
                    "ignored": [],
                    "fuchsia": "fuchsia",
                    "stripSrcPrefix": "fuchsia/",
                    "destSubdir": "fuchsia",
                }
            }

            def fake_run(
                cmd: list[str],
                cwd: str | Path | None = None,
                capture_output: bool = False,
                exit_on_error: bool = True,
            ) -> str:
                import subprocess

                res = subprocess.run(
                    cmd, cwd=cwd, capture_output=capture_output, text=True
                )
                return res.stdout

            mock_ws._run.side_effect = fake_run

            with (
                patch.object(os, "getcwd", return_value=str(fs.repo_dir)),
                patch(
                    "sync_workspace.workspace.Workspace", return_value=mock_ws
                ),
            ):
                os.environ["GIT_CITC_OUTPUT_TYPE"] = "typical"

                service = sync_workspace.WorkspaceSyncService()

                affected = service.affected_files(
                    sync_workspace.WorkspaceType.COG
                )

                expected = {
                    "fuchsia/placeholder.txt",
                    "fuchsia/third_party/some_dep/src/placeholder.txt",
                }
                self.assertEqual(affected, expected)

                # Check concurrency
                if self.signal_file.exists():
                    content = self.signal_file.read_text().splitlines()
                    self.assertEqual(len(content), 4)

                    starts = [line for line in content if "start" in line]
                    ends = [line for line in content if "end" in line]

                    self.assertEqual(len(starts), 2)
                    self.assertEqual(len(ends), 2)

                    ranges: list[dict[str, Any]] = []
                    for line in content:
                        parts = line.split()
                        pid = parts[0]
                        event = parts[1]
                        ts = float(parts[2])
                        pid_range = next(
                            (r for r in ranges if r["pid"] == pid), None
                        )
                        if not pid_range:
                            pid_range = {"pid": pid}
                            ranges.append(pid_range)
                        pid_range[event] = ts

                    self.assertEqual(len(ranges), 2)

                    r1 = ranges[0]
                    r2 = ranges[1]

                    overlap = not (
                        r1["end"] < r2["start"] or r2["end"] < r1["start"]
                    )
                    self.assertTrue(overlap, "Executions did not overlap")
                else:
                    self.fail("Signal file was not created")

    def test_affected_files_superproject(self) -> None:
        """Test affected_files in Superproject case."""
        with mock_fs.FileSystemTestHelper() as fs:
            config_path = fs.repo_dir / "scripts" / "cog" / "repo_config.json"
            config_path.parent.mkdir(exist_ok=True, parents=True)
            with open(config_path, "w") as f:
                f.write(
                    """{
  "repo": {
    "ignored": ["superproject"],
    "fuchsia": "superproject/fuchsia",
    "integration": "superproject/integration",
    "stripSrcPrefix": "superproject/",
    "destSubdir": "superproject"
  }
}"""
                )

            # Create directories for mock repos
            cog_root = fs.repo_dir.parent
            (cog_root / "superproject").mkdir(parents=True, exist_ok=True)
            (
                cog_root
                / "superproject"
                / "fuchsia"
                / "third_party"
                / "some_dep"
                / "src"
            ).mkdir(parents=True, exist_ok=True)
            (cog_root / "superproject" / "integration").mkdir(
                parents=True, exist_ok=True
            )
            (cog_root / "superproject" / "vendor" / "company").mkdir(
                parents=True, exist_ok=True
            )

            mock_ws = MagicMock()
            mock_ws.workspace_root = fs.repo_dir.parent
            mock_ws.cartfs_dir = fs.cartfs_dir
            mock_ws.config = {
                "repo": {
                    "ignored": ["superproject"],
                    "fuchsia": "superproject/fuchsia",
                    "integration": "superproject/integration",
                    "stripSrcPrefix": "superproject/",
                    "destSubdir": "superproject",
                }
            }

            def fake_run(
                cmd: list[str],
                cwd: str | Path | None = None,
                capture_output: bool = False,
                exit_on_error: bool = True,
            ) -> str:
                import subprocess

                res = subprocess.run(
                    cmd, cwd=cwd, capture_output=capture_output, text=True
                )
                return res.stdout

            mock_ws._run.side_effect = fake_run

            with (
                patch.object(os, "getcwd", return_value=str(fs.repo_dir)),
                patch(
                    "sync_workspace.workspace.Workspace", return_value=mock_ws
                ),
            ):
                os.environ["GIT_CITC_OUTPUT_TYPE"] = "superproject"

                service = sync_workspace.WorkspaceSyncService()

                affected = service.affected_files(
                    sync_workspace.WorkspaceType.COG
                )

                expected = {
                    "superproject/fuchsia/placeholder.txt",
                    "superproject/fuchsia/third_party/some_dep/src/placeholder.txt",
                    "superproject/integration/placeholder.txt",
                    "superproject/vendor/company/placeholder.txt",
                }
                self.assertEqual(affected, expected)

                # Verify no calls for ignored repo
                calls = mock_ws._run.call_args_list
                superproject_calls = [
                    c
                    for c in calls
                    if len(c[0][0]) > 2
                    and c[0][0][1] == "api.get-repo-states"
                    and c[0][0][2] == "superproject"
                ]
                self.assertEqual(
                    len(superproject_calls),
                    0,
                    "Should not call api.get-repo-states for ignored repo",
                )

                if self.signal_file.exists():
                    content = self.signal_file.read_text().splitlines()
                    self.assertEqual(len(content), 8)

                    ranges: list[dict[str, Any]] = []
                    for line in content:
                        parts = line.split()
                        pid = parts[0]
                        event = parts[1]
                        ts = float(parts[2])
                        pid_range = next(
                            (r for r in ranges if r["pid"] == pid), None
                        )
                        if not pid_range:
                            pid_range = {"pid": pid}
                            ranges.append(pid_range)
                        pid_range[event] = ts

                    self.assertEqual(len(ranges), 4)

                    overlapped = False
                    for i in range(len(ranges)):
                        for j in range(i + 1, len(ranges)):
                            r1 = ranges[i]
                            r2 = ranges[j]
                            if not (
                                r1["end"] < r2["start"]
                                or r2["end"] < r1["start"]
                            ):
                                overlapped = True
                                break
                        if overlapped:
                            break

                    self.assertTrue(
                        overlapped,
                        "No executions overlapped in superproject case",
                    )
                else:
                    self.fail("Signal file was not created")

    def test_affected_files_cartfs(self) -> None:
        """Test affected_files in CartFS case."""
        with mock_fs.FileSystemTestHelper() as fs:
            pass

            mock_ws = MagicMock()
            mock_ws.workspace_root = fs.repo_dir.parent
            mock_ws.cartfs_dir = fs.cartfs_dir
            mock_ws.config = {
                "repo": {
                    "ignored": [],
                    "fuchsia": "fuchsia",
                    "stripSrcPrefix": "fuchsia/",
                    "destSubdir": "fuchsia",
                }
            }

            def fake_run(
                cmd: list[str],
                cwd: str | Path | None = None,
                capture_output: bool = False,
                exit_on_error: bool = True,
            ) -> str:
                import subprocess

                res = subprocess.run(
                    cmd, cwd=cwd, capture_output=capture_output, text=True
                )
                return res.stdout

            mock_ws._run.side_effect = fake_run

            with (
                patch.object(os, "getcwd", return_value=str(fs.repo_dir)),
                patch(
                    "sync_workspace.workspace.Workspace", return_value=mock_ws
                ),
            ):
                hashes_file = fs.cartfs_dir / "cog_transfer_file_hashes.json"
                hashes_file.write_text(
                    """{
                    "fuchsia/foo/bar.json": "94f89aec76f20839d601ee791b11166e",
                    "fuchsia/foo/baz.rs": "108516e4946f8672ca19c03ac11c41f1"
                }"""
                )

                # Create a file in CartFS that is not in hashes file
                (fs.cartfs_dir / "fuchsia").mkdir(parents=True, exist_ok=True)
                (fs.cartfs_dir / "fuchsia" / "new_file.txt").write_text(
                    "new file"
                )

                service = sync_workspace.WorkspaceSyncService()

                affected = service.affected_files(
                    sync_workspace.WorkspaceType.CARTFS
                )

                expected = {
                    "fuchsia/foo/bar.json",
                    "fuchsia/foo/baz.rs",
                }
                self.assertEqual(affected, expected)

                # TODO(https://fxbug.dev/501540419): Swap this once
                # `WorkspaceSyncService.affected_files(WorkspaceType.CARTFS)` supports detecting
                # new files.
                self.assertNotIn("fuchsia/new_file.txt", affected)
                # self.assertIn("fuchsia/new_file.txt", affected)

    def test_cog_transfer_file_hashes_malformed(self) -> None:
        """Test handling of malformed JSON in cog_transfer_file_hashes.json."""
        with mock_fs.FileSystemTestHelper() as fs:
            mock_ws = MagicMock()
            mock_ws.workspace_root = fs.repo_dir.parent
            mock_ws.cartfs_dir = fs.cartfs_dir

            with patch.object(workspace, "Workspace", return_value=mock_ws):
                service = sync_workspace.WorkspaceSyncService()

                hashes_file = fs.cartfs_dir / "cog_transfer_file_hashes.json"
                hashes_file.write_text("{ malformed json }")

                with patch.object(logger, "log_error") as mock_log:
                    result = service._cog_transfer_file_hashes
                    self.assertEqual(result, {})
                    mock_log.assert_called_once()

    def test_cartfs_path_typical(self) -> None:
        """Test cartfs_path in typical Fuchsia case."""
        with mock_fs.FileSystemTestHelper() as fs:
            mock_ws = MagicMock()
            mock_ws.workspace_root = fs.repo_dir.parent
            mock_ws.cartfs_dir = fs.cartfs_dir
            mock_ws.config = {
                "repo": {
                    "ignored": [],
                    "fuchsia": "fuchsia",
                    "stripSrcPrefix": "fuchsia/",
                    "destSubdir": "fuchsia",
                }
            }

            with (
                patch.object(os, "getcwd", return_value=str(fs.repo_dir)),
                patch(
                    "sync_workspace.workspace.Workspace", return_value=mock_ws
                ),
            ):
                service = sync_workspace.WorkspaceSyncService()

                self.assertEqual(
                    service.cartfs_path("fuchsia/foo/bar.rs"),
                    fs.cartfs_dir / "fuchsia" / "foo" / "bar.rs",
                )

    def test_cartfs_path_lstrip_slash(self) -> None:
        """Test cartfs_path strips leading slash to prevent path escape."""
        with mock_fs.FileSystemTestHelper() as fs:
            mock_ws = MagicMock()
            mock_ws.workspace_root = fs.repo_dir.parent
            mock_ws.cartfs_dir = fs.cartfs_dir
            mock_ws.config = {
                "repo": {
                    "ignored": [],
                    "fuchsia": "fuchsia",
                    "stripSrcPrefix": "tools",
                    "destSubdir": "",
                    "integration": None,
                }
            }

            with (
                patch.object(os, "getcwd", return_value=str(fs.repo_dir)),
                patch(
                    "sync_workspace.workspace.Workspace", return_value=mock_ws
                ),
            ):
                service = sync_workspace.WorkspaceSyncService()

                self.assertEqual(
                    service.cartfs_path("tools/my_script.py"),
                    fs.cartfs_dir / "my_script.py",
                )

    def test_get_cog_commit_regex(self) -> None:
        """Test that _get_cog_commit handles variations in output."""
        with mock_fs.FileSystemTestHelper() as fs:
            mock_ws = MagicMock()

            with patch.object(workspace, "Workspace", return_value=mock_ws):
                service = sync_workspace.WorkspaceSyncService()

                with patch.object(
                    service,
                    "_git_citc",
                    return_value='base_commit_hash: "6224216ffe756b3682bc3910fc624f22bab449e0"',
                ):
                    commit = service._get_cog_commit("fuchsia")
                    self.assertEqual(
                        commit, "6224216ffe756b3682bc3910fc624f22bab449e0"
                    )

                with patch.object(
                    service,
                    "_git_citc",
                    return_value="base_commit_hash: 6224216ffe756b3682bc3910fc624f22bab449e0",
                ):
                    commit = service._get_cog_commit("fuchsia")
                    self.assertEqual(
                        commit, "6224216ffe756b3682bc3910fc624f22bab449e0"
                    )

                with patch.object(
                    service,
                    "_git_citc",
                    return_value='base_commit_hash:  "6224216ffe756b3682bc3910fc624f22bab449e0"',
                ):
                    commit = service._get_cog_commit("fuchsia")
                    self.assertEqual(
                        commit, "6224216ffe756b3682bc3910fc624f22bab449e0"
                    )

    def test_md5hash_symlink(self) -> None:
        """Test that _md5hash hashes the symlink target path."""
        with mock_fs.FileSystemTestHelper() as fs:
            mock_ws = MagicMock()

            with patch.object(workspace, "Workspace", return_value=mock_ws):
                service = sync_workspace.WorkspaceSyncService()

                target_file = fs.repo_dir / "target.txt"
                target_file.write_text("target content")

                symlink_file = fs.repo_dir / "symlink.txt"
                symlink_file.symlink_to(target_file)

                expected_hash = hashlib.md5(
                    str(target_file).encode()
                ).hexdigest()

                self.assertEqual(service._md5hash(symlink_file), expected_hash)

    def test_md5hash_symlink_to_directory(self) -> None:
        """Test that _md5hash hashes the target path of a symlink to a directory."""
        with mock_fs.FileSystemTestHelper() as fs:
            mock_ws = MagicMock()

            with patch.object(workspace, "Workspace", return_value=mock_ws):
                service = sync_workspace.WorkspaceSyncService()

                target_dir = fs.repo_dir / "target_dir"
                target_dir.mkdir()

                symlink_file = fs.repo_dir / "symlink_dir"
                symlink_file.symlink_to(target_dir)

                expected_hash = hashlib.md5(
                    str(target_dir).encode()
                ).hexdigest()

                self.assertEqual(service._md5hash(symlink_file), expected_hash)

    def test_md5hash_broken_symlink(self) -> None:
        """Test that _md5hash hashes the target path of a broken symlink."""
        with mock_fs.FileSystemTestHelper() as fs:
            mock_ws = MagicMock()

            with patch.object(workspace, "Workspace", return_value=mock_ws):
                service = sync_workspace.WorkspaceSyncService()

                target_file = fs.repo_dir / "non_existent.txt"

                symlink_file = fs.repo_dir / "broken_symlink"
                symlink_file.symlink_to(target_file)

                expected_hash = hashlib.md5(
                    str(target_file).encode()
                ).hexdigest()

                self.assertEqual(service._md5hash(symlink_file), expected_hash)

    def test_cartfs_path_superproject(self) -> None:
        """Test cartfs_path in Superproject case."""
        with mock_fs.FileSystemTestHelper() as fs:
            mock_ws = MagicMock()
            mock_ws.workspace_root = fs.repo_dir.parent
            mock_ws.cartfs_dir = fs.cartfs_dir
            mock_ws.config = {
                "repo": {
                    "ignored": [],
                    "fuchsia": "superproject/fuchsia",
                    "integration": "superproject/integration",
                    "stripSrcPrefix": "superproject/",
                    "destSubdir": "fuchsia",
                }
            }

            with (
                patch.object(os, "getcwd", return_value=str(fs.repo_dir)),
                patch(
                    "sync_workspace.workspace.Workspace", return_value=mock_ws
                ),
            ):
                service = sync_workspace.WorkspaceSyncService()

                # Test fuchsia file
                self.assertEqual(
                    service.cartfs_path("superproject/fuchsia/foo/bar.rs"),
                    fs.cartfs_dir / "fuchsia" / "foo" / "bar.rs",
                )

                # Test integration file
                self.assertEqual(
                    service.cartfs_path(
                        "superproject/integration/foo/bar.json"
                    ),
                    fs.cartfs_dir / "integration" / "foo" / "bar.json",
                )

                # Test vendor file (fallback case)
                self.assertEqual(
                    service.cartfs_path(
                        "superproject/vendor/company/foo/bar.rs"
                    ),
                    fs.cartfs_dir
                    / "fuchsia"
                    / "vendor"
                    / "company"
                    / "foo"
                    / "bar.rs",
                )

    def test_main_exception_calls_preflight(self) -> None:
        """Test that main calls preflight.check_all on exception."""
        with patch.object(sync_workspace, "_main") as mock_main, patch.object(
            preflight, "check_all"
        ) as mock_check_all, patch(
            "sync_workspace.logger.log_exception"
        ) as mock_log_exception:
            mock_main.side_effect = Exception("test error")
            result = sync_workspace.main()
            self.assertEqual(result, 1)
            mock_check_all.assert_called_once()
            mock_log_exception.assert_called_once_with(
                "An unexpected error occurred:"
            )

    def test_main_keyboard_interrupt(self) -> None:
        """Test that main returns 130 on KeyboardInterrupt."""
        with patch.object(sync_workspace, "_main") as mock_main, patch(
            "sync_workspace.logger.log_error"
        ) as mock_log_error:
            mock_main.side_effect = KeyboardInterrupt()
            result = sync_workspace.main()
            self.assertEqual(result, 130)
            mock_log_error.assert_called_once_with(
                "Sync cancelled by user (KeyboardInterrupt)."
            )


class TestSyncBatch(TestWorkspaceSyncService):
    """Tests for sync_batch method."""

    def test_sync_batch_addition(self) -> None:
        """Test copying a new file."""
        with mock_fs.FileSystemTestHelper() as fs:
            mock_ws = MagicMock()
            mock_ws.workspace_root = fs.repo_dir.parent
            mock_ws.cartfs_dir = fs.cartfs_dir

            with patch.object(workspace, "Workspace", return_value=mock_ws):
                service = sync_workspace.WorkspaceSyncService()

                src_dir = fs.repo_dir / "src"
                dest_dir = fs.repo_dir / "dest"
                src_dir.mkdir()
                dest_dir.mkdir()

                src_file = src_dir / "foo.txt"
                src_file.write_text("hello")

                src_func = lambda p: src_dir / p
                dest_func = lambda p: dest_dir / p
                hash_func = lambda p: "hash" if p.exists() else None

                paths = {"foo.txt"}
                result = service.sync_batch(
                    src_func, dest_func, paths, hash_func
                )
                self.assertEqual(
                    result, sync_workspace.SyncResult(added={"foo.txt"})
                )
                self.assertTrue((dest_dir / "foo.txt").exists())
                self.assertEqual((dest_dir / "foo.txt").read_text(), "hello")

    def test_sync_batch_modification(self) -> None:
        """Test copying a modified file."""
        with mock_fs.FileSystemTestHelper() as fs:
            mock_ws = MagicMock()

            with patch.object(workspace, "Workspace", return_value=mock_ws):
                service = sync_workspace.WorkspaceSyncService()

                src_dir = fs.repo_dir / "src"
                dest_dir = fs.repo_dir / "dest"
                src_dir.mkdir()
                dest_dir.mkdir()

                src_file = src_dir / "foo.txt"
                src_file.write_text("new content")
                dest_file = dest_dir / "foo.txt"
                dest_file.write_text("old content")

                src_func = lambda p: src_dir / p
                dest_func = lambda p: dest_dir / p

                hashes = {str(src_file): "new_hash", str(dest_file): "old_hash"}
                hash_func = lambda p: hashes.get(str(p))

                paths = {"foo.txt"}
                result = service.sync_batch(
                    src_func, dest_func, paths, hash_func
                )
                self.assertEqual(
                    result, sync_workspace.SyncResult(modified={"foo.txt"})
                )
                self.assertEqual(dest_file.read_text(), "new content")

    def test_sync_batch_noop_identical(self) -> None:
        """Test identical files are not copied."""
        with mock_fs.FileSystemTestHelper() as fs:
            mock_ws = MagicMock()

            with patch.object(workspace, "Workspace", return_value=mock_ws):
                service = sync_workspace.WorkspaceSyncService()

                src_dir = fs.repo_dir / "src"
                dest_dir = fs.repo_dir / "dest"
                src_dir.mkdir()
                dest_dir.mkdir()

                src_file = src_dir / "foo.txt"
                src_file.write_text("content")
                dest_file = dest_dir / "foo.txt"
                dest_file.write_text("content")

                src_func = lambda p: src_dir / p
                dest_func = lambda p: dest_dir / p

                hash_func = lambda p: "same_hash" if p.exists() else None

                paths = {"foo.txt"}

                with patch("sync_workspace.shutil.copy2") as mock_copy:
                    result = service.sync_batch(
                        src_func, dest_func, paths, hash_func
                    )
                    self.assertEqual(
                        result, sync_workspace.SyncResult(noop={"foo.txt"})
                    )
                    mock_copy.assert_not_called()

    def test_sync_batch_deletion(self) -> None:
        """Test deleting a file."""
        with mock_fs.FileSystemTestHelper() as fs:
            mock_ws = MagicMock()

            with patch.object(workspace, "Workspace", return_value=mock_ws):
                service = sync_workspace.WorkspaceSyncService()

                src_dir = fs.repo_dir / "src"
                dest_dir = fs.repo_dir / "dest"
                src_dir.mkdir()
                dest_dir.mkdir()

                dest_file = dest_dir / "foo.txt"
                dest_file.write_text("content")

                src_func = lambda p: src_dir / p
                dest_func = lambda p: dest_dir / p
                hash_func = lambda p: "hash" if p.exists() else None

                paths = {"foo.txt"}
                result = service.sync_batch(
                    src_func, dest_func, paths, hash_func
                )
                self.assertEqual(
                    result, sync_workspace.SyncResult(deleted={"foo.txt"})
                )
                self.assertFalse(dest_file.exists())

    def test_sync_batch_deletion_noop(self) -> None:
        """Test deleting a file that doesn't exist on either side."""
        with mock_fs.FileSystemTestHelper() as fs:
            mock_ws = MagicMock()

            with patch.object(workspace, "Workspace", return_value=mock_ws):
                service = sync_workspace.WorkspaceSyncService()

                src_dir = fs.repo_dir / "src"
                dest_dir = fs.repo_dir / "dest"
                src_dir.mkdir()
                dest_dir.mkdir()

                src_func = lambda p: src_dir / p
                dest_func = lambda p: dest_dir / p
                hash_func = lambda p: "hash" if p.exists() else None

                paths = {"foo.txt"}
                result = service.sync_batch(
                    src_func, dest_func, paths, hash_func
                )
                self.assertEqual(
                    result, sync_workspace.SyncResult(noop={"foo.txt"})
                )

    def test_sync_batch_deletion_symlink(self) -> None:
        """Test deleting a symlink at destination."""
        with mock_fs.FileSystemTestHelper() as fs:
            mock_ws = MagicMock()

            with patch.object(workspace, "Workspace", return_value=mock_ws):
                service = sync_workspace.WorkspaceSyncService()

                src_dir = fs.repo_dir / "src"
                dest_dir = fs.repo_dir / "dest"
                src_dir.mkdir()
                dest_dir.mkdir()

                # Create a symlink at dest
                dest_file = dest_dir / "foo.txt"
                dest_file.symlink_to(fs.repo_dir / "non_existent")

                src_func = lambda p: src_dir / p
                dest_func = lambda p: dest_dir / p
                hash_func = lambda p: "hash" if p.exists() else None

                paths = {"foo.txt"}
                result = service.sync_batch(
                    src_func, dest_func, paths, hash_func
                )
                self.assertEqual(
                    result, sync_workspace.SyncResult(deleted={"foo.txt"})
                )
                self.assertFalse(dest_file.is_symlink())
                self.assertFalse(dest_file.exists())

    def test_sync_batch_path_resolution_error(self) -> None:
        """Test path resolution failure."""
        with mock_fs.FileSystemTestHelper() as fs:
            mock_ws = MagicMock()

            with patch.object(workspace, "Workspace", return_value=mock_ws):
                service = sync_workspace.WorkspaceSyncService()

                def failing_src_func(p: str) -> Path:
                    raise Exception("Failed to resolve")

                dest_func = lambda p: fs.repo_dir / p
                hash_func = lambda p: None

                paths = {"foo.txt"}

                with self.assertRaises(sync_workspace.SyncError):
                    service.sync_batch(
                        failing_src_func, dest_func, paths, hash_func
                    )

    def test_sync_batch_directory_skipped(self) -> None:
        """Test skipping directories."""
        with mock_fs.FileSystemTestHelper() as fs:
            mock_ws = MagicMock()

            with patch.object(workspace, "Workspace", return_value=mock_ws):
                service = sync_workspace.WorkspaceSyncService()

                src_dir = fs.repo_dir / "src"
                dest_dir = fs.repo_dir / "dest"
                src_dir.mkdir()
                dest_dir.mkdir()

                (src_dir / "foo_dir").mkdir()

                src_func = lambda p: src_dir / p
                dest_func = lambda p: dest_dir / p
                hash_func = lambda p: None

                paths = {"foo_dir"}
                result = service.sync_batch(
                    src_func, dest_func, paths, hash_func
                )
                self.assertEqual(
                    result, sync_workspace.SyncResult(failed={"foo_dir"})
                )

    def test_sync_batch_deletion_fails(self) -> None:
        """Test failure to delete file."""
        with mock_fs.FileSystemTestHelper() as fs:
            mock_ws = MagicMock()

            with patch.object(workspace, "Workspace", return_value=mock_ws):
                service = sync_workspace.WorkspaceSyncService()

                src_dir = fs.repo_dir / "src"
                dest_dir = fs.repo_dir / "dest"
                src_dir.mkdir()
                dest_dir.mkdir()

                dest_file = dest_dir / "foo.txt"
                dest_file.write_text("content")

                src_func = lambda p: src_dir / p
                dest_func = lambda p: dest_dir / p
                hash_func = lambda p: None

                paths = {"foo.txt"}

                original_unlink = Path.unlink

                def mock_unlink(path_instance: Path) -> None:
                    if str(path_instance) == str(dest_file):
                        raise OSError("Permission denied")
                    return original_unlink(path_instance)

                with patch.object(Path, "unlink", autospec=True) as mock_unl:
                    mock_unl.side_effect = mock_unlink
                    result = service.sync_batch(
                        src_func, dest_func, paths, hash_func
                    )
                    self.assertEqual(
                        result, sync_workspace.SyncResult(failed={"foo.txt"})
                    )

    def test_sync_batch_copy_fails(self) -> None:
        """Test failure to copy file."""
        with mock_fs.FileSystemTestHelper() as fs:
            mock_ws = MagicMock()

            with patch.object(workspace, "Workspace", return_value=mock_ws):
                service = sync_workspace.WorkspaceSyncService()

                src_dir = fs.repo_dir / "src"
                dest_dir = fs.repo_dir / "dest"
                src_dir.mkdir()
                dest_dir.mkdir()

                src_file = src_dir / "foo.txt"
                src_file.write_text("content")

                src_func = lambda p: src_dir / p
                dest_func = lambda p: dest_dir / p
                hash_func = lambda p: str(p)

                paths = {"foo.txt"}

                with patch("sync_workspace.shutil.copy2") as mock_copy:
                    mock_copy.side_effect = OSError("Permission denied")
                    result = service.sync_batch(
                        src_func, dest_func, paths, hash_func
                    )
                    self.assertEqual(
                        result, sync_workspace.SyncResult(failed={"foo.txt"})
                    )

    def test_sync_batch_overwrites_symlink(self) -> None:
        """Test that sync_batch unlinks a symlink at destination before copying."""
        with mock_fs.FileSystemTestHelper() as fs:
            mock_ws = MagicMock()

            with patch.object(workspace, "Workspace", return_value=mock_ws):
                service = sync_workspace.WorkspaceSyncService()

                src_dir = fs.repo_dir / "src"
                src_dir.mkdir()
                src_file = src_dir / "foo.txt"
                src_file.write_text("new content")

                dest_dir = fs.cartfs_dir / "dest"
                dest_dir.mkdir()
                dest_file = dest_dir / "foo.txt"

                target_file = fs.cartfs_dir / "target.txt"
                target_file.write_text("target content")
                dest_file.symlink_to(target_file)

                src_func = lambda p: src_dir / p
                dest_func = lambda p: dest_dir / p
                hash_func = lambda p: str(p)

                paths = {"foo.txt"}

                result = service.sync_batch(
                    src_func, dest_func, paths, hash_func
                )

                self.assertEqual(
                    result, sync_workspace.SyncResult(modified={"foo.txt"})
                )

                self.assertFalse(dest_file.is_symlink())
                self.assertTrue(dest_file.is_file())
                self.assertEqual(dest_file.read_text(), "new content")
                self.assertEqual(target_file.read_text(), "target content")

    def test_sync_batch_symlink_to_directory(self) -> None:
        """Test that sync_batch does not skip a symlink to a directory."""
        with mock_fs.FileSystemTestHelper() as fs:
            mock_ws = MagicMock()

            with patch.object(workspace, "Workspace", return_value=mock_ws):
                service = sync_workspace.WorkspaceSyncService()

                src_dir = fs.repo_dir / "src"
                src_dir.mkdir()

                target_dir = fs.repo_dir / "target_dir"
                target_dir.mkdir()

                src_file = src_dir / "symlink_dir"
                src_file.symlink_to(target_dir)

                dest_dir = fs.cartfs_dir / "dest"
                dest_dir.mkdir()

                src_func = lambda p: src_dir / p
                dest_func = lambda p: dest_dir / p
                hash_func = lambda p: str(p)

                paths = {"symlink_dir"}

                result = service.sync_batch(
                    src_func, dest_func, paths, hash_func
                )

                self.assertEqual(
                    result, sync_workspace.SyncResult(added={"symlink_dir"})
                )

                dest_file = dest_dir / "symlink_dir"
                self.assertTrue(dest_file.is_symlink())

    def test_sync_batch_broken_symlink(self) -> None:
        """Test that sync_batch does not treat broken symlink as deletion."""
        with mock_fs.FileSystemTestHelper() as fs:
            mock_ws = MagicMock()

            with patch.object(workspace, "Workspace", return_value=mock_ws):
                service = sync_workspace.WorkspaceSyncService()

                src_dir = fs.repo_dir / "src"
                src_dir.mkdir()

                src_file = src_dir / "broken_symlink"
                src_file.symlink_to(fs.repo_dir / "non_existent")

                dest_dir = fs.cartfs_dir / "dest"
                dest_dir.mkdir()

                dest_file = dest_dir / "broken_symlink"
                target_file = dest_dir / "target.txt"
                target_file.write_text("target content")
                dest_file.symlink_to(target_file)

                src_func = lambda p: src_dir / p

                dest_func = lambda p: dest_dir / p
                hash_func = lambda p: str(p)

                paths = {"broken_symlink"}

                result = service.sync_batch(
                    src_func, dest_func, paths, hash_func
                )

                self.assertEqual(
                    result,
                    sync_workspace.SyncResult(modified={"broken_symlink"}),
                )

                self.assertTrue(dest_file.is_symlink())

    def test_sync_batch_overwrite_file_with_symlink(self) -> None:
        """Test that sync_batch overwrites a regular file with a symlink."""
        with mock_fs.FileSystemTestHelper() as fs:
            mock_ws = MagicMock()

            with patch.object(workspace, "Workspace", return_value=mock_ws):
                service = sync_workspace.WorkspaceSyncService()

                src_dir = fs.repo_dir / "src"
                src_dir.mkdir()

                target_file = fs.repo_dir / "target.txt"
                target_file.write_text("target content")

                src_file = src_dir / "symlink_file"
                src_file.symlink_to(target_file)

                dest_dir = fs.cartfs_dir / "dest"
                dest_dir.mkdir()

                dest_file = dest_dir / "symlink_file"
                dest_file.write_text("existing file content")

                src_func = lambda p: src_dir / p
                dest_func = lambda p: dest_dir / p

                hash_func = lambda p: "hash1" if p == src_file else "hash2"

                paths = {"symlink_file"}

                result = service.sync_batch(
                    src_func, dest_func, paths, hash_func
                )

                self.assertEqual(
                    result,
                    sync_workspace.SyncResult(modified={"symlink_file"}),
                )

                self.assertTrue(dest_file.is_symlink())
                self.assertEqual(os.readlink(dest_file), str(target_file))


class TestSyncCogToCartFS(TestWorkspaceSyncService):
    """Tests for sync_cog_to_cartfs method."""

    def test_sync_cog_to_cartfs_success(self) -> None:
        """Test successful sync from Cog to CartFS."""
        with mock_fs.FileSystemTestHelper() as fs:
            mock_ws = MagicMock()
            mock_ws.workspace_root = fs.repo_dir
            mock_ws.cartfs_dir = fs.cartfs_dir
            mock_ws.cartfs_root = fs.cartfs_dir
            mock_ws.has_cartfs_dir = True
            mock_ws.is_checkout_uptodate.return_value = False
            mock_ws.config = {
                "repo": {
                    "ignored": [],
                    "fuchsia": "fuchsia",
                    "stripSrcPrefix": "fuchsia/",
                    "destSubdir": "fuchsia",
                }
            }

            (fs.repo_dir / "fuchsia").mkdir(parents=True, exist_ok=True)
            (fs.cartfs_dir / "fuchsia").mkdir(parents=True, exist_ok=True)

            cog_file = fs.repo_dir / "fuchsia" / "foo.txt"
            cog_file.write_text("new content in cog")

            def fake_run(
                cmd: list[str],
                cwd: str | Path | None = None,
                capture_output: bool = False,
                exit_on_error: bool = True,
            ) -> str:
                import subprocess

                res = subprocess.run(
                    cmd, cwd=cwd, capture_output=capture_output, text=True
                )
                return res.stdout

            mock_ws._run.side_effect = fake_run

            with (
                patch.object(os, "getcwd", return_value=str(fs.repo_dir)),
                patch(
                    "sync_workspace.workspace.Workspace", return_value=mock_ws
                ),
                patch.dict(
                    os.environ,
                    {
                        "FAKE_MODIFIED_REPOS": "fuchsia",
                        "FAKE_CLI_DIFF_COG": "[M] foo.txt",
                        "FAKE_CLI_DIFF_CARTFS": "",
                    },
                ),
            ):
                service = sync_workspace.WorkspaceSyncService()

                service.sync_cog_to_cartfs()

                mock_ws.checkout_cartfs_to_cog_revisions.assert_called_once()

                cartfs_file = fs.cartfs_dir / "fuchsia" / "foo.txt"
                self.assertTrue(cartfs_file.exists())
                self.assertEqual(cartfs_file.read_text(), "new content in cog")

                hashes_file = fs.cartfs_dir / "cog_transfer_file_hashes.json"
                self.assertTrue(hashes_file.exists())
                import json

                hashes = json.loads(hashes_file.read_text())
                self.assertIn("fuchsia/foo.txt", hashes)

    def test_sync_cog_to_cartfs_auto_init(self) -> None:
        """Test that sync auto-initializes cartfs if missing."""
        with mock_fs.FileSystemTestHelper() as fs:
            mock_ws = MagicMock()
            mock_ws.workspace_root = fs.repo_dir
            mock_ws.cartfs_dir = fs.cartfs_dir
            mock_ws.cartfs_root = fs.cartfs_dir
            mock_ws.has_cartfs_dir = False
            mock_ws.is_checkout_uptodate.return_value = False
            mock_ws.config = {"repo": {"ignored": [], "fuchsia": "fuchsia"}}

            with (
                patch.object(os, "getcwd", return_value=str(fs.repo_dir)),
                patch(
                    "sync_workspace.workspace.Workspace", return_value=mock_ws
                ),
            ):
                service = sync_workspace.WorkspaceSyncService()

                with patch.object(
                    service, "affected_files", return_value=set()
                ):
                    service.sync_cog_to_cartfs()

                mock_ws.init_cartfs_workspace.assert_called_once()
                mock_ws.checkout_cartfs_to_cog_revisions.assert_called_once()

    def test_sync_cog_to_cartfs_noop(self) -> None:
        """Test sync when no files are modified."""
        with mock_fs.FileSystemTestHelper() as fs:
            mock_ws = MagicMock()
            mock_ws.workspace_root = fs.repo_dir
            mock_ws.cartfs_dir = fs.cartfs_dir
            mock_ws.cartfs_root = fs.cartfs_dir
            mock_ws.config = {"repo": {"ignored": [], "fuchsia": "fuchsia"}}

            (fs.repo_dir / "fuchsia").mkdir(parents=True, exist_ok=True)

            def fake_run(
                cmd: list[str],
                cwd: str | Path | None = None,
                capture_output: bool = False,
                exit_on_error: bool = True,
            ) -> str:
                import subprocess

                res = subprocess.run(
                    cmd, cwd=cwd, capture_output=capture_output, text=True
                )
                return res.stdout

            mock_ws._run.side_effect = fake_run

            with (
                patch.object(os, "getcwd", return_value=str(fs.repo_dir)),
                patch(
                    "sync_workspace.workspace.Workspace", return_value=mock_ws
                ),
                patch.dict(
                    os.environ, {"FAKE_MODIFIED_REPOS": "", "FAKE_CLI_DIFF": ""}
                ),
            ):
                service = sync_workspace.WorkspaceSyncService()

                result = service.sync_cog_to_cartfs()

                self.assertEqual(len(result.added), 0)
                self.assertEqual(len(result.modified), 0)

                hashes_file = fs.cartfs_dir / "cog_transfer_file_hashes.json"
                self.assertTrue(hashes_file.exists())
                import json

                hashes = json.loads(hashes_file.read_text())
                self.assertEqual(hashes, {})

    def test_sync_cog_to_cartfs_failed_retained(self) -> None:
        """Test that failed syncs are still retained in hashes file."""
        with mock_fs.FileSystemTestHelper() as fs:
            mock_ws = MagicMock()
            mock_ws.workspace_root = fs.repo_dir
            mock_ws.cartfs_dir = fs.cartfs_dir
            mock_ws.cartfs_root = fs.cartfs_dir
            mock_ws.config = {"repo": {"ignored": [], "fuchsia": "fuchsia"}}

            (fs.repo_dir / "fuchsia").mkdir(parents=True, exist_ok=True)
            (fs.cartfs_dir / "fuchsia").mkdir(parents=True, exist_ok=True)

            cog_file = fs.repo_dir / "fuchsia" / "foo.txt"
            cog_file.write_text("content")

            # Create a DIRECTORY in CartFS with the same name to cause failure
            cartfs_file = fs.cartfs_dir / "fuchsia" / "foo.txt"
            cartfs_file.mkdir()

            def fake_run(
                cmd: list[str],
                cwd: str | Path | None = None,
                capture_output: bool = False,
                exit_on_error: bool = True,
            ) -> str:
                import subprocess

                res = subprocess.run(
                    cmd, cwd=cwd, capture_output=capture_output, text=True
                )
                return res.stdout

            mock_ws._run.side_effect = fake_run

            with (
                patch.object(os, "getcwd", return_value=str(fs.repo_dir)),
                patch(
                    "sync_workspace.workspace.Workspace", return_value=mock_ws
                ),
                patch.dict(
                    os.environ,
                    {
                        "FAKE_MODIFIED_REPOS": "fuchsia",
                        "FAKE_CLI_DIFF_COG": "[M] foo.txt",
                        "FAKE_CLI_DIFF_CARTFS": "",
                    },
                ),
            ):
                service = sync_workspace.WorkspaceSyncService()

                result = service.sync_cog_to_cartfs()

                self.assertIn("fuchsia/foo.txt", result.failed)

                hashes_file = fs.cartfs_dir / "cog_transfer_file_hashes.json"
                self.assertTrue(hashes_file.exists())
                import json

                hashes = json.loads(hashes_file.read_text())
                self.assertIn("fuchsia/foo.txt", hashes)


class TestSyncCartFSToCog(TestWorkspaceSyncService):
    """Tests for sync_cartfs_to_cog method."""

    def test_sync_cartfs_to_cog_success(self) -> None:
        """Test successful sync from CartFS to Cog."""
        with mock_fs.FileSystemTestHelper() as fs:
            mock_ws = MagicMock()
            mock_ws.workspace_root = fs.repo_dir
            mock_ws.cartfs_dir = fs.cartfs_dir
            mock_ws.cartfs_root = fs.cartfs_dir
            mock_ws.config = {"repo": {"ignored": [], "fuchsia": "fuchsia"}}

            (fs.repo_dir / "fuchsia").mkdir(parents=True, exist_ok=True)
            (fs.cartfs_dir / "fuchsia").mkdir(parents=True, exist_ok=True)

            # Create file in CartFS
            cartfs_file = fs.cartfs_dir / "fuchsia" / "foo.txt"
            cartfs_file.write_text("content in cartfs")

            # Create hashes file pointing to it
            hashes_file = fs.cartfs_dir / "cog_transfer_file_hashes.json"
            hashes_file.write_text('{"fuchsia/foo.txt": "hash"}')

            def fake_run(
                cmd: list[str],
                cwd: str | Path | None = None,
                capture_output: bool = False,
                exit_on_error: bool = True,
            ) -> str:
                import subprocess

                res = subprocess.run(
                    cmd, cwd=cwd, capture_output=capture_output, text=True
                )
                return res.stdout

            mock_ws._run.side_effect = fake_run

            with (
                patch.object(os, "getcwd", return_value=str(fs.repo_dir)),
                patch(
                    "sync_workspace.workspace.Workspace", return_value=mock_ws
                ),
                patch.dict(
                    os.environ, {"FAKE_MODIFIED_REPOS": "", "FAKE_CLI_DIFF": ""}
                ),
            ):
                service = sync_workspace.WorkspaceSyncService()

                result = service.sync_cartfs_to_cog(
                    diff_against_previous_cog_to_cartfs_sync=False
                )

                cog_file = fs.repo_dir / "fuchsia" / "foo.txt"
                self.assertTrue(cog_file.exists())
                self.assertEqual(cog_file.read_text(), "content in cartfs")

    def test_sync_cartfs_to_cog_noop(self) -> None:
        """Test sync when no files differ."""
        with mock_fs.FileSystemTestHelper() as fs:
            mock_ws = MagicMock()
            mock_ws.workspace_root = fs.repo_dir
            mock_ws.cartfs_dir = fs.cartfs_dir
            mock_ws.cartfs_root = fs.cartfs_dir
            mock_ws.config = {"repo": {"ignored": [], "fuchsia": "fuchsia"}}

            (fs.repo_dir / "fuchsia").mkdir(parents=True, exist_ok=True)
            (fs.cartfs_dir / "fuchsia").mkdir(parents=True, exist_ok=True)

            # Create identical files
            content = "identical"
            (fs.repo_dir / "fuchsia" / "foo.txt").write_text(content)
            (fs.cartfs_dir / "fuchsia" / "foo.txt").write_text(content)

            # Create hashes file
            hashes_file = fs.cartfs_dir / "cog_transfer_file_hashes.json"
            import hashlib

            h = hashlib.md5(content.encode()).hexdigest()
            hashes_file.write_text(f'{{"fuchsia/foo.txt": "{h}"}}')

            def fake_run(
                cmd: list[str],
                cwd: str | Path | None = None,
                capture_output: bool = False,
                exit_on_error: bool = True,
            ) -> str:
                import subprocess

                res = subprocess.run(
                    cmd, cwd=cwd, capture_output=capture_output, text=True
                )
                return res.stdout

            mock_ws._run.side_effect = fake_run

            with (
                patch.object(os, "getcwd", return_value=str(fs.repo_dir)),
                patch(
                    "sync_workspace.workspace.Workspace", return_value=mock_ws
                ),
                patch.dict(
                    os.environ, {"FAKE_MODIFIED_REPOS": "", "FAKE_CLI_DIFF": ""}
                ),
            ):
                service = sync_workspace.WorkspaceSyncService()

                result = service.sync_cartfs_to_cog(
                    diff_against_previous_cog_to_cartfs_sync=False
                )

                self.assertEqual(len(result.added), 0)
                self.assertEqual(len(result.modified), 0)

    def test_sync_cartfs_to_cog_protection(self) -> None:
        """Test protection of Cog edits."""
        with mock_fs.FileSystemTestHelper() as fs:
            mock_ws = MagicMock()
            mock_ws.workspace_root = fs.repo_dir
            mock_ws.cartfs_dir = fs.cartfs_dir
            mock_ws.cartfs_root = fs.cartfs_dir
            mock_ws.config = {"repo": {"ignored": [], "fuchsia": "fuchsia"}}

            (fs.repo_dir / "fuchsia").mkdir(parents=True, exist_ok=True)
            (fs.cartfs_dir / "fuchsia").mkdir(parents=True, exist_ok=True)

            # Cog file modified
            cog_file = fs.repo_dir / "fuchsia" / "foo.txt"
            cog_file.write_text("cog edits")

            # CartFS file not modified (relative to last sync)
            cartfs_file = fs.cartfs_dir / "fuchsia" / "foo.txt"
            cartfs_file.write_text("base content")

            # Hashes file has base hash
            import hashlib

            base_hash = hashlib.md5(b"base content").hexdigest()
            hashes_file = fs.cartfs_dir / "cog_transfer_file_hashes.json"
            hashes_file.write_text(f'{{"fuchsia/foo.txt": "{base_hash}"}}')

            def fake_run(
                cmd: list[str],
                cwd: str | Path | None = None,
                capture_output: bool = False,
                exit_on_error: bool = True,
            ) -> str:
                import subprocess

                res = subprocess.run(
                    cmd, cwd=cwd, capture_output=capture_output, text=True
                )
                return res.stdout

            mock_ws._run.side_effect = fake_run

            with (
                patch.object(os, "getcwd", return_value=str(fs.repo_dir)),
                patch(
                    "sync_workspace.workspace.Workspace", return_value=mock_ws
                ),
                patch.dict(
                    os.environ, {"FAKE_MODIFIED_REPOS": "", "FAKE_CLI_DIFF": ""}
                ),
            ):
                service = sync_workspace.WorkspaceSyncService()

                result = service.sync_cartfs_to_cog(
                    diff_against_previous_cog_to_cartfs_sync=True
                )

                # Cog file should NOT be overwritten
                self.assertEqual(cog_file.read_text(), "cog edits")

    def test_sync_cartfs_to_cog_overwrite(self) -> None:
        """Test CartFS wins when both modified."""
        with mock_fs.FileSystemTestHelper() as fs:
            mock_ws = MagicMock()
            mock_ws.workspace_root = fs.repo_dir
            mock_ws.cartfs_dir = fs.cartfs_dir
            mock_ws.cartfs_root = fs.cartfs_dir
            mock_ws.config = {"repo": {"ignored": [], "fuchsia": "fuchsia"}}

            (fs.repo_dir / "fuchsia").mkdir(parents=True, exist_ok=True)
            (fs.cartfs_dir / "fuchsia").mkdir(parents=True, exist_ok=True)

            # Cog file modified
            cog_file = fs.repo_dir / "fuchsia" / "foo.txt"
            cog_file.write_text("cog edits")

            # CartFS file also modified
            cartfs_file = fs.cartfs_dir / "fuchsia" / "foo.txt"
            cartfs_file.write_text("cartfs edits")

            # Hashes file has OLD base hash
            hashes_file = fs.cartfs_dir / "cog_transfer_file_hashes.json"
            hashes_file.write_text('{"fuchsia/foo.txt": "old_hash"}')

            def fake_run(
                cmd: list[str],
                cwd: str | Path | None = None,
                capture_output: bool = False,
                exit_on_error: bool = True,
            ) -> str:
                import subprocess

                res = subprocess.run(
                    cmd, cwd=cwd, capture_output=capture_output, text=True
                )
                return res.stdout

            mock_ws._run.side_effect = fake_run

            with (
                patch.object(os, "getcwd", return_value=str(fs.repo_dir)),
                patch(
                    "sync_workspace.workspace.Workspace", return_value=mock_ws
                ),
                patch.dict(
                    os.environ, {"FAKE_MODIFIED_REPOS": "", "FAKE_CLI_DIFF": ""}
                ),
            ):
                service = sync_workspace.WorkspaceSyncService()

                result = service.sync_cartfs_to_cog(
                    diff_against_previous_cog_to_cartfs_sync=True
                )

                # Cog file SHOULD be overwritten
                self.assertEqual(cog_file.read_text(), "cartfs edits")

    def test_sync_cartfs_to_cog_ignored_new_file(self) -> None:
        """Test that new files in CartFS not in hashes file are ignored."""
        with mock_fs.FileSystemTestHelper() as fs:
            mock_ws = MagicMock()
            mock_ws.workspace_root = fs.repo_dir
            mock_ws.cartfs_dir = fs.cartfs_dir
            mock_ws.cartfs_root = fs.cartfs_dir
            mock_ws.config = {"repo": {"ignored": [], "fuchsia": "fuchsia"}}

            (fs.repo_dir / "fuchsia").mkdir(parents=True, exist_ok=True)
            (fs.cartfs_dir / "fuchsia").mkdir(parents=True, exist_ok=True)

            # New file in CartFS
            cartfs_file = fs.cartfs_dir / "fuchsia" / "new_file.txt"
            cartfs_file.write_text("new file content")

            # Hashes file does NOT contain it
            hashes_file = fs.cartfs_dir / "cog_transfer_file_hashes.json"
            hashes_file.write_text("{}")

            def fake_run(
                cmd: list[str],
                cwd: str | Path | None = None,
                capture_output: bool = False,
                exit_on_error: bool = True,
            ) -> str:
                import subprocess

                res = subprocess.run(
                    cmd, cwd=cwd, capture_output=capture_output, text=True
                )
                return res.stdout

            mock_ws._run.side_effect = fake_run

            with (
                patch.object(os, "getcwd", return_value=str(fs.repo_dir)),
                patch(
                    "sync_workspace.workspace.Workspace", return_value=mock_ws
                ),
                patch.dict(
                    os.environ, {"FAKE_MODIFIED_REPOS": "", "FAKE_CLI_DIFF": ""}
                ),
            ):
                service = sync_workspace.WorkspaceSyncService()

                result = service.sync_cartfs_to_cog(
                    diff_against_previous_cog_to_cartfs_sync=False
                )

                cog_file = fs.repo_dir / "fuchsia" / "new_file.txt"

                # TODO(https://fxbug.dev/501540419): Swap this once
                # `WorkspaceSyncService.affected_files(WorkspaceType.CARTFS)` supports detecting
                # new files.
                self.assertFalse(cog_file.exists())
                # self.assertTrue(cog_file.exists())

    def test_sync_cartfs_to_cog_ignores_cog_only_files(self) -> None:
        """Test that sync_cartfs_to_cog ignores files only modified in Cog."""
        with mock_fs.FileSystemTestHelper() as fs:
            mock_ws = MagicMock()
            mock_ws.workspace_root = fs.repo_dir
            mock_ws.cartfs_dir = fs.cartfs_dir
            mock_ws.cartfs_root = fs.cartfs_dir
            mock_ws.config = {"repo": {"ignored": [], "fuchsia": "fuchsia"}}

            (fs.repo_dir / "fuchsia").mkdir(parents=True, exist_ok=True)
            (fs.cartfs_dir / "fuchsia").mkdir(parents=True, exist_ok=True)

            # New file in Cog
            cog_file = fs.repo_dir / "fuchsia" / "new_cog_file.txt"
            cog_file.write_text("new file in cog")

            # Hashes file empty
            hashes_file = fs.cartfs_dir / "cog_transfer_file_hashes.json"
            hashes_file.write_text("{}")

            def fake_run(
                cmd: list[str],
                cwd: str | Path | None = None,
                capture_output: bool = False,
                exit_on_error: bool = True,
            ) -> str:
                import subprocess

                res = subprocess.run(
                    cmd, cwd=cwd, capture_output=capture_output, text=True
                )
                return res.stdout

            mock_ws._run.side_effect = fake_run

            with (
                patch.object(os, "getcwd", return_value=str(fs.repo_dir)),
                patch(
                    "sync_workspace.workspace.Workspace", return_value=mock_ws
                ),
                patch.dict(
                    os.environ, {"FAKE_MODIFIED_REPOS": "", "FAKE_CLI_DIFF": ""}
                ),
                patch.object(
                    sync_workspace.WorkspaceSyncService,
                    "affected_files",
                    side_effect=lambda ws_type: {"fuchsia/new_cog_file.txt"}
                    if ws_type == sync_workspace.WorkspaceType.COG
                    else set(),
                ),
            ):
                service = sync_workspace.WorkspaceSyncService()

                result = service.sync_cartfs_to_cog(
                    diff_against_previous_cog_to_cartfs_sync=False
                )

                # Cog file should STILL exist and NOT be deleted
                self.assertTrue(cog_file.exists())
                self.assertEqual(cog_file.read_text(), "new file in cog")

                self.assertNotIn("fuchsia/new_cog_file.txt", result.deleted)
                self.assertNotIn("fuchsia/new_cog_file.txt", result.failed)


class TestEnsureCartfsCwd(TestWorkspaceSyncService):
    """Tests for ensure_cartfs_cwd method."""

    def test_ensure_cartfs_cwd_typical(self) -> None:
        with mock_fs.FileSystemTestHelper() as fs:
            mock_ws = MagicMock()
            mock_ws.workspace_root = fs.repo_dir.parent
            mock_ws.cartfs_dir = fs.cartfs_dir
            mock_ws.config = {"repo": {"fuchsia": "fuchsia"}}

            cog_root = fs.repo_dir.parent
            cog_fuchsia_dir = cog_root / "fuchsia"
            cog_fuchsia_dir.mkdir(parents=True, exist_ok=True)

            with (
                patch(
                    "sync_workspace.workspace.Workspace", return_value=mock_ws
                ),
                patch.dict(os.environ, {"PWD": str(cog_fuchsia_dir)}),
            ):
                service = sync_workspace.WorkspaceSyncService()
                cwd = service.ensure_cartfs_cwd()
                self.assertEqual(cwd, fs.cartfs_dir / "fuchsia")

    def test_ensure_cartfs_cwd_outside(self) -> None:
        with mock_fs.FileSystemTestHelper() as fs:
            mock_ws = MagicMock()
            mock_ws.workspace_root = fs.repo_dir.parent
            mock_ws.cartfs_dir = fs.cartfs_dir
            mock_ws.config = {"repo": {"fuchsia": "fuchsia"}}

            cog_root = fs.repo_dir.parent
            cog_fuchsia_dir = cog_root / "fuchsia"
            cog_fuchsia_dir.mkdir(parents=True, exist_ok=True)

            outside_dir = cog_root / "bar"
            outside_dir.mkdir(parents=True, exist_ok=True)

            with (
                patch(
                    "sync_workspace.workspace.Workspace", return_value=mock_ws
                ),
                patch.dict(os.environ, {"PWD": str(outside_dir)}),
                patch.object(logger, "log_warn") as mock_log,
            ):
                service = sync_workspace.WorkspaceSyncService()
                cwd = service.ensure_cartfs_cwd()
                self.assertEqual(cwd, fs.cartfs_dir / "fuchsia")
                mock_log.assert_called_once()

    def test_ensure_cartfs_cwd_mkdir(self) -> None:
        with mock_fs.FileSystemTestHelper() as fs:
            mock_ws = MagicMock()
            mock_ws.workspace_root = fs.repo_dir.parent
            mock_ws.cartfs_dir = fs.cartfs_dir
            mock_ws.config = {"repo": {"fuchsia": "fuchsia"}}

            cog_root = fs.repo_dir.parent
            cog_fuchsia_dir = cog_root / "fuchsia"
            cog_fuchsia_dir.mkdir(parents=True, exist_ok=True)

            sub_dir = cog_fuchsia_dir / "new_dir"
            sub_dir.mkdir(parents=True, exist_ok=True)

            cartfs_target = fs.cartfs_dir / "fuchsia" / "new_dir"

            with (
                patch(
                    "sync_workspace.workspace.Workspace", return_value=mock_ws
                ),
                patch.dict(os.environ, {"PWD": str(sub_dir)}),
            ):
                service = sync_workspace.WorkspaceSyncService()
                self.assertFalse(cartfs_target.exists())
                cwd = service.ensure_cartfs_cwd()
                self.assertEqual(cwd, cartfs_target)
                self.assertTrue(cartfs_target.exists())

    def test_ensure_cartfs_cwd_symlink_cog(self) -> None:
        with mock_fs.FileSystemTestHelper() as fs:
            mock_ws = MagicMock()
            mock_ws.workspace_root = fs.repo_dir.parent
            mock_ws.cartfs_dir = fs.cartfs_dir
            mock_ws.config = {"repo": {"fuchsia": "superproject/fuchsia"}}

            cog_root = fs.repo_dir.parent
            cog_fuchsia_dir = cog_root / "superproject/fuchsia"
            cog_fuchsia_dir.mkdir(parents=True, exist_ok=True)

            # Create superproject/vendor/company
            vendor_company = cog_root / "superproject/vendor/company"
            vendor_company.mkdir(parents=True, exist_ok=True)

            # Create symlink superproject/fuchsia/vendor -> ../vendor
            symlink_vendor = cog_fuchsia_dir / "vendor"
            symlink_vendor.symlink_to("../vendor")

            cog_cwd = symlink_vendor / "company"

            with (
                patch(
                    "sync_workspace.workspace.Workspace", return_value=mock_ws
                ),
                patch.dict(os.environ, {"PWD": str(cog_cwd)}),
            ):
                service = sync_workspace.WorkspaceSyncService()
                cwd = service.ensure_cartfs_cwd()
                self.assertEqual(
                    cwd, fs.cartfs_dir / "fuchsia" / "vendor" / "company"
                )

    def test_ensure_cartfs_cwd_symlink_cartfs(self) -> None:
        with mock_fs.FileSystemTestHelper() as fs:
            mock_ws = MagicMock()
            mock_ws.workspace_root = fs.repo_dir.parent
            mock_ws.cartfs_dir = fs.cartfs_dir
            mock_ws.config = {"repo": {"fuchsia": "superproject/fuchsia"}}

            cog_root = fs.repo_dir.parent
            cog_fuchsia_dir = cog_root / "superproject/fuchsia"
            cog_fuchsia_dir.mkdir(parents=True, exist_ok=True)

            vendor_company = cog_root / "superproject/vendor/company"
            vendor_company.mkdir(parents=True, exist_ok=True)

            symlink_vendor = cog_fuchsia_dir / "vendor"
            symlink_vendor.symlink_to("../vendor")

            cog_cwd = symlink_vendor / "company"

            # Setup CartFS symlink
            cartfs_fuchsia = fs.cartfs_dir / "fuchsia"
            cartfs_fuchsia.mkdir(parents=True, exist_ok=True)

            cartfs_vendor_target = fs.cartfs_dir / "vendor"
            cartfs_vendor_target.mkdir(parents=True, exist_ok=True)

            cartfs_vendor_symlink = cartfs_fuchsia / "vendor"
            cartfs_vendor_symlink.symlink_to("../vendor")

            cartfs_target = fs.cartfs_dir / "fuchsia" / "vendor" / "company"

            with (
                patch(
                    "sync_workspace.workspace.Workspace", return_value=mock_ws
                ),
                patch.dict(os.environ, {"PWD": str(cog_cwd)}),
            ):
                service = sync_workspace.WorkspaceSyncService()
                self.assertFalse((cartfs_vendor_target / "company").exists())
                cwd = service.ensure_cartfs_cwd()
                self.assertEqual(cwd, cartfs_target)
                # Verify that it created the directory in the target location
                self.assertTrue((cartfs_vendor_target / "company").exists())

    def test_main_report(self) -> None:
        """Test that main writes a report file when --report is used."""
        with mock_fs.FileSystemTestHelper() as fs:
            mock_ws = MagicMock()
            mock_ws.workspace_root = fs.repo_dir.parent
            mock_ws.cartfs_dir = fs.cartfs_dir
            mock_ws.config = {"repo": {"fuchsia": "fuchsia"}}

            mock_service = MagicMock()
            mock_service.ensure_cartfs_cwd.return_value = (
                fs.cartfs_dir / "fuchsia"
            )
            mock_service.cartfs_path.return_value = fs.cartfs_dir / "fuchsia"
            mock_service.cartfs_root = fs.cartfs_dir
            mock_service.sync_cog_to_cartfs.return_value = (
                sync_workspace.SyncResult()
            )

            report_file = fs.repo_dir / "report.json"

            with (
                patch(
                    "sync_workspace.workspace.Workspace", return_value=mock_ws
                ),
                patch(
                    "sync_workspace.WorkspaceSyncService",
                    return_value=mock_service,
                ),
                patch(
                    "sys.argv",
                    [
                        "sync_workspace.py",
                        "--from-cog-to-cartfs",
                        "--report",
                        str(report_file),
                    ],
                ),
            ):
                result_code = sync_workspace.main()
                self.assertEqual(result_code, 0)
                self.assertTrue(report_file.exists())

                report_content = report_file.read_text()
                self.assertIn(
                    f"CARTFS_CWD={shlex.quote(str(fs.cartfs_dir / 'fuchsia'))}\n",
                    report_content,
                )
                self.assertIn(
                    f"CARTFS_FUCHSIA_DIR={shlex.quote(str(fs.cartfs_dir / 'fuchsia'))}\n",
                    report_content,
                )


if __name__ == "__main__":
    unittest.main()
