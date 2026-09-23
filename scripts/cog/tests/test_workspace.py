# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Tests for workspace."""

import os
import subprocess
import threading
import time
import unittest
from pathlib import Path
from unittest.mock import MagicMock, PropertyMock, patch

import cartfs
import mock_fs
import workspace


class TestCogMetadata(unittest.TestCase):
    """Tests for CogMetadata."""

    def test_to_dict(self) -> None:
        """Test that the dictionary representation is correct."""
        metadata = workspace.CogMetadata(
            workspace_name="test-ws", repo_name="fuchsia", workspace_id="ws-id"
        )
        self.assertEqual(
            metadata.to_dict(),
            {
                "workspace_name": "test-ws",
                "repo_name": "fuchsia",
                "workspace_id": "ws-id",
            },
        )

    def test_from_file_success(self) -> None:
        """Test that metadata can be loaded from a file."""
        with mock_fs.FileSystemTestHelper() as fs:
            metadata_path = (
                fs.cartfs_dir / "test-ws" / workspace.COG_METADATA_FILE_NAME
            )
            metadata_path.parent.mkdir(exist_ok=True, parents=True)
            with open(metadata_path, "w") as f:
                f.write(
                    '{"workspace_name": "test-ws", "repo_name": "fuchsia", "workspace_id": "ws-id"}'
                )

            metadata = workspace.CogMetadata.from_file(metadata_path)
            self.assertIsNotNone(metadata)

            # This assert is needed to make mypy happy
            assert metadata is not None
            self.assertEqual(metadata.workspace_name, "test-ws")
            self.assertEqual(metadata.repo_name, "fuchsia")
            self.assertEqual(metadata.workspace_id, "ws-id")

    def test_from_file_not_found(self) -> None:
        """Test that None is returned when the file does not exist."""
        with mock_fs.FileSystemTestHelper():
            metadata = workspace.CogMetadata.from_file(
                Path("non-existent-file")
            )
            self.assertIsNone(metadata)

    def test_from_file_invalid_json(self) -> None:
        """Test that None is returned when the file contains invalid JSON."""
        with mock_fs.FileSystemTestHelper() as fs:
            metadata_path = (
                fs.cartfs_dir / "test-ws" / workspace.COG_METADATA_FILE_NAME
            )
            metadata_path.parent.mkdir(exist_ok=True, parents=True)
            with open(metadata_path, "w") as f:
                f.write("invalid-json")
            metadata = workspace.CogMetadata.from_file(metadata_path)
            self.assertIsNone(metadata)

    def test_from_file_missing_keys(self) -> None:
        """Test that None is returned when the file is missing keys."""
        with mock_fs.FileSystemTestHelper() as fs:
            metadata_path = (
                fs.cartfs_dir / "test-ws" / workspace.COG_METADATA_FILE_NAME
            )
            metadata_path.parent.mkdir(exist_ok=True, parents=True)
            with open(metadata_path, "w") as f:
                f.write('{"workspace_name": "test-ws"}')
            metadata = workspace.CogMetadata.from_file(metadata_path)
            self.assertIsNone(metadata)

    def test_write(self) -> None:
        """Test that metadata can be written to a file."""
        with mock_fs.FileSystemTestHelper() as fs:
            metadata = workspace.CogMetadata(
                workspace_name="test-ws",
                repo_name="fuchsia",
                workspace_id="ws-id",
            )
            metadata.write(fs.cartfs_dir)
            self.assertEqual(
                fs.read(
                    workspace.COG_METADATA_FILE_NAME, mock_fs.FSType.CARTFS
                ),
                '{\n    "workspace_name": "test-ws",\n    "repo_name": "fuchsia",\n    "workspace_id": "ws-id"\n}',
            )


class TestCartfs(unittest.TestCase):
    """Tests for Cartfs."""

    def test_suggest_cartfs_dir_name_handles_collision(self) -> None:
        """Test that a counter is appended when the directory exists."""
        with mock_fs.FileSystemTestHelper() as fs:
            with patch.object(
                cartfs.Cartfs, "find_mount_point", return_value=fs.cartfs_dir
            ):
                c = cartfs.Cartfs()
                (fs.cartfs_dir / "test-ws").mkdir()
                suggested_name = c.suggest_cartfs_dir_name(base_name="test-ws")
                self.assertEqual(str(suggested_name), "test-ws-1")


class TestWorkspace(unittest.TestCase):
    """Tests for Workspace."""

    def setUp(self) -> None:
        self.fs = mock_fs.FileSystemTestHelper()
        self.patcher = patch.object(
            cartfs.Cartfs, "find_mount_point", return_value=self.fs.cartfs_dir
        )
        self.patcher.start()

    def tearDown(self) -> None:
        self.patcher.stop()
        self.fs.cleanup()

    def test_config_success(self) -> None:
        """Test that config reads from file."""
        workspace_dir = self.fs.cog_dir / "testuser" / "test-workspace"
        repo_name = "fuchsia"
        config_path = (
            workspace_dir / repo_name / "scripts" / "cog" / "repo_config.json"
        )
        config_path.parent.mkdir(exist_ok=True, parents=True)
        with open(config_path, "w") as f:
            f.write('{"fuchsia": {"repo": "fuchsia"}}')

        ws = workspace.Workspace(self.fs.repo_dir)
        self.assertEqual(ws.config, {"fuchsia": {"repo": "fuchsia"}})

    def test_config_file_not_found(self) -> None:
        """Test that FileNotFoundError is raised when config file is missing."""
        ws = workspace.Workspace(self.fs.repo_dir)
        with self.assertRaises(FileNotFoundError):
            _ = ws.config

    def test_has_cartfs_dir_true(self) -> None:
        """Test that has_cartfs_dir returns True when cartfs_dir is available."""
        ws = workspace.Workspace(self.fs.repo_dir)
        ws.__dict__["cartfs_dir"] = self.fs.cartfs_dir
        self.assertTrue(ws.has_cartfs_dir)

    def test_has_cartfs_dir_false(self) -> None:
        """Test that has_cartfs_dir returns False when cartfs_dir is not available."""
        ws = workspace.Workspace(self.fs.repo_dir)
        with patch.object(ws, "_get_linked_cartfs_dir", return_value=None):
            self.assertFalse(ws.has_cartfs_dir)

    def test_cogd_path_success(self) -> None:
        """Test that cogd_path returns the path from git-citc."""
        workspace.Workspace.cogd_path.cache_clear()
        with patch("subprocess.check_output") as mock_check_output:
            mock_check_output.return_value = "/path/to/cogd\n"
            path = workspace.Workspace.cogd_path()
            self.assertEqual(path, Path("/path/to/cogd"))
            mock_check_output.assert_called_once_with(
                ["git-citc", "cogd"], text=True
            )

    def test_cogd_path_cached(self) -> None:
        """Test that cogd_path is cached."""
        workspace.Workspace.cogd_path.cache_clear()
        with patch("subprocess.check_output") as mock_check_output:
            mock_check_output.return_value = "/path/to/cogd\n"
            path1 = workspace.Workspace.cogd_path()
            path2 = workspace.Workspace.cogd_path()
            self.assertEqual(path1, path2)
            mock_check_output.assert_called_once()

    def test_init_cogd_path_failure(self) -> None:
        """Test that Workspace instantiation fails when cogd_path fails."""
        workspace.Workspace.cogd_path.cache_clear()
        with patch("subprocess.check_output") as mock_check_output:
            mock_check_output.side_effect = subprocess.CalledProcessError(
                1, "cmd"
            )
            with self.assertRaises(workspace.NotInCogWorkspaceError):
                workspace.Workspace(repo_dir=None)

    def test_init_cogd_path_not_found(self) -> None:
        """Test that cogd_path raises RepoSetupError when git-citc is missing."""
        workspace.Workspace.cogd_path.cache_clear()
        with patch("subprocess.check_output", side_effect=FileNotFoundError):
            with self.assertRaises(workspace.NotInCogWorkspaceError):
                workspace.Workspace.cogd_path()

    def test_init_cartfs_find_mount_point_failure(self) -> None:
        """Test that Workspace instantiation fails when Cartfs.find_mount_point fails."""
        with patch.object(
            cartfs.Cartfs,
            "find_mount_point",
            side_effect=cartfs.CartfsNotRunningError("mock error"),
        ):
            with self.assertRaises(cartfs.CartfsNotRunningError):
                workspace.Workspace(repo_dir=self.fs.repo_dir)

    def test_init_cartfs_workspace_snapshot_success(
        self,
    ) -> None:
        """Test that snapshotting from a previous instance is successful."""
        cartfs_instance = MagicMock()
        cartfs_instance.mount_point = self.fs.cartfs_dir
        suggested_directory_name = "new_cartfs_dir"
        cartfs_instance.suggest_cartfs_dir_name.return_value = (
            suggested_directory_name
        )

        def mock_snapshot_workspace(
            _workspace_to_snapshot_from: Path,
            _workspace_to_snapshot_to: Path,
            cartfs_mount_point: Path,
        ) -> None:
            (cartfs_mount_point / suggested_directory_name).mkdir()

        ws = workspace.Workspace(self.fs.repo_dir)
        ws.cartfs_instance = cartfs_instance
        with patch.object(
            ws,
            "_find_previous_instance",
            return_value=Path("foo"),
        ), patch.object(
            workspace.Workspace, "lock_file", new_callable=PropertyMock
        ) as mock_lock_file, patch(
            "workspace.snapshotter.snapshot_workspace",
            side_effect=mock_snapshot_workspace,
        ):
            mock_lock_file.return_value = self.fs.cog_dir / "test.lock"
            with ws.lock():
                ws.init_cartfs_workspace(snapshot=True)

            self.assertTrue(
                self.fs.full_path(
                    suggested_directory_name, mock_fs.FSType.CARTFS
                ).is_dir()
            )
            symlink_path = self.fs.repo_dir / workspace.CARTFS_SYMLINK_NAME
            self.assertTrue(symlink_path.is_symlink())

    def test_init_cartfs_workspace_snapshot_no_previous_instance(
        self,
    ) -> None:
        """Test that empty workspace is created when no previous instance is found."""
        cartfs_instance = MagicMock()
        cartfs_instance.mount_point = self.fs.cartfs_dir
        suggested_directory_name = "new_cartfs_dir"
        cartfs_instance.suggest_cartfs_dir_name.return_value = (
            suggested_directory_name
        )
        ws = workspace.Workspace(self.fs.repo_dir)
        ws.cartfs_instance = cartfs_instance
        with patch.object(
            ws, "_find_previous_instance", return_value=None
        ), patch.object(
            workspace.Workspace, "lock_file", new_callable=PropertyMock
        ) as mock_lock_file:
            mock_lock_file.return_value = self.fs.cog_dir / "test.lock"
            with ws.lock():
                ws.init_cartfs_workspace(snapshot=True)

            symlink_path = self.fs.repo_dir / workspace.CARTFS_SYMLINK_NAME
            self.assertTrue(symlink_path.is_symlink())

    def test_init_cartfs_workspace_snapshot_snapshot_error(self) -> None:
        """Test that empty workspace is created when snapshotting raises an error."""
        cartfs_instance = MagicMock()
        cartfs_instance.mount_point = self.fs.cartfs_dir
        suggested_directory_name = "new_cartfs_dir"
        cartfs_instance.suggest_cartfs_dir_name.return_value = (
            suggested_directory_name
        )
        ws = workspace.Workspace(self.fs.repo_dir)
        ws.cartfs_instance = cartfs_instance
        with patch.object(
            ws,
            "_find_previous_instance",
            return_value=self.fs.cartfs_dir / "previous_instance",
        ), patch.object(
            workspace.Workspace, "lock_file", new_callable=PropertyMock
        ) as mock_lock_file:
            mock_lock_file.return_value = self.fs.cog_dir / "test.lock"

            def mock_snapshot_workspace(
                _workspace_to_snapshot_from: Path,
                _workspace_to_snapshot_to: Path,
                _cartfs_mount_point: Path,
            ) -> None:
                raise ValueError("test error")

            with ws.lock():
                with patch(
                    "workspace.snapshotter.snapshot_workspace",
                    side_effect=mock_snapshot_workspace,
                ):
                    ws.init_cartfs_workspace(snapshot=True)

            symlink_path = self.fs.repo_dir / workspace.CARTFS_SYMLINK_NAME
            self.assertTrue(symlink_path.is_symlink())

    def test_init_cartfs_workspace_empty(self) -> None:
        """Test that an empty cartfs workspace directory is created and linked."""
        cartfs_instance = MagicMock()
        cartfs_instance.mount_point = self.fs.cartfs_dir
        suggested_directory_name = "new_cartfs_dir"
        cartfs_instance.suggest_cartfs_dir_name.return_value = (
            suggested_directory_name
        )
        ws = workspace.Workspace(self.fs.repo_dir)
        ws.cartfs_instance = cartfs_instance

        with patch.object(
            workspace.Workspace, "lock_file", new_callable=PropertyMock
        ) as mock_lock_file:
            mock_lock_file.return_value = self.fs.cog_dir / "test.lock"
            with ws.lock():
                ws.init_cartfs_workspace(snapshot=False)

        expected_dir = (
            Path(cartfs_instance.mount_point) / suggested_directory_name
        )
        self.assertTrue(expected_dir.is_dir())
        symlink_path = self.fs.repo_dir / workspace.CARTFS_SYMLINK_NAME
        self.assertTrue(symlink_path.is_symlink())

    def test_link_to_cartfs(self) -> None:
        """Test that the workspace can be linked to a cartfs directory."""
        cartfs_instance = MagicMock()
        ws = workspace.Workspace(self.fs.repo_dir)
        ws.cartfs_instance = cartfs_instance

        cartfs_dir = self.fs.mkdir("cartfs_dir", mock_fs.FSType.CARTFS)
        ws._link_to_cartfs(cartfs_dir)

        symlink_path = self.fs.repo_dir / workspace.CARTFS_SYMLINK_NAME
        self.assertTrue(symlink_path.is_symlink())

        # Ensure that we write the name of the repository in cartfs
        metadata = workspace.CogMetadata.from_file(
            cartfs_dir / workspace.COG_METADATA_FILE_NAME
        )
        self.assertIsNotNone(metadata)

        self.assertEqual(
            metadata and metadata.repo_name or "",
            "fuchsia",
        )
        self.assertEqual(
            metadata and metadata.workspace_name or "",
            "test-workspace",
        )
        self.assertEqual(
            metadata and metadata.workspace_id or "",
            self.fs.workspace_id,
        )

    def test_find_previous_instance_success(self) -> None:
        """Test that the previous instance is found correctly."""
        cartfs_instance = MagicMock()
        cartfs_instance.mount_point = self.fs.cartfs_dir

        # Create a candidate directory.
        candidate_dir = self.fs.mkdir("candidate", mock_fs.FSType.CARTFS)
        workspace.CogMetadata(
            workspace_name="test-workspace",
            repo_name="fuchsia",
        ).write(candidate_dir)

        ws = workspace.Workspace(self.fs.repo_dir)
        ws.cartfs_instance = cartfs_instance

        result = ws._find_previous_instance()
        self.assertEqual(result, Path("candidate"))

    def test_find_previous_instance_ignores_current_workspace(self) -> None:
        """Test that the current workspace is ignored."""
        cartfs_instance = MagicMock()
        cartfs_instance.mount_point = self.fs.cartfs_dir

        ws = workspace.Workspace(self.fs.repo_dir)
        ws.cartfs_instance = cartfs_instance

        result = ws._find_previous_instance()
        self.assertIsNone(result)

    def test_find_previous_instance_ignores_other_repo(self) -> None:
        """Test that the previous instance is skipped if it is for a different repository."""
        cartfs_instance = MagicMock()
        cartfs_instance.mount_point = self.fs.cartfs_dir

        # Create a candidate directory.
        candidate_dir = self.fs.mkdir("candidate", mock_fs.FSType.CARTFS)
        workspace.CogMetadata(
            workspace_name="test-workspace",
            repo_name="other-repo",
        ).write(candidate_dir)

        ws = workspace.Workspace(self.fs.repo_dir)
        ws.cartfs_instance = cartfs_instance

        result = ws._find_previous_instance()
        self.assertIsNone(result)

    def test_find_previous_instance_no_candidates(self) -> None:
        """Test that None is returned when there are no candidate directories."""
        cartfs_instance = MagicMock()
        cartfs_instance.mount_point = self.fs.cartfs_dir

        ws = workspace.Workspace(self.fs.repo_dir)
        ws.cartfs_instance = cartfs_instance

        result = ws._find_previous_instance()
        self.assertIsNone(result)

    def test_golden_snapshot_dir_fuchsia_repo(self) -> None:
        """Test that _golden_snapshot_dir returns None for fuchsia repo."""
        ws = workspace.Workspace(self.fs.repo_dir)
        self.assertIsNone(ws._golden_snapshot_dir)

    def test_golden_snapshot_dir_internal_superproject_missing(self) -> None:
        """Test that _golden_snapshot_dir returns None if .fuchsia_golden_snapshot missing."""
        ws = workspace.Workspace(
            self.fs.repo_dir.parent / "fuchsia-cog-superproject"
        )
        self.assertIsNone(ws._golden_snapshot_dir)

    def test_golden_snapshot_dir_outside_mount_point(self) -> None:
        """Test that _golden_snapshot_dir returns None if resolved path is outside cartfs mount point."""
        ws = workspace.Workspace(
            self.fs.repo_dir.parent / "fuchsia-cog-superproject"
        )
        outside_dir = self.fs.cog_dir / "outside_golden"
        outside_dir.mkdir(parents=True, exist_ok=True)
        (outside_dir / ".integration_commit_hash").touch()

        golden_symlink = self.fs.cartfs_dir / ".fuchsia_golden_snapshot"
        golden_symlink.symlink_to(outside_dir)

        self.assertIsNone(ws._golden_snapshot_dir)

    def test_golden_snapshot_dir_missing_commit_hash(self) -> None:
        """Test that _golden_snapshot_dir returns None if .integration_commit_hash is missing."""
        ws = workspace.Workspace(
            self.fs.repo_dir.parent / "fuchsia-cog-superproject"
        )
        inside_dir = self.fs.cartfs_dir / "golden_snapshot_v1"
        inside_dir.mkdir(parents=True, exist_ok=True)

        golden_symlink = self.fs.cartfs_dir / ".fuchsia_golden_snapshot"
        golden_symlink.symlink_to(inside_dir)

        self.assertIsNone(ws._golden_snapshot_dir)

    def test_golden_snapshot_dir_valid(self) -> None:
        """Test that _golden_snapshot_dir returns relative path for valid golden snapshot."""
        ws = workspace.Workspace(
            self.fs.repo_dir.parent / "fuchsia-cog-superproject"
        )
        inside_dir = self.fs.cartfs_dir / "golden_snapshot_v1"
        inside_dir.mkdir(parents=True, exist_ok=True)
        (inside_dir / ".integration_commit_hash").touch()

        golden_symlink = self.fs.cartfs_dir / ".fuchsia_golden_snapshot"
        golden_symlink.symlink_to(inside_dir)

        self.assertEqual(ws._golden_snapshot_dir, Path("golden_snapshot_v1"))

    def test_is_cartfs_workspace_clean_initially_false(self) -> None:
        """Test that _is_cartfs_workspace_clean is initially False."""
        ws = workspace.Workspace(self.fs.repo_dir)
        self.assertFalse(ws._is_cartfs_workspace_clean)

    def test_is_cartfs_workspace_clean_after_empty_init(self) -> None:
        """Test that _is_cartfs_workspace_clean is True after initializing an empty workspace."""
        cartfs_instance = MagicMock()
        cartfs_instance.mount_point = self.fs.cartfs_dir
        suggested_directory_name = "new_cartfs_dir"
        cartfs_instance.suggest_cartfs_dir_name.return_value = (
            suggested_directory_name
        )

        ws = workspace.Workspace(self.fs.repo_dir)
        ws.cartfs_instance = cartfs_instance

        with patch.object(
            workspace.Workspace, "lock_file", new_callable=PropertyMock
        ) as mock_lock_file:
            mock_lock_file.return_value = self.fs.cog_dir / "test.lock"
            with ws.lock():
                ws.init_cartfs_workspace(snapshot=False)

        self.assertTrue(ws._is_cartfs_workspace_clean)

    def test_is_cartfs_workspace_clean_after_golden_snapshot_init(self) -> None:
        """Test that _is_cartfs_workspace_clean is True after initializing from golden snapshot."""
        cartfs_instance = MagicMock()
        cartfs_instance.mount_point = self.fs.cartfs_dir
        suggested_directory_name = "new_cartfs_dir"
        cartfs_instance.suggest_cartfs_dir_name.return_value = (
            suggested_directory_name
        )

        def mock_snapshot_workspace(
            _workspace_to_snapshot_from: Path,
            _workspace_to_snapshot_to: Path,
            cartfs_mount_point: Path,
        ) -> None:
            (cartfs_mount_point / suggested_directory_name).mkdir()

        ws_repo_dir = self.fs.repo_dir.parent / "fuchsia-cog-superproject"
        ws_repo_dir.mkdir(parents=True, exist_ok=True)
        ws = workspace.Workspace(ws_repo_dir)
        ws.cartfs_instance = cartfs_instance

        with (
            patch.object(
                workspace.Workspace,
                "_golden_snapshot_dir",
                new_callable=PropertyMock,
                return_value=Path("golden_snapshot_v1"),
            ),
            patch.object(
                workspace.Workspace, "lock_file", new_callable=PropertyMock
            ) as mock_lock_file,
            patch(
                "workspace.snapshotter.snapshot_workspace",
                side_effect=mock_snapshot_workspace,
            ),
            patch.object(ws, "_sync_cog_to_golden_revision") as mock_sync,
        ):
            mock_lock_file.return_value = self.fs.cog_dir / "test.lock"
            with ws.lock():
                ws.init_cartfs_workspace(snapshot=True)

            self.assertTrue(ws._is_cartfs_workspace_clean)
            mock_sync.assert_called_once()

    def test_is_cartfs_workspace_clean_after_previous_snapshot_init(
        self,
    ) -> None:
        """Test that _is_cartfs_workspace_clean is False after initializing from a previous instance."""
        cartfs_instance = MagicMock()
        cartfs_instance.mount_point = self.fs.cartfs_dir
        suggested_directory_name = "new_cartfs_dir"
        cartfs_instance.suggest_cartfs_dir_name.return_value = (
            suggested_directory_name
        )

        def mock_snapshot_workspace(
            _workspace_to_snapshot_from: Path,
            _workspace_to_snapshot_to: Path,
            cartfs_mount_point: Path,
        ) -> None:
            (cartfs_mount_point / suggested_directory_name).mkdir()

        ws = workspace.Workspace(self.fs.repo_dir)
        ws.cartfs_instance = cartfs_instance

        with (
            patch.object(
                workspace.Workspace,
                "_golden_snapshot_dir",
                new_callable=PropertyMock,
                return_value=None,
            ),
            patch.object(
                ws,
                "_find_previous_instance",
                return_value=Path("previous_instance"),
            ),
            patch.object(
                workspace.Workspace, "lock_file", new_callable=PropertyMock
            ) as mock_lock_file,
            patch(
                "workspace.snapshotter.snapshot_workspace",
                side_effect=mock_snapshot_workspace,
            ),
            patch.object(ws, "_sync_cog_to_golden_revision") as mock_sync,
        ):
            mock_lock_file.return_value = self.fs.cog_dir / "test.lock"
            with ws.lock():
                ws.init_cartfs_workspace(snapshot=True)

            self.assertFalse(ws._is_cartfs_workspace_clean)
            mock_sync.assert_not_called()

    def test_write_jiri_config_when_clean(self) -> None:
        """Test that _write_jiri_config does not clean/restore git when _is_cartfs_workspace_clean is True."""
        ws = workspace.Workspace(self.fs.repo_dir)
        ws._is_cartfs_workspace_clean = True
        ws.__dict__["cartfs_dir"] = self.fs.cartfs_dir

        with (
            patch.object(ws, "_create_symlink") as mock_symlink,
            patch.object(ws, "_run") as mock_run,
        ):
            ws._write_jiri_config()

            # Ensure we ran jiri init but not clean/restore
            mock_run.assert_called_once_with(
                [".jiri_root/bin/jiri", "init", "-analytics-opt=true"],
                cwd=ws.cartfs_fuchsia_dir,
            )

    def test_write_jiri_config_when_dirty(self) -> None:
        """Test that _write_jiri_config cleans and restores git when _is_cartfs_workspace_clean is False."""
        ws = workspace.Workspace(self.fs.repo_dir)
        ws._is_cartfs_workspace_clean = False
        ws.__dict__["cartfs_dir"] = self.fs.cartfs_dir

        with (
            patch.object(ws, "_create_symlink") as mock_symlink,
            patch.object(ws, "_run") as mock_run,
        ):
            ws._write_jiri_config()

            self.assertEqual(mock_run.call_count, 3)
            mock_run.assert_any_call(
                [".jiri_root/bin/jiri", "init", "-analytics-opt=true"],
                cwd=ws.cartfs_fuchsia_dir,
            )
            mock_run.assert_any_call(
                [".jiri_root/bin/jiri", "runp", "git", "clean", "-df"],
                cwd=ws.cartfs_fuchsia_dir,
            )
            mock_run.assert_any_call(
                [".jiri_root/bin/jiri", "runp", "git", "restore", "."],
                cwd=ws.cartfs_fuchsia_dir,
            )

    def test_checkout_cartfs_to_cog_revisions_up_to_date_no_cog_integration(
        self,
    ) -> None:
        """Test that initialization skips sync and symlinks when up-to-date."""
        with mock_fs.FileSystemTestHelper() as fs:
            ws = workspace.Workspace(
                fs.full_path("test-workspace", mock_fs.FSType.COG) / "fuchsia",
            )
            ws.__dict__["cartfs_dir"] = fs.cartfs_dir
            ws.__dict__["config"] = {
                "integration_url": "https://fuchsia.googlesource.com/integration",
                "repo": {
                    "fuchsia": "fuchsia",
                    "integration": None,
                },
                "jiriImports": [],
            }

            with (
                patch.object(ws, "get_cog_commit", return_value="hash123"),
                patch.object(ws, "get_cartfs_commit", return_value="hash123"),
                patch.object(ws, "_sync_fuchsia_repo") as mock_sync,
                patch.object(ws, "_create_symlinks") as mock_symlinks,
                patch.object(ws, "_fetch_prebuilts") as mock_fetch,
                patch.object(ws, "_reinit_integration_repo") as mock_reinit,
                patch.object(
                    ws, "_checkout_integration_roll", return_value="int_hash"
                ) as mock_checkout,
                patch.object(ws, "_is_jiri_bootstrapped", return_value=True),
                patch("subprocess.run"),
            ):
                with patch.object(ws, "_assert_locked") as mock_assert_locked:
                    ws.checkout_cartfs_to_cog_revisions()
                    mock_assert_locked.assert_called_once()

                mock_sync.assert_called_once_with("hash123")
                mock_symlinks.assert_called_once()

    def test_checkout_cartfs_to_cog_revisions_up_to_date_with_cog_integration(
        self,
    ) -> None:
        """Test that initialization skips sync and symlinks when up-to-date (including integration)."""
        with mock_fs.FileSystemTestHelper() as fs:
            ws = workspace.Workspace(
                fs.full_path("test-workspace", mock_fs.FSType.COG) / "fuchsia",
            )
            ws.__dict__["cartfs_dir"] = fs.cartfs_dir
            ws.__dict__["config"] = {
                "integration_url": "https://fuchsia.googlesource.com/integration",
                "repo": {
                    "fuchsia": "fuchsia",
                    "integration": "integration",
                },
                "jiriImports": [],
            }

            def mock_get_cog_commit(repo: str) -> str:
                return "fuchsia_hash" if repo == "fuchsia" else "int_hash"

            def mock_get_cartfs_commit(repo: str) -> str:
                return "fuchsia_hash" if repo == "fuchsia" else "int_hash"

            with (
                patch.object(
                    ws, "get_cog_commit", side_effect=mock_get_cog_commit
                ),
                patch.object(
                    ws, "get_cartfs_commit", side_effect=mock_get_cartfs_commit
                ),
                patch.object(ws, "_sync_fuchsia_repo") as mock_sync,
                patch.object(ws, "_create_symlinks") as mock_symlinks,
                patch.object(ws, "_fetch_prebuilts") as mock_fetch,
                patch.object(ws, "_reinit_integration_repo") as mock_reinit,
                patch.object(ws, "_is_jiri_bootstrapped", return_value=True),
                patch("subprocess.run"),
            ):
                with patch.object(ws, "_assert_locked") as mock_assert_locked:
                    ws.checkout_cartfs_to_cog_revisions()
                    mock_assert_locked.assert_called_once()

                mock_sync.assert_called_once_with("fuchsia_hash")
                mock_symlinks.assert_called_once()

    def test_checkout_cartfs_to_cog_revisions_not_up_to_date_no_cog_integration(
        self,
    ) -> None:
        """Test that initialization performs sync when not up-to-date (no integration repo)."""
        with mock_fs.FileSystemTestHelper() as fs:
            ws = workspace.Workspace(
                fs.full_path("test-workspace", mock_fs.FSType.COG) / "fuchsia",
            )
            ws.__dict__["cartfs_dir"] = fs.cartfs_dir
            ws.__dict__["config"] = {
                "integration_url": "https://fuchsia.googlesource.com/integration",
                "repo": {
                    "fuchsia": "fuchsia",
                    "integration": None,
                },
                "jiriImports": [],
                "symlinks": {},
            }

            with (
                patch.object(ws, "get_cog_commit", return_value="hash123"),
                patch.object(
                    ws, "get_cartfs_commit", return_value="different-hash"
                ),
                patch.object(ws, "_sync_fuchsia_repo") as mock_sync,
                patch.object(ws, "_create_symlinks") as mock_symlinks,
                patch.object(ws, "_fetch_prebuilts") as mock_fetch,
                patch.object(ws, "_reinit_integration_repo") as mock_reinit,
                patch.object(
                    ws,
                    "_checkout_integration_roll",
                    return_value="integration_hash_abc",
                ) as mock_checkout,
                patch.object(ws, "_is_jiri_bootstrapped", return_value=True),
                patch("subprocess.run"),
            ):
                with patch.object(ws, "_assert_locked") as mock_assert_locked:
                    ws.checkout_cartfs_to_cog_revisions()
                    mock_assert_locked.assert_called_once()

                mock_reinit.assert_called_once()
                mock_checkout.assert_called_once_with("hash123")
                mock_sync.assert_called_once_with("hash123")
                mock_symlinks.assert_called_once()
                mock_fetch.assert_called_once()

    def test_checkout_cartfs_to_cog_revisions_not_up_to_date_with_cog_integration(
        self,
    ) -> None:
        """Test that initialization performs sync when not up-to-date (with integration repo)."""
        with mock_fs.FileSystemTestHelper() as fs:
            ws = workspace.Workspace(
                fs.full_path("test-workspace", mock_fs.FSType.COG) / "fuchsia",
            )
            ws.__dict__["cartfs_dir"] = fs.cartfs_dir
            ws.__dict__["config"] = {
                "integration_url": "https://fuchsia.googlesource.com/integration",
                "repo": {
                    "fuchsia": "fuchsia",
                    "integration": "integration",
                },
                "jiriImports": [],
                "symlinks": {},
            }

            def mock_get_cog_commit(repo: str) -> str:
                return "fuchsia_hash" if repo == "fuchsia" else "int_hash"

            def mock_get_cartfs_commit(repo: str) -> str:
                return (
                    "diff_fuchsia_hash"
                    if repo == "fuchsia"
                    else "diff_int_hash"
                )

            with (
                patch.object(
                    ws, "get_cog_commit", side_effect=mock_get_cog_commit
                ),
                patch.object(
                    ws, "get_cartfs_commit", side_effect=mock_get_cartfs_commit
                ),
                patch.object(ws, "_sync_fuchsia_repo") as mock_sync,
                patch.object(ws, "_create_symlinks") as mock_symlinks,
                patch.object(ws, "_fetch_prebuilts") as mock_fetch,
                patch.object(ws, "_reinit_integration_repo") as mock_reinit,
                patch.object(ws, "_checkout_integration_roll") as mock_checkout,
                patch.object(ws, "_is_jiri_bootstrapped", return_value=True),
                patch("subprocess.run"),
            ):
                with patch.object(ws, "_assert_locked") as mock_assert_locked:
                    ws.checkout_cartfs_to_cog_revisions()
                    mock_assert_locked.assert_called_once()

                mock_reinit.assert_called_once_with("int_hash")
                mock_checkout.assert_not_called()
                mock_sync.assert_called_once_with("fuchsia_hash")
                mock_symlinks.assert_called_once()
                mock_fetch.assert_called_once()

    def test_create_symlinks_resolves_paths(self) -> None:
        """Test that _create_symlinks correctly resolves @cog// and @cartfs// paths."""
        with mock_fs.FileSystemTestHelper() as fs:
            ws = workspace.Workspace(
                fs.full_path("test-workspace", mock_fs.FSType.COG) / "fuchsia",
            )
            ws.__dict__["cartfs_dir"] = fs.cartfs_dir

            assert ws.cartfs_dir is not None

            mock_config_patcher = patch.object(
                workspace.Workspace, "config", new_callable=PropertyMock
            )
            mock_config = mock_config_patcher.start()
            mock_config.return_value = {
                "integration_url": "https://fuchsia.googlesource.com/integration",
                "repo": {
                    "fuchsia": "fuchsia",
                    "integration": None,
                },
                "symlinkMap": {
                    "@cog//dest_path": "@cartfs//src_path",
                },
            }
            self.addCleanup(mock_config_patcher.stop)

            with (
                patch.object(ws, "_create_symlink") as mock_create_symlink,
                patch.object(ws, "_run") as mock_run,
                patch("subprocess.Popen") as mock_popen,
            ):
                # Mock directory creation to avoid side effects
                with patch.object(Path, "mkdir"):
                    ws._create_symlinks()

                mock_create_symlink.assert_called_once_with(
                    fs.cartfs_dir / "src_path",
                    fs.full_path("test-workspace", mock_fs.FSType.COG)
                    / "fuchsia"
                    / "dest_path",
                )

            # Test invalid root raises KeyError
            mock_config.return_value = {
                "integration_url": "https://fuchsia.googlesource.com/integration",
                "repo": {
                    "fuchsia": "fuchsia",
                    "integration": None,
                },
                "symlinkMap": {
                    "@cog//dest_path": "@invalid//src_path",
                },
            }
            with self.assertRaises(KeyError):
                ws._create_symlinks()

    def test_write_jiri_manifest_uses_imports(self) -> None:
        """Test that _write_jiri_manifest uses jiriImports from config."""
        with mock_fs.FileSystemTestHelper() as fs:
            ws = workspace.Workspace(
                fs.full_path("test-workspace", mock_fs.FSType.COG) / "fuchsia",
            )
            ws.__dict__["cartfs_dir"] = fs.cartfs_dir

            mock_config_patcher = patch.object(
                workspace.Workspace, "config", new_callable=PropertyMock
            )
            mock_config = mock_config_patcher.start()
            mock_config.return_value = {
                "integration_url": "https://fuchsia.googlesource.com/integration",
                "repo": {
                    "fuchsia": "fuchsia",
                    "integration": None,
                },
                "jiriImports": [
                    "manifest/path1",
                    "manifest/path2",
                ],
            }
            self.addCleanup(mock_config_patcher.stop)

            ws._write_jiri_manifest()

            manifest_path = fs.cartfs_dir / "fuchsia" / ".jiri_manifest"
            self.assertTrue(manifest_path.exists())

            content = manifest_path.read_text()
            self.assertIn('<localimport file="manifest/path1"/>', content)
            self.assertIn('<localimport file="manifest/path2"/>', content)

    def test_checkout_cartfs_to_cog_revisions_with_custom_integration(
        self,
    ) -> None:
        """Test that specifying integration.repo and integration.remote changes behavior."""
        with mock_fs.FileSystemTestHelper() as fs:
            ws = workspace.Workspace(
                fs.full_path("test-workspace", mock_fs.FSType.COG) / "fuchsia",
            )
            ws.__dict__["cartfs_dir"] = fs.cartfs_dir

            mock_config_patcher = patch.object(
                workspace.Workspace, "config", new_callable=PropertyMock
            )
            mock_config = mock_config_patcher.start()
            mock_config.return_value = {
                "integration_url": "https://custom.git/integration",
                "repo": {
                    "fuchsia": "custom-fuchsia",
                    "integration": "custom-integration",
                },
                "jiriImports": [],
            }
            self.addCleanup(mock_config_patcher.stop)

            with (
                patch.object(ws, "get_cog_commit") as mock_get_cog_commit,
                patch.object(ws, "get_cartfs_commit", return_value="hash123"),
                patch.object(ws, "_sync_fuchsia_repo") as mock_sync,
                patch.object(ws, "_create_symlinks") as mock_symlinks,
                patch.object(ws, "_fetch_prebuilts") as mock_fetch,
                patch.object(
                    ws, "_checkout_integration_roll"
                ) as mock_checkout_roll,
                patch.object(ws, "_is_jiri_bootstrapped", return_value=True),
                patch.object(ws, "_write_jiri_config"),
                patch("subprocess.run") as mock_subprocess_run,
            ):
                mock_get_cog_commit.side_effect = lambda repo: {
                    "custom-fuchsia": "fuchsia_hash",
                    "custom-integration": "integration_hash",
                }[repo]

                with patch.object(ws, "_assert_locked") as mock_assert_locked:
                    ws.checkout_cartfs_to_cog_revisions()
                    mock_assert_locked.assert_called_once()

                # Should bypass checkout_integration_roll
                mock_checkout_roll.assert_not_called()

                # Should call sync_fuchsia_repo
                mock_sync.assert_called_once_with("fuchsia_hash")

                # Should call subprocess.run (for git clone) in _reinit_integration_repo
                mock_subprocess_run.assert_called_once()
                args, kwargs = mock_subprocess_run.call_args
                cmd = args[0]
                self.assertIn("https://custom.git/integration", cmd)
                self.assertIn("--revision=integration_hash", cmd)

                # Should call _fetch_prebuilts
                mock_fetch.assert_called_once()

    def test_is_checkout_uptodate_standalone_true(self) -> None:
        """Test that returns True when standalone checkout is up to date."""
        with mock_fs.FileSystemTestHelper() as fs:
            ws = workspace.Workspace(
                fs.full_path("test-workspace", mock_fs.FSType.COG) / "fuchsia",
            )
            ws.__dict__["cartfs_dir"] = fs.cartfs_dir
            ws.__dict__["config"] = {
                "repo": {
                    "fuchsia": "fuchsia",
                    "integration": None,
                },
            }
            with (
                patch.object(ws, "get_cog_commit", return_value="hash123"),
                patch.object(ws, "get_cartfs_commit", return_value="hash123"),
            ):
                with patch.object(ws, "_assert_locked") as mock_assert_locked:
                    self.assertTrue(ws.is_checkout_uptodate())
                    mock_assert_locked.assert_called_once()

    def test_is_checkout_uptodate_standalone_false(self) -> None:
        """Test that returns False when standalone checkout is not up to date."""
        with mock_fs.FileSystemTestHelper() as fs:
            ws = workspace.Workspace(
                fs.full_path("test-workspace", mock_fs.FSType.COG) / "fuchsia",
            )
            ws.__dict__["cartfs_dir"] = fs.cartfs_dir
            ws.__dict__["config"] = {
                "repo": {
                    "fuchsia": "fuchsia",
                    "integration": None,
                },
            }
            with (
                patch.object(ws, "get_cog_commit", return_value="hash123"),
                patch.object(
                    ws, "get_cartfs_commit", return_value="different-hash"
                ),
            ):
                with patch.object(ws, "_assert_locked") as mock_assert_locked:
                    self.assertFalse(ws.is_checkout_uptodate())
                    mock_assert_locked.assert_called_once()

    def test_is_checkout_uptodate_superproject_true(self) -> None:
        """Test that returns True when superproject checkout is up to date."""
        with mock_fs.FileSystemTestHelper() as fs:
            ws = workspace.Workspace(
                fs.full_path("test-workspace", mock_fs.FSType.COG) / "fuchsia",
            )
            ws.__dict__["cartfs_dir"] = fs.cartfs_dir
            ws.__dict__["config"] = {
                "repo": {
                    "fuchsia": "fuchsia",
                    "integration": "integration",
                },
                "useJiriUpdateOnly": True,
            }

            def mock_get_commit(repo: str) -> str:
                return "fuchsia_hash" if repo == "fuchsia" else "int_hash"

            with (
                patch.object(ws, "get_cog_commit", side_effect=mock_get_commit),
                patch.object(
                    ws, "get_cartfs_commit", side_effect=mock_get_commit
                ),
            ):
                with patch.object(ws, "_assert_locked") as mock_assert_locked:
                    self.assertTrue(ws.is_checkout_uptodate())
                    mock_assert_locked.assert_called_once()

    def test_is_checkout_uptodate_superproject_false_fuchsia_differs(
        self,
    ) -> None:
        """Test that returns False when fuchsia commit differs in superproject."""
        with mock_fs.FileSystemTestHelper() as fs:
            ws = workspace.Workspace(
                fs.full_path("test-workspace", mock_fs.FSType.COG) / "fuchsia",
            )
            ws.__dict__["cartfs_dir"] = fs.cartfs_dir
            ws.__dict__["config"] = {
                "repo": {
                    "fuchsia": "fuchsia",
                    "integration": "integration",
                },
                "useJiriUpdateOnly": True,
            }
            with (
                patch.object(ws, "get_cog_commit", return_value="fuchsia_hash"),
                patch.object(ws, "get_cartfs_commit", return_value="diff_hash"),
            ):
                with patch.object(ws, "_assert_locked") as mock_assert_locked:
                    self.assertFalse(ws.is_checkout_uptodate())
                    mock_assert_locked.assert_called_once()

    def test_is_checkout_uptodate_superproject_false_integration_differs(
        self,
    ) -> None:
        """Test that returns False when integration commit differs in superproject."""
        with mock_fs.FileSystemTestHelper() as fs:
            ws = workspace.Workspace(
                fs.full_path("test-workspace", mock_fs.FSType.COG) / "fuchsia",
            )
            ws.__dict__["cartfs_dir"] = fs.cartfs_dir
            ws.__dict__["config"] = {
                "repo": {
                    "fuchsia": "fuchsia",
                    "integration": "integration",
                },
                "useJiriUpdateOnly": True,
            }

            def mock_get_cog_commit(repo: str) -> str:
                return "fuchsia_hash" if repo == "fuchsia" else "int_hash"

            def mock_get_cartfs_commit(repo: str) -> str:
                return "fuchsia_hash" if repo == "fuchsia" else "diff_int_hash"

            with (
                patch.object(
                    ws, "get_cog_commit", side_effect=mock_get_cog_commit
                ),
                patch.object(
                    ws, "get_cartfs_commit", side_effect=mock_get_cartfs_commit
                ),
            ):
                with patch.object(ws, "_assert_locked") as mock_assert_locked:
                    self.assertFalse(ws.is_checkout_uptodate())
                    mock_assert_locked.assert_called_once()

    def test_lock_acquisition(self) -> None:
        """Test that lock can be acquired."""
        with mock_fs.FileSystemTestHelper() as fs:
            ws = workspace.Workspace(fs.repo_dir)
            with patch.object(
                workspace.Workspace, "lock_file", new_callable=PropertyMock
            ) as mock_lock_file:
                mock_lock_file.return_value = fs.cog_dir / "test.lock"

                self.assertEqual(ws._lock_count, 0)
                with ws.lock():
                    self.assertEqual(ws._lock_count, 1)
                    self.assertTrue((fs.cog_dir / "test.lock").exists())
                    lock_content = (fs.cog_dir / "test.lock").read_text()
                    self.assertEqual(lock_content, str(os.getpid()))
                self.assertEqual(ws._lock_count, 0)

    def test_lock_reentrant(self) -> None:
        """Test that lock is reentrant."""
        with mock_fs.FileSystemTestHelper() as fs:
            ws = workspace.Workspace(fs.repo_dir)
            with patch.object(
                workspace.Workspace, "lock_file", new_callable=PropertyMock
            ) as mock_lock_file:
                mock_lock_file.return_value = fs.cog_dir / "test.lock"

                with ws.lock():
                    self.assertEqual(ws._lock_count, 1)
                    with ws.lock():
                        self.assertEqual(ws._lock_count, 2)
                    self.assertEqual(ws._lock_count, 1)
                self.assertEqual(ws._lock_count, 0)

    def test_lock_blocks_and_resumes(self) -> None:
        """Test that a second workspace instance blocks and resumes."""
        with mock_fs.FileSystemTestHelper() as fs:
            ws1 = workspace.Workspace(fs.repo_dir)
            ws2 = workspace.Workspace(fs.repo_dir)

            # Mock Path.home() to return fs.cog_dir
            with patch.object(Path, "home", return_value=fs.cog_dir):
                lock_acquired_by_ws1 = threading.Event()
                ws2_can_proceed = threading.Event()
                ws2_finished = threading.Event()

                def ws2_worker() -> None:
                    # Wait for ws1 to hold the lock
                    lock_acquired_by_ws1.wait()
                    # This should block until ws1 releases the lock
                    with ws2.lock():
                        ws2_can_proceed.set()
                        ws2_finished.set()

                t = threading.Thread(target=ws2_worker)
                t.start()

                with ws1.lock():
                    lock_acquired_by_ws1.set()
                    # ws2 should be blocked now.
                    # We assert that ws2 has NOT proceeded yet.
                    self.assertFalse(ws2_can_proceed.is_set())
                    # Give worker thread a moment to try and block
                    time.sleep(0.5)
                    self.assertFalse(ws2_can_proceed.is_set())

                # After ws1 exits lock, ws2 should proceed.
                ws2_finished.wait(timeout=5)
                self.assertTrue(ws2_can_proceed.is_set())
                t.join()

    def test_assert_locked_raises(self) -> None:
        """Test that _assert_locked raises error when not locked."""
        with mock_fs.FileSystemTestHelper() as fs:
            ws = workspace.Workspace(fs.repo_dir)
            with self.assertRaises(workspace.WorkspaceError):
                ws._assert_locked()

    def test_assert_locked_passes(self) -> None:
        """Test that _assert_locked passes when locked."""
        with mock_fs.FileSystemTestHelper() as fs:
            ws = workspace.Workspace(fs.repo_dir)
            ws._lock_count = 1
            ws._assert_locked()

    def test_lock_decorator(self) -> None:
        """Test that the lock decorator acquires the lock."""
        mock_ws = MagicMock()

        class SomeClass:
            def __init__(self) -> None:
                self.workspace: workspace.Workspace = mock_ws

            @workspace.lock
            def decorated_method(self, x: int) -> int:
                return x * 2

        some_object = SomeClass()
        mock_ws.lock.assert_not_called()

        result = some_object.decorated_method(21)
        self.assertEqual(result, 42)
        mock_ws.lock.assert_called_once()

    def test_sync_cog_to_golden_revision_success(self) -> None:
        """Test that _sync_cog_to_golden_revision successfully rebases the superproject."""
        with mock_fs.FileSystemTestHelper() as fs:
            ws = workspace.Workspace(fs.repo_dir)
            ws.cartfs_dir = fs.cartfs_dir

            # Setup fake superproject directory in CartFS
            superproject_dir = fs.cartfs_dir / "fuchsia-cog-superproject"
            superproject_dir.mkdir(parents=True, exist_ok=True)

            target_commit = "ca69b3f0bfe3b37612bbf2e95fbd29ec8b31012d"

            def mock_run(
                cmd: list[str], cwd: Path, capture_output: bool = False
            ) -> str:
                if (
                    cmd == ["git", "rev-parse", "HEAD"]
                    and cwd == superproject_dir
                ):
                    return target_commit
                return ""

            with (
                patch.object(ws, "_run", side_effect=mock_run) as mock_run_spy,
                patch.object(workspace.logger, "emit_status") as mock_emit,
                patch.object(workspace.logger, "log_info") as mock_log_info,
            ):
                ws._sync_cog_to_golden_revision()

                # Verify git rev-parse HEAD was called
                mock_run_spy.assert_any_call(
                    ["git", "rev-parse", "HEAD"],
                    cwd=superproject_dir,
                    capture_output=True,
                )

                # Verify git-citc api.call Rebase was called
                mock_run_spy.assert_any_call(
                    [
                        "git-citc",
                        "api.call",
                        "Rebase",
                        f'repo_root: "{ws.repo_name}" new_base: "{target_commit}"',
                    ],
                    cwd=ws.repo_dir,
                    capture_output=True,
                )

    def test_sync_cog_to_golden_revision_missing_dir(self) -> None:
        """Test that _sync_cog_to_golden_revision logs error if directory missing."""
        with mock_fs.FileSystemTestHelper() as fs:
            ws = workspace.Workspace(fs.repo_dir)
            ws.cartfs_dir = fs.cartfs_dir
            # Do NOT create the directory

            with (
                patch.object(ws, "_run") as mock_run,
                patch.object(workspace.logger, "log_error") as mock_log_error,
            ):
                ws._sync_cog_to_golden_revision()

                # Verify it logged the error
                mock_log_error.assert_called_once()
                self.assertIn(
                    "directory not found in CartFS",
                    mock_log_error.call_args[0][0],
                )

                # Verify no run calls were made
                mock_run.assert_not_called()

    def test_sync_cog_to_golden_revision_git_failure(self) -> None:
        """Test that _sync_cog_to_golden_revision handles git failure gracefully."""
        with mock_fs.FileSystemTestHelper() as fs:
            ws = workspace.Workspace(fs.repo_dir)
            ws.cartfs_dir = fs.cartfs_dir

            # Setup fake superproject directory in CartFS
            superproject_dir = fs.cartfs_dir / "fuchsia-cog-superproject"
            superproject_dir.mkdir(parents=True, exist_ok=True)

            def mock_run(
                cmd: list[str], cwd: Path, capture_output: bool = False
            ) -> str:
                if (
                    cmd == ["git", "rev-parse", "HEAD"]
                    and cwd == superproject_dir
                ):
                    raise subprocess.CalledProcessError(
                        128, cmd, stderr=b"fatal: not a git repository"
                    )
                return ""

            with (
                patch.object(ws, "_run", side_effect=mock_run),
                patch.object(workspace.logger, "log_error") as mock_log_error,
            ):
                ws._sync_cog_to_golden_revision()

                # Verify error is logged and includes the stderr of the process
                mock_log_error.assert_called_once()
                self.assertIn(
                    "Failed to get superproject commit from CartFS snapshot",
                    mock_log_error.call_args[0][0],
                )
                self.assertIn(
                    "fatal: not a git repository",
                    mock_log_error.call_args[0][0],
                )

    def test_sync_cog_to_golden_revision_rebase_failure(self) -> None:
        """Test that _sync_cog_to_golden_revision handles git-citc rebase failure gracefully."""
        with mock_fs.FileSystemTestHelper() as fs:
            ws = workspace.Workspace(fs.repo_dir)
            ws.cartfs_dir = fs.cartfs_dir

            # Setup fake superproject directory in CartFS
            superproject_dir = fs.cartfs_dir / "fuchsia-cog-superproject"
            superproject_dir.mkdir(parents=True, exist_ok=True)

            target_commit = "ca69b3f0bfe3b37612bbf2e95fbd29ec8b31012d"

            def mock_run(
                cmd: list[str], cwd: Path, capture_output: bool = False
            ) -> str:
                if (
                    cmd == ["git", "rev-parse", "HEAD"]
                    and cwd == superproject_dir
                ):
                    return target_commit
                if "git-citc" in cmd:
                    raise subprocess.CalledProcessError(
                        1, cmd, stderr=b"Rebase failed because of conflict"
                    )
                return ""

            with (
                patch.object(ws, "_run", side_effect=mock_run),
                patch.object(workspace.logger, "log_error") as mock_log_error,
            ):
                ws._sync_cog_to_golden_revision()

                # Verify error is logged and includes the stderr of the process
                mock_log_error.assert_called_once()
                self.assertIn(
                    "Failed to sync Cog CITC base",
                    mock_log_error.call_args[0][0],
                )
                self.assertIn(
                    "Rebase failed because of conflict",
                    mock_log_error.call_args[0][0],
                )


if __name__ == "__main__":
    unittest.main()
