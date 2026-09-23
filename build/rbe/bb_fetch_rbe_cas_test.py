#!/usr/bin/env fuchsia-vendored-python
# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import contextlib
import io
import unittest
from pathlib import Path
from unittest import mock

import bb_fetch_rbe_cas
import bbtool
import cl_utils
import remotetool
import reproxy_logs


class MainArgParserTests(unittest.TestCase):
    def test_with_bbid(self):
        args = bb_fetch_rbe_cas._MAIN_ARG_PARSER.parse_args(
            ["--bbid", "1234", "--path", "obj/hello.o"]
        )
        self.assertEqual(args.bbid, "1234")
        self.assertEqual(args.path, Path("obj/hello.o"))
        self.assertIsNone(args.output)
        self.assertFalse(args.verbose)
        self.assertIsNone(args.reproxy_log)

    def test_with_reproxy_log(self):
        args = bb_fetch_rbe_cas._MAIN_ARG_PARSER.parse_args(
            ["--reproxy_log", "x.rrpl", "--path", "obj/hello.o", "--verbose"]
        )
        self.assertIsNone(args.bbid)
        self.assertEqual(args.path, Path("obj/hello.o"))
        self.assertIsNone(args.output)
        self.assertTrue(args.verbose)
        self.assertEqual(args.reproxy_log, Path("x.rrpl"))


_reproxy_cfg = {
    "service": "some.remote.service.com:443",
    "instance": "projects/my-project/instance/default",
}


class DownloadArtifactTests(unittest.TestCase):
    @property
    def downloader(self):
        return remotetool.RemoteTool(reproxy_cfg=_reproxy_cfg)

    def test_success(self):
        with mock.patch.object(
            remotetool.RemoteTool,
            "download_blob",
            return_value=cl_utils.SubprocessResult(0),
        ) as mock_download:
            exit_code = bb_fetch_rbe_cas.download_artifact(
                self.downloader, "8787def87e90/434", Path("storage/bin")
            )
            self.assertEqual(exit_code, 0)

    def test_failure(self):
        with mock.patch.object(
            remotetool.RemoteTool,
            "download_blob",
            return_value=cl_utils.SubprocessResult(1),
        ) as mock_download:
            exit_code = bb_fetch_rbe_cas.download_artifact(
                self.downloader, "1c1df1000/93", Path("shipping/dock")
            )
            self.assertEqual(exit_code, 1)


class FetchArtifactFromReproxyLogTests(unittest.TestCase):
    @property
    def downloader(self):
        return remotetool.RemoteTool(reproxy_cfg=_reproxy_cfg)

    def test_digest_found(self):
        reproxy_log = Path("cached.rrpl")
        artifact_path = Path("obj/hello.cc.o")
        cfg = Path("reproxy.cfg")
        digest = "f1233c9744d/101"
        output = Path("fetched-hello.cc.o")
        status = 0
        with mock.patch.object(
            reproxy_logs, "lookup_output_file_digest", return_value=digest
        ) as mock_lookup:
            with mock.patch.object(
                remotetool, "configure_remotetool", return_value=self.downloader
            ) as mock_downloader:
                with mock.patch.object(
                    bb_fetch_rbe_cas, "download_artifact", return_value=status
                ) as mock_download:
                    self.assertEqual(
                        bb_fetch_rbe_cas.fetch_artifact_from_reproxy_log(
                            reproxy_log=reproxy_log,
                            artifact_path=artifact_path,
                            cfg=cfg,
                            output=output,
                        ),
                        status,
                    )
        mock_lookup.assert_called_once_with(log=reproxy_log, path=artifact_path)
        mock_downloader.assert_called_once_with(cfg)
        mock_download.assert_called_once_with(self.downloader, digest, output)

    def test_digest_not_found(self):
        reproxy_log = Path("cached.rrpl")
        artifact_path = Path("obj/hello.cc.o")
        with mock.patch.object(
            reproxy_logs, "lookup_output_file_digest", return_value=None
        ) as mock_lookup:
            self.assertEqual(
                bb_fetch_rbe_cas.fetch_artifact_from_reproxy_log(
                    reproxy_log=reproxy_log,
                    artifact_path=artifact_path,
                    cfg=Path("reproxy.cfg"),
                    output=Path("fetched-hello.cc.o"),
                ),
                1,
            )
        mock_lookup.assert_called_once_with(log=reproxy_log, path=artifact_path)

    def test_download_failed(self):
        reproxy_log = Path("smashed.rrpl")
        artifact_path = Path("obj/h3llo.cc.o")
        cfg = Path("reproxy_2.cfg")
        output = Path("fetched-h3llo.cc.o")
        digest = "a87e1321/88"
        status = 1
        with mock.patch.object(
            reproxy_logs, "lookup_output_file_digest", return_value=digest
        ) as mock_lookup:
            with mock.patch.object(
                remotetool, "configure_remotetool", return_value=self.downloader
            ) as mock_downloader:
                with mock.patch.object(
                    bb_fetch_rbe_cas, "download_artifact", return_value=status
                ) as mock_download:
                    self.assertEqual(
                        bb_fetch_rbe_cas.fetch_artifact_from_reproxy_log(
                            reproxy_log=reproxy_log,
                            artifact_path=artifact_path,
                            cfg=cfg,
                            output=output,
                        ),
                        status,
                    )
        mock_lookup.assert_called_once_with(log=reproxy_log, path=artifact_path)
        mock_downloader.assert_called_once_with(cfg)
        mock_download.assert_called_once_with(self.downloader, digest, output)


class MainTests(unittest.TestCase):
    def test_e2e_using_bbid_success(self):
        bb = bbtool._BB_TOOL
        bbid = "b4356254"
        artifact_path = Path("foo/bar/baz.rlib")
        cfg = bb_fetch_rbe_cas._REPROXY_CFG
        reproxy_log_path = Path("/some/where/in/temp/foo.rrpl")
        with mock.patch.object(
            bbtool, "fetch_reproxy_log_from_bbid", return_value=reproxy_log_path
        ) as mock_fetch_log:
            with mock.patch.object(
                bb_fetch_rbe_cas,
                "fetch_artifact_from_reproxy_log",
                return_value=0,
            ) as mock_fetch_artifact:
                self.assertEqual(
                    bb_fetch_rbe_cas.main(
                        ["--bbid", bbid, "--path", str(artifact_path)]
                    ),
                    0,
                )
        mock_fetch_log.assert_called_once_with(
            bbpath=bb, bbid=bbid.lstrip("b"), verbose=False
        )
        mock_fetch_artifact.assert_called_once_with(
            reproxy_log=reproxy_log_path,
            artifact_path=artifact_path,
            cfg=cfg,
            output=Path(artifact_path.name),
            verbose=False,
        )

    def test_e2e_bbid_to_log_only(self):
        bb = bbtool._BB_TOOL
        bbid = "b991918261"
        reproxy_log_path = Path("/path/to/cache/foobar.rrpl")
        result = io.StringIO()
        with contextlib.redirect_stdout(result):
            with mock.patch.object(
                bbtool,
                "fetch_reproxy_log_from_bbid",
                return_value=reproxy_log_path,
            ) as mock_fetch_log:
                self.assertEqual(bb_fetch_rbe_cas.main(["--bbid", bbid]), 0)
        self.assertTrue(
            result.getvalue().endswith(f"reproxy log: {reproxy_log_path}\n")
        )
        mock_fetch_log.assert_called_once_with(
            bbpath=bb, bbid=bbid.lstrip("b"), verbose=False
        )

    def test_e2e_using_reproxy_log_success(self):
        artifact_path = Path("foo/bar/baz.rlib")
        cfg = bb_fetch_rbe_cas._REPROXY_CFG
        reproxy_log_path = Path("use/me.rrpl")
        with mock.patch.object(
            bb_fetch_rbe_cas, "fetch_artifact_from_reproxy_log", return_value=0
        ) as mock_fetch_artifact:
            self.assertEqual(
                bb_fetch_rbe_cas.main(
                    [
                        "--reproxy_log",
                        str(reproxy_log_path),
                        "--path",
                        str(artifact_path),
                    ]
                ),
                0,
            )
        # bypassed call to fetch_reproxy_log_from_bbid
        mock_fetch_artifact.assert_called_once_with(
            reproxy_log=reproxy_log_path,
            artifact_path=artifact_path,
            cfg=cfg,
            output=Path(artifact_path.name),
            verbose=False,
        )

    def test_e2e_reproxy_log_failed(self):
        bb = bbtool._BB_TOOL
        bbid = "b4356254"
        with mock.patch.object(
            bbtool, "fetch_reproxy_log_from_bbid", return_value=None
        ) as mock_fetch_log:
            self.assertEqual(
                bb_fetch_rbe_cas.main(
                    ["--bbid", bbid, "--path", "foo/bar/baz.rlib"]
                ),
                1,
            )
        mock_fetch_log.assert_called_once_with(
            bbpath=bb, bbid=bbid.lstrip("b"), verbose=False
        )

    def test_e2e_bb_error(self):
        bbid = "b99669966"
        with mock.patch.object(
            bbtool.BuildBucketTool,
            "get_rbe_build_info",
            side_effect=bbtool.BBError("error"),
        ) as mock_rbe_build_info:
            self.assertEqual(
                bb_fetch_rbe_cas.main(
                    ["--bbid", bbid, "--path", "zoo/bar/faz.rlib"]
                ),
                1,
            )
        mock_rbe_build_info.assert_called_once_with(
            bbid.lstrip("b"), verbose=False
        )

    def test_e2e_download_artifact_error(self):
        bb = bbtool._BB_TOOL
        bbid = "b881231281"
        reproxy_log_path = Path("/some/where/in/temp/foo.rrpl")
        artifact_path = Path("foo/bar/baz.rlib")
        cfg = bb_fetch_rbe_cas._REPROXY_CFG
        status = 1
        with mock.patch.object(
            bbtool, "fetch_reproxy_log_from_bbid", return_value=reproxy_log_path
        ) as mock_fetch_log:
            with mock.patch.object(
                bb_fetch_rbe_cas,
                "fetch_artifact_from_reproxy_log",
                return_value=status,
            ) as mock_fetch_artifact:
                self.assertEqual(
                    bb_fetch_rbe_cas.main(
                        ["--bbid", bbid, "--path", str(artifact_path)]
                    ),
                    status,
                )
        mock_fetch_log.assert_called_once_with(
            bbpath=bb, bbid=bbid.lstrip("b"), verbose=False
        )
        mock_fetch_artifact.assert_called_once_with(
            reproxy_log=reproxy_log_path,
            artifact_path=artifact_path,
            cfg=cfg,
            output=Path(artifact_path.name),
            verbose=False,
        )


if __name__ == "__main__":
    unittest.main()
