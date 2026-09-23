#!/usr/bin/env fuchsia-vendored-python
# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import subprocess
import unittest
from pathlib import Path
from unittest import mock

import cl_utils
import dlwrap
import remote_action
import remotetool


class MainArgParserTests(unittest.TestCase):
    def test_defaults(self) -> None:
        args = dlwrap._MAIN_ARG_PARSER.parse_args([])
        self.assertFalse(args.verbose)
        self.assertFalse(args.dry_run)
        self.assertFalse(args.undownload)
        self.assertEqual(args.download, [])
        self.assertEqual(args.download_list, [])
        self.assertEqual(args.command, [])

    def test_verbose(self) -> None:
        args = dlwrap._MAIN_ARG_PARSER.parse_args(["--verbose"])
        self.assertTrue(args.verbose)

    def test_dry_run(self) -> None:
        args = dlwrap._MAIN_ARG_PARSER.parse_args(["--dry-run"])
        self.assertTrue(args.dry_run)

    def test_undownload(self) -> None:
        args = dlwrap._MAIN_ARG_PARSER.parse_args(["--undownload"])
        self.assertTrue(args.undownload)

    def test_download(self) -> None:
        args = dlwrap._MAIN_ARG_PARSER.parse_args(
            ["--download", "aa.o", "bb.o"]
        )
        self.assertEqual(args.download, [Path("aa.o"), Path("bb.o")])

    def test_download_list(self) -> None:
        args = dlwrap._MAIN_ARG_PARSER.parse_args(
            ["--download_list", "f1.rsp", "f2.rsp"]
        )
        self.assertEqual(args.download_list, [Path("f1.rsp"), Path("f2.rsp")])

    def test_command(self) -> None:
        args = dlwrap._MAIN_ARG_PARSER.parse_args(["--", "cat", "dog.txt"])
        self.assertEqual(args.command, ["cat", "dog.txt"])


_fake_downloader = remotetool.RemoteTool(
    reproxy_cfg={
        "service": "foo.build.service:443",
        "instance": "my-project/remote/instances/default",
    }
)


def _fake_download(
    packed_args: tuple[Path, remotetool.RemoteTool, Path]
) -> tuple[Path, cl_utils.SubprocessResult]:
    # For mocking dlwrap._download_for_mp.
    # defined because multiprocessing cannot serialize mocks
    stub_path, downloader, working_dir_abs = packed_args
    return (stub_path, cl_utils.SubprocessResult(0))


def _fake_download_fail(
    packed_args: tuple[Path, remotetool.RemoteTool, Path]
) -> tuple[Path, cl_utils.SubprocessResult]:
    # For mocking dlwrap._download_for_mp.
    # defined because multiprocessing cannot serialize mocks
    stub_path, downloader, working_dir_abs = packed_args
    return (stub_path, cl_utils.SubprocessResult(1))


class DownloadArtifactsTests(unittest.TestCase):
    def _stub_info(self, path: Path) -> remote_action.DownloadStubInfo:
        return remote_action.DownloadStubInfo(
            path=path,
            type="file",
            blob_digest="f33333df44444ce/124",
            action_digest="47ac8eb38351/41",
            build_id="50f93-a38e-b112",
        )

    def test_success(self) -> None:
        path = Path("road/to/perdition.obj")
        exec_root = Path("/exec/root")
        working_dir = exec_root / "work"
        download_status = 0
        with mock.patch.object(
            remote_action,
            "download_input_stub_paths_batch",
            return_value={path: cl_utils.SubprocessResult(download_status)},
        ) as mock_download:
            status = dlwrap.download_artifacts(
                [path],
                downloader=_fake_downloader,
                working_dir_abs=working_dir,
            )
        self.assertEqual(status, download_status)
        mock_download.assert_called_once()

    def test_failure(self) -> None:
        path = Path("highway/to/hell.obj")
        exec_root = Path("/exec/root")
        working_dir = exec_root / "work"
        download_status = 1
        with mock.patch.object(
            remote_action,
            "download_input_stub_paths_batch",
            return_value={path: cl_utils.SubprocessResult(download_status)},
        ) as mock_download:
            status = dlwrap.download_artifacts(
                [path],
                downloader=_fake_downloader,
                working_dir_abs=working_dir,
            )
        self.assertEqual(status, download_status)
        mock_download.assert_called_once()


class MainTests(unittest.TestCase):
    def test_dry_run(self) -> None:
        exec_root = Path("/exec/root")
        working_dir = exec_root / "work"
        with mock.patch.object(
            dlwrap, "download_artifacts", return_value=0
        ) as mock_download:
            with mock.patch.object(subprocess, "call") as mock_run:
                status = dlwrap._main(
                    ["--dry-run", "--", "cat", "foo"],
                    downloader=_fake_downloader,
                    working_dir_abs=working_dir,
                )
        self.assertEqual(status, 0)
        mock_run.assert_not_called()

    def test_download_fail(self) -> None:
        exec_root = Path("/exec/root")
        working_dir = exec_root / "work"
        with mock.patch.object(
            dlwrap, "download_artifacts", return_value=1
        ) as mock_download:
            with mock.patch.object(subprocess, "call") as mock_run:
                status = dlwrap._main(
                    ["--", "cat", "foo"],
                    downloader=_fake_downloader,
                    working_dir_abs=working_dir,
                )
        mock_download.assert_called_once()
        self.assertEqual(status, 1)
        mock_run.assert_not_called()

    def test_no_command(self) -> None:
        exec_root = Path("/exec/root")
        working_dir = exec_root / "work"
        with mock.patch.object(
            dlwrap, "download_artifacts", return_value=0
        ) as mock_download:
            with mock.patch.object(
                subprocess, "call", return_value=0
            ) as mock_run:
                status = dlwrap._main(
                    [], downloader=_fake_downloader, working_dir_abs=working_dir
                )
        self.assertEqual(status, 0)
        mock_run.assert_not_called()

    def test_success(self) -> None:
        exec_root = Path("/exec/root")
        working_dir = exec_root / "work"
        with mock.patch.object(
            dlwrap, "download_artifacts", return_value=0
        ) as mock_download:
            with mock.patch.object(
                subprocess, "call", return_value=0
            ) as mock_run:
                status = dlwrap._main(
                    ["--", "cat", "foo"],
                    downloader=_fake_downloader,
                    working_dir_abs=working_dir,
                )
        self.assertEqual(status, 0)
        mock_run.assert_called_with(["cat", "foo"])

    def test_success_from_list(self) -> None:
        path = "dir/file.o"
        rspfile = "f.rsp"
        exec_root = Path("/exec/root")
        working_dir = exec_root / "work"
        with mock.patch.object(
            dlwrap, "download_artifacts", return_value=0
        ) as mock_download:
            with mock.patch.object(
                subprocess, "call", return_value=0
            ) as mock_run:
                with mock.patch.object(
                    cl_utils,
                    "expand_paths_from_files",
                    return_value=iter([Path(path)]),
                ) as mock_expand_files:
                    status = dlwrap._main(
                        ["--download_list", rspfile, "--", "cat", "foo"],
                        downloader=_fake_downloader,
                        working_dir_abs=working_dir,
                    )
        self.assertEqual(status, 0)
        mock_expand_files.assert_called_with([Path(rspfile)])
        mock_run.assert_called_with(["cat", "foo"])

    def test_undownload(self) -> None:
        exec_root = Path("/exec/root")
        working_dir = exec_root / "work"
        with mock.patch.object(
            dlwrap, "download_artifacts", return_value=0
        ) as mock_download:
            with mock.patch.object(
                remote_action, "undownload"
            ) as mock_undownload:
                with mock.patch.object(
                    subprocess, "call", return_value=0
                ) as mock_run:
                    status = dlwrap._main(
                        [
                            "--download",
                            "fetch.me",
                            "--undownload",
                            "--",
                            "cat",
                            "foo",
                        ],
                        downloader=_fake_downloader,
                        working_dir_abs=working_dir,
                    )
        self.assertEqual(status, 0)
        mock_download.assert_not_called()
        mock_undownload.assert_called()
        mock_run.assert_not_called()


if __name__ == "__main__":
    remote_action.init_from_main_once()
    unittest.main()
