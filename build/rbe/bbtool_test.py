#!/usr/bin/env fuchsia-vendored-python
# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import contextlib
import io
import unittest
from pathlib import Path
from typing import Any
from unittest import mock

import bbtool
import cas
import cl_utils


class MainArgParserTests(unittest.TestCase):
    def test_fetch_reproxy_log(self) -> None:
        args = bbtool._MAIN_ARG_PARSER.parse_args(
            ["--bbid", "1234", "fetch_reproxy_log"]
        )
        self.assertEqual(args.bbid, "1234")
        self.assertFalse(args.verbose)


class LogCachePathTests(unittest.TestCase):
    def test_path(self) -> None:
        build_id = "9910918"
        log_name = "reproxy_5.6.7.8.rrpl"
        cache_path = bbtool.log_cache_path(build_id, log_name)
        self.assertEqual(cache_path.name, log_name)
        self.assertEqual(cache_path.parent.name, f"b{build_id}")


class BuildBucketToolTests(unittest.TestCase):
    def test_get_json_fields_success(self) -> None:
        bb = bbtool.BuildBucketTool()
        with mock.patch.object(
            cl_utils,
            "subprocess_call",
            return_value=cl_utils.SubprocessResult(0, stdout=["{}"]),
        ) as mock_call:
            bb_json = bb.get_json_fields("5432")
            self.assertEqual(bb_json, dict())
        mock_call.assert_called_once()

    def test_get_json_fields_error(self) -> None:
        bb = bbtool.BuildBucketTool()
        with mock.patch.object(
            cl_utils,
            "subprocess_call",
            return_value=cl_utils.SubprocessResult(1, stderr=["oops"]),
        ) as mock_call:
            with self.assertRaises(bbtool.BBError):
                bb.get_json_fields("6432")
        mock_call.assert_called_once()

    def test_download_reproxy_log_success(self) -> None:
        bb = bbtool.BuildBucketTool()
        with mock.patch.object(
            cl_utils,
            "subprocess_call",
            return_value=cl_utils.SubprocessResult(0),
        ) as mock_call:
            self.assertEqual(
                bb.download_reproxy_log("1271", "reproxy_3.1415926.rrpl"), "\n"
            )
        mock_call.assert_called_once()

    def test_download_reproxy_log_error(self) -> None:
        bb = bbtool.BuildBucketTool()
        with mock.patch.object(
            cl_utils,
            "subprocess_call",
            return_value=cl_utils.SubprocessResult(1),
        ) as mock_call:
            with self.assertRaises(bbtool.BBError):
                bb.download_reproxy_log("1271", "reproxy_3.1415926.rrpl")
        mock_call.assert_called_once()

    def test_get_rbe_build_info_no_child(self) -> None:
        bb = bbtool.BuildBucketTool()
        bbid = "8888"
        build_json: dict[str, Any] = {"output": {"properties": {}}}
        with mock.patch.object(
            bbtool.BuildBucketTool, "get_json_fields", return_value=build_json
        ) as mock_get_json:
            rbe_build_id, rbe_build_json = bb.get_rbe_build_info(bbid)
            self.assertEqual(rbe_build_id, bbid)
            self.assertEqual(rbe_build_json, build_json)

    def test_get_rbe_build_info_with_child(self) -> None:
        bb = bbtool.BuildBucketTool()
        parent_bbid = "9977"
        child_bbid = "0404"
        parent_build_json = {
            "output": {"properties": {"child_build_id": child_bbid}}
        }
        child_build_json = {
            "output": {"properties": {"rpl_files": ["foo.rrpl"]}}
        }
        with mock.patch.object(
            bbtool.BuildBucketTool,
            "get_json_fields",
            side_effect=[parent_build_json, child_build_json],
        ) as mock_get_json:
            rbe_build_id, rbe_build_json = bb.get_rbe_build_info(parent_bbid)
            self.assertEqual(rbe_build_id, child_bbid)
            self.assertEqual(rbe_build_json, child_build_json)


class FetchReproxyLogFromBbidTests(unittest.TestCase):
    def test_lookup_and_fetch_cached_legacy(self) -> None:
        bb_json = {
            "output": {"properties": {"rpl_files": ["reproxy_2.718.rrpl"]}}
        }
        with mock.patch.object(
            bbtool.BuildBucketTool,
            "get_rbe_build_info",
            return_value=("8728721", bb_json),
        ) as mock_rbe_build_info:
            with mock.patch.object(
                bbtool,
                "_cache_path_exists",
                return_value=True,
            ) as mock_cache_exists:
                bbtool.fetch_reproxy_log_from_bbid(
                    bbpath=Path("bb"), bbid="b789789"
                )

        mock_rbe_build_info.assert_called_once()
        mock_cache_exists.assert_called_once()

    def test_lookup_and_fetch_uncached_legacy(self) -> None:
        bb_json = {
            "output": {"properties": {"rpl_files": ["reproxy_2.718.rrpl"]}}
        }
        with mock.patch.object(
            bbtool.BuildBucketTool,
            "get_rbe_build_info",
            return_value=("8728721", bb_json),
        ) as mock_rbe_build_info:
            with mock.patch.object(
                bbtool,
                "_cache_path_exists",
                return_value=False,
            ) as mock_cache_exists:
                with mock.patch.object(
                    bbtool.BuildBucketTool,
                    "download_reproxy_log",
                    return_value="ignored_contents",
                ) as mock_download:
                    bbtool.fetch_reproxy_log_from_bbid(
                        bbpath=Path("bb"), bbid="b789789"
                    )

        mock_rbe_build_info.assert_called_once()
        mock_cache_exists.assert_called_once()
        mock_download.assert_called_once()

    def test_lookup_and_fetch_cached(self) -> None:
        bb_json = {
            "output": {
                "properties": {
                    "cas_reproxy_log": {
                        "file": "reproxy_6.413.rrpl",
                        "digest": "98a7c9ebf96e0/441",
                        "instance": "projects/PROJECT/instance/INSTANCE",
                    }
                }
            }
        }
        with mock.patch.object(
            bbtool.BuildBucketTool,
            "get_rbe_build_info",
            return_value=("7589112", bb_json),
        ) as mock_rbe_build_info:
            with mock.patch.object(
                bbtool,
                "_cache_path_exists",
                return_value=True,
            ) as mock_cache_exists:
                bbtool.fetch_reproxy_log_from_bbid(
                    bbpath=Path("bb"), bbid="b789789"
                )

        mock_rbe_build_info.assert_called_once()
        mock_cache_exists.assert_called_once()

    def test_lookup_and_fetch_uncached(self) -> None:
        bb_json = {
            "output": {
                "properties": {
                    "cas_reproxy_log": {
                        "file": "reproxy_3.523.rrpl",
                        "digest": "ce0018727cbae0/421",
                        "instance": "projects/PROJECT/instance/INSTANCE",
                    }
                }
            }
        }
        with mock.patch.object(
            bbtool.BuildBucketTool,
            "get_rbe_build_info",
            return_value=("8728721", bb_json),
        ) as mock_rbe_build_info:
            with mock.patch.object(
                bbtool,
                "_cache_path_exists",
                return_value=False,
            ) as mock_cache_exists:
                with mock.patch.object(
                    cas.File,
                    "download",
                    return_value=cl_utils.SubprocessResult(0),
                ) as mock_download:
                    bbtool.fetch_reproxy_log_from_bbid(
                        bbpath=Path("bb"), bbid="b789789"
                    )

        mock_rbe_build_info.assert_called_once()
        mock_cache_exists.assert_called_once()
        mock_download.assert_called_once()

    def test_no_rpl_files(self) -> None:
        bb_json: dict[str, Any] = {"output": {"properties": {}}}
        with mock.patch.object(
            bbtool.BuildBucketTool,
            "get_rbe_build_info",
            return_value=("6728724", bb_json),
        ) as mock_rbe_build_info:
            self.assertIsNone(
                bbtool.fetch_reproxy_log_from_bbid(
                    bbpath=Path("bb"), bbid="b789789"
                )
            )
        mock_rbe_build_info.assert_called_once()


class MainTests(unittest.TestCase):
    def test_e2e_bbid_to_log_only(self) -> None:
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
                self.assertEqual(
                    bbtool.main(["--bbid", bbid, "fetch_reproxy_log"]), 0
                )
        self.assertTrue(
            result.getvalue().endswith(
                f"reproxy log name: {reproxy_log_path}\n"
            )
        )
        mock_fetch_log.assert_called_once_with(
            bbpath=bb, bbid=bbid.lstrip("b"), verbose=False
        )

    def test_e2e_bb_error(self) -> None:
        bbid = "b99669966"
        with mock.patch.object(
            bbtool.BuildBucketTool,
            "get_rbe_build_info",
            side_effect=bbtool.BBError("error"),
        ) as mock_rbe_build_info:
            self.assertEqual(
                bbtool.main(["--bbid", bbid, "fetch_reproxy_log"]), 1
            )
        mock_rbe_build_info.assert_called_once_with(
            bbid.lstrip("b"), verbose=False
        )


if __name__ == "__main__":
    unittest.main()
