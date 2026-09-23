# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import os
import sys
import tempfile
import unittest
from io import StringIO
from pathlib import Path
from unittest.mock import MagicMock, patch

worktree_dir = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, worktree_dir)

import argparse

from subcommands import add as add_cmd
from subcommands import pool_add as pool_add_cmd
from subcommands import pool_remove as pool_remove_cmd
from subcommands import remove as remove_cmd
from worktree import NoFreeWorktreesError, SyncStatus, Worktree, WorktreeState
from worktree_pool import ADJECTIVES, NOUNS, WorktreePool
from worktree_printer import WorktreePrinter


class TestWorktreePool(unittest.TestCase):
    def setUp(self) -> None:
        self.temp_dir = tempfile.TemporaryDirectory()
        self.fuchsia_dir = Path(self.temp_dir.name)
        self.jiri_root = self.fuchsia_dir / ".jiri_root"
        self.jiri_root.mkdir(parents=True, exist_ok=True)
        self.pool = WorktreePool(fuchsia_dir=str(self.fuchsia_dir))

    def tearDown(self) -> None:
        self.temp_dir.cleanup()

    def test_empty(self) -> None:
        self.assertEqual(self.pool.get_worktrees(), [])

    def test_invalid_state_transitions(self) -> None:
        wt_path = self.jiri_root / "worktrees" / "wt1"
        wt_path.mkdir(parents=True, exist_ok=True)
        with open(self.pool.registry_file, "w") as f:
            f.write(f"{wt_path}\n")

        wt = self.pool.get_worktree_by_name("wt1")
        self.assertEqual(wt.get_state(), WorktreeState.FREE)

        # Cannot release if FREE
        with self.assertRaises(RuntimeError):
            wt.release_lease()

        # Lease it
        wt.acquire_lease(task_id="test")
        self.assertEqual(wt.get_state(), WorktreeState.LEASED)

        # Cannot lease again
        with self.assertRaises(RuntimeError):
            wt.acquire_lease(task_id="test2")

    def test_get_any_free_worktree(self) -> None:
        with self.assertRaises(NoFreeWorktreesError):
            self.pool.get_any_free_worktree()

        wt_path = self.jiri_root / "worktrees" / "wt1"
        wt_path.mkdir(parents=True, exist_ok=True)
        with open(self.pool.registry_file, "w") as f:
            f.write(f"{wt_path}\n")

        wt = self.pool.get_any_free_worktree()
        self.assertEqual(wt.name, "wt1")

    @patch("worktree.run_git")
    def test_release_detaches_head(self, mock_run_git: MagicMock) -> None:
        wt_path = self.jiri_root / "worktrees" / "wt1"
        wt_path.mkdir(parents=True, exist_ok=True)
        with open(self.pool.registry_file, "w") as f:
            f.write(f"{wt_path}\n")
        wt = self.pool.get_worktrees()[0]
        wt.acquire_lease("my-task")
        wt.release_lease()
        mock_run_git.assert_called_once_with(
            wt.path, ["checkout", "--detach"], quiet=True, check=True
        )

    def test_generate_random_pool_name(self) -> None:
        name = self.pool._generate_random_pool_name()
        self.assertIn("-", name)
        adj, noun = name.split("-", 1)
        self.assertIn(adj, ADJECTIVES)
        self.assertIn(noun, NOUNS)


class TestAddSubcommand(unittest.TestCase):
    def setUp(self) -> None:
        self.temp_dir = tempfile.TemporaryDirectory()
        self.fuchsia_dir = Path(self.temp_dir.name)
        self.jiri_root = self.fuchsia_dir / ".jiri_root"
        self.jiri_root.mkdir(parents=True, exist_ok=True)
        self.pool = WorktreePool(fuchsia_dir=str(self.fuchsia_dir))

    def tearDown(self) -> None:
        self.temp_dir.cleanup()

    def test_add_claims_slot(self) -> None:
        wt_path = self.jiri_root / "worktrees" / "wt1"
        wt_path.mkdir(parents=True, exist_ok=True)
        self.pool.registry_file.write_text(f"{wt_path}\n")

        args = argparse.Namespace(
            name="my-feat", pool_name=None, sync=False, json=False
        )
        with patch("sys.stdout", new_callable=StringIO) as mock_out:
            add_cmd.run(args, self.pool)
            self.assertIn(".jiri_root/worktrees/my-feat", mock_out.getvalue())
        wt = self.pool.get_worktrees()[0]
        self.assertEqual(wt.get_state(), WorktreeState.LEASED)
        lease = wt.get_lease_info()
        assert lease is not None
        self.assertEqual(lease.task_id, "my-feat")

    @patch("subcommands.add.run_jiri")
    def test_add_with_sync_order(self, mock_run_jiri: MagicMock) -> None:
        wt_path = self.jiri_root / "worktrees" / "wt1"
        wt_path.mkdir(parents=True, exist_ok=True)
        self.pool.registry_file.write_text(f"{wt_path}\n")

        args = argparse.Namespace(
            name="my-feat", pool_name=None, sync=True, json=False
        )
        manager = MagicMock()
        manager.attach_mock(mock_run_jiri, "mock_run_jiri")

        with patch("sys.stdout", new_callable=StringIO):
            add_cmd.run(args, self.pool)

        expected_calls = [
            unittest.mock.call.mock_run_jiri(
                self.jiri_root,
                ["worktree", "sync", str(wt_path)],
                check=True,
            ),
        ]
        self.assertEqual(manager.mock_calls, expected_calls)

    @patch("worktree.run_git")
    def test_remove_by_task_id(self, mock_run_git: MagicMock) -> None:
        wt_path = self.jiri_root / "worktrees" / "wt1"
        wt_path.mkdir(parents=True, exist_ok=True)
        self.pool.registry_file.write_text(f"{wt_path}\n")
        wt = self.pool.get_worktrees()[0]
        wt.acquire_lease("my-task-123")

        args = argparse.Namespace(name="my-task-123")
        remove_cmd.run(args, self.pool)
        self.assertEqual(wt.get_state(), WorktreeState.FREE)

        args_pool = argparse.Namespace(name="my-task-123", force=False)
        with self.assertRaises(KeyError):
            pool_remove_cmd.run(args_pool, self.pool)

    @patch("worktree_pool.run_jiri")
    @patch("sys.stderr", new_callable=StringIO)
    def test_add_auto_provisions_when_no_free_slots(
        self, mock_stderr: MagicMock, mock_run_jiri: MagicMock
    ) -> None:
        from typing import Any

        def mock_run_jiri_side_effect(
            jiri_root: Path, args: list[str], **kwargs: Any
        ) -> MagicMock:
            if args[0:2] == ["worktree", "add"]:
                path = Path(args[2])
                path.mkdir(parents=True, exist_ok=True)
                with open(self.pool.registry_file, "a") as f:
                    f.write(f"{path}\n")
            return MagicMock()

        mock_run_jiri.side_effect = mock_run_jiri_side_effect

        args = argparse.Namespace(
            name="my-feat", pool_name=None, sync=False, json=False
        )

        with patch("sys.stdout", new_callable=StringIO) as mock_out:
            add_cmd.run(args, self.pool)
            self.assertIn(
                "No free worktrees available in the pool. Provisioning a new one...",
                mock_stderr.getvalue(),
            )
            self.assertIn(".jiri_root/worktrees/my-feat", mock_out.getvalue())

        mock_run_jiri.assert_called_once()
        call_args = mock_run_jiri.call_args[0][1]
        self.assertEqual(call_args[0:2], ["worktree", "add"])

        symlink_path = self.pool.worktrees_dir / "my-feat"
        self.assertTrue(symlink_path.is_symlink())

        worktrees = self.pool.get_worktrees()
        self.assertEqual(len(worktrees), 1)
        wt = worktrees[0]
        self.assertEqual(wt.get_state(), WorktreeState.LEASED)
        lease = wt.get_lease_info()
        self.assertIsNotNone(lease)
        assert lease is not None
        self.assertEqual(lease.task_id, "my-feat")

    def test_add_already_leased_task_fails(self) -> None:
        wt_path1 = self.jiri_root / "worktrees" / "wt1"
        wt_path2 = self.jiri_root / "worktrees" / "wt2"
        wt_path1.mkdir(parents=True, exist_ok=True)
        wt_path2.mkdir(parents=True, exist_ok=True)
        self.pool.registry_file.write_text(f"{wt_path1}\n{wt_path2}\n")

        args1 = argparse.Namespace(
            name="my-feat", pool_name=None, sync=False, json=False
        )
        add_cmd.run(args1, self.pool)

        args2 = argparse.Namespace(
            name="my-feat", pool_name=None, sync=False, json=False
        )
        with self.assertRaises(ValueError) as context:
            add_cmd.run(args2, self.pool)
        self.assertIn("already active", str(context.exception))


class TestPoolAddSubcommand(unittest.TestCase):
    def setUp(self) -> None:
        self.temp_dir = tempfile.TemporaryDirectory()
        self.fuchsia_dir = Path(self.temp_dir.name)
        self.jiri_root = self.fuchsia_dir / ".jiri_root"
        self.jiri_root.mkdir(parents=True, exist_ok=True)
        self.pool = WorktreePool(fuchsia_dir=str(self.fuchsia_dir))
        self.patcher_jiri = patch("worktree_pool.run_jiri")
        self.patcher_fx = patch("subcommands.pool_add.run_fx")
        self.mock_jiri = self.patcher_jiri.start()
        self.mock_fx = self.patcher_fx.start()

    def tearDown(self) -> None:
        self.patcher_jiri.stop()
        self.patcher_fx.stop()
        self.temp_dir.cleanup()

    def test_add_multiple_set_args(self) -> None:
        wt_path = self.pool.worktrees_dir / "wt1"
        self.pool.registry_file.write_text(f"{wt_path}\n")
        args = argparse.Namespace(
            name="wt1",
            set=["core.x64 --out out/core", "workbench.arm64 --out out/wb"],
        )
        pool_add_cmd.run(args, self.pool)
        self.assertEqual(self.mock_fx.call_count, 2)
        self.pool.get_worktree_by_name("wt1")

    def test_add_symlink_local_dir(self) -> None:
        src_local = self.fuchsia_dir / "local"
        src_local.mkdir(parents=True, exist_ok=True)
        (src_local / "file.txt").write_text("hello")

        wt_path = self.pool.worktrees_dir / "wt1"
        args = argparse.Namespace(
            name="wt1",
            set=None,
            symlink_local=True,
            copy_local=False,
        )
        pool_add_cmd.run(args, self.pool)

        dest_local = wt_path / "local"
        self.assertTrue(dest_local.is_symlink())
        self.assertEqual(dest_local.resolve(), src_local.resolve())

    def test_add_copy_local_dir(self) -> None:
        src_local = self.fuchsia_dir / "local"
        src_local.mkdir(parents=True, exist_ok=True)
        (src_local / "file.txt").write_text("hello")

        wt_path = self.pool.worktrees_dir / "wt1"
        args = argparse.Namespace(
            name="wt1",
            set=None,
            symlink_local=False,
            copy_local=True,
        )
        pool_add_cmd.run(args, self.pool)

        dest_local = wt_path / "local"
        self.assertTrue(dest_local.exists())
        self.assertFalse(dest_local.is_symlink())
        self.assertTrue((dest_local / "file.txt").exists())
        self.assertEqual((dest_local / "file.txt").read_text(), "hello")

    def test_add_local_dir_missing(self) -> None:
        args = argparse.Namespace(
            name="wt1",
            set=None,
            symlink_local=True,
            copy_local=False,
        )
        with self.assertRaises(SystemExit):
            pool_add_cmd.run(args, self.pool)


class TestWorktreeSelectedBuildDir(unittest.TestCase):
    def setUp(self) -> None:
        self.temp_dir = tempfile.TemporaryDirectory()
        self.wt_path = Path(self.temp_dir.name)
        self.wt = Worktree(
            name="test-wt",
            path=self.wt_path,
            main_checkout_dir=self.wt_path / "main",
        )

    def tearDown(self) -> None:
        self.temp_dir.cleanup()

    def test_default_selected_build_dir(self) -> None:
        with self.assertRaises(FileNotFoundError):
            self.wt.selected_build_dir()

    def test_empty_selected_build_dir(self) -> None:
        fx_build_dir_file = self.wt_path / ".fx-build-dir"
        fx_build_dir_file.write_text("")
        with self.assertRaises(ValueError):
            self.wt.selected_build_dir()

    def test_custom_selected_build_dir(self) -> None:
        fx_build_dir_file = self.wt_path / ".fx-build-dir"
        fx_build_dir_file.write_text("out/custom-dir")
        expected = (self.wt_path / "out" / "custom-dir").resolve()
        self.assertEqual(self.wt.selected_build_dir(), expected)

    def test_printer_highlights_selected_build_dir(self) -> None:
        other_dir = self.wt_path / "out" / "other"
        other_dir.mkdir(parents=True, exist_ok=True)
        (other_dir / "args.gn").write_text(
            'build_info_product = "core"\nbuild_info_board = "x64"\n'
        )

        default_symlink = self.wt_path / "out" / "default"
        default_symlink.symlink_to("other")

        fx_build_dir_file = self.wt_path / ".fx-build-dir"
        fx_build_dir_file.write_text("out/default")

        another_dir = self.wt_path / "out" / "another"
        another_dir.mkdir(parents=True, exist_ok=True)
        (another_dir / "args.gn").write_text(
            'build_info_product = "workbench"\nbuild_info_board = "arm64"\n'
        )

        with patch.object(
            self.wt, "get_sync_status", return_value=(SyncStatus.SYNCED, 0, 0)
        ):
            with patch("sys.stdout", new_callable=StringIO) as mock_out:
                WorktreePrinter.print_worktrees([self.wt])
                output = mock_out.getvalue()

        self.assertIn("out/other *:", output)
        self.assertIn("out/another:", output)
        self.assertNotIn("out/default", output)
        self.assertNotIn("out/another *:", output)

    def test_printer_highlights_selected_build_dir_with_color(self) -> None:
        other_dir = self.wt_path / "out" / "other"
        other_dir.mkdir(parents=True, exist_ok=True)
        (other_dir / "args.gn").write_text(
            'build_info_product = "core"\nbuild_info_board = "x64"\n'
        )

        default_symlink = self.wt_path / "out" / "default"
        default_symlink.symlink_to("other")

        fx_build_dir_file = self.wt_path / ".fx-build-dir"
        fx_build_dir_file.write_text("out/default")

        with patch.object(
            self.wt, "get_sync_status", return_value=(SyncStatus.SYNCED, 0, 0)
        ):
            with patch("utils.USE_COLORS", True):
                with patch("sys.stdout", new_callable=StringIO) as mock_out:
                    WorktreePrinter.print_worktrees([self.wt])
                    output = mock_out.getvalue()

        # Colors.GREEN is \033[92m, Colors.RESET is \033[0m
        self.assertIn("\033[92m", output)
        self.assertIn("out/other *:", output)
        self.assertIn("\033[0m", output)

    def test_printer_no_builds_no_trailing_newline(self) -> None:
        with patch.object(
            self.wt, "get_sync_status", return_value=(SyncStatus.SYNCED, 0, 0)
        ):
            with patch("utils.USE_COLORS", False):
                with patch("sys.stdout", new_callable=StringIO) as mock_out:
                    WorktreePrinter.print_worktrees([self.wt])
                    output = mock_out.getvalue()

        self.assertEqual(output, "test-wt\n")


class TestCompletionChoices(unittest.TestCase):
    def setUp(self) -> None:
        self.temp_dir = tempfile.TemporaryDirectory()
        self.fuchsia_dir = Path(self.temp_dir.name)
        self.jiri_root = self.fuchsia_dir / ".jiri_root"
        self.jiri_root.mkdir(parents=True, exist_ok=True)
        self.pool = WorktreePool(fuchsia_dir=str(self.fuchsia_dir))

        # Create physical worktree dirs
        self.wt1_path = self.pool.worktrees_dir / "wt1"
        self.wt2_path = self.pool.worktrees_dir / "wt2"
        self.wt3_path = self.pool.worktrees_dir / "wt3"

        self.wt1_path.mkdir(parents=True, exist_ok=True)
        self.wt2_path.mkdir(parents=True, exist_ok=True)
        self.wt3_path.mkdir(parents=True, exist_ok=True)

        # Register them
        with open(self.pool.registry_file, "w") as f:
            f.write(f"{self.wt1_path}\n")
            f.write(f"{self.wt2_path}\n")
            f.write(f"{self.wt3_path}\n")

        # wt1 is free
        # wt2 is leased
        wt2 = self.pool.get_worktree_by_name("wt2")
        wt2.acquire_lease(task_id="task-wt2")

        # wt3 is leased but also has leased symlink with different name
        wt3 = self.pool.get_worktree_by_name("wt3")
        wt3.acquire_lease(task_id="task-wt3")

        # Create leased symlink for task-wt3
        self.leased_link = self.pool.worktrees_dir / "task-wt3"
        self.leased_link.symlink_to("wt3")

    def tearDown(self) -> None:
        self.temp_dir.cleanup()

    def test_filter_physical_all(self) -> None:
        choices = self.pool.get_completion_choices("physical_all")
        self.assertEqual(sorted(choices), ["wt1", "wt2", "wt3"])

    def test_filter_physical_free(self) -> None:
        choices = self.pool.get_completion_choices("physical_free")
        self.assertEqual(choices, ["wt1"])

    def test_filter_physical_leased(self) -> None:
        choices = self.pool.get_completion_choices("physical_leased")
        self.assertEqual(sorted(choices), ["wt2", "wt3"])

    def test_filter_leased(self) -> None:
        choices = self.pool.get_completion_choices("leased")
        self.assertEqual(choices, ["task-wt3"])

    def test_filter_all(self) -> None:
        choices = self.pool.get_completion_choices("all")
        self.assertEqual(sorted(choices), ["task-wt3", "wt1", "wt2", "wt3"])


if __name__ == "__main__":
    unittest.main()
