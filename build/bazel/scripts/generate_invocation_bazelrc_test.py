#!/usr/bin/env fuchsia-vendored-python
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Unit-tests for generate_invocation_bazelrc.py functions."""

import os
import sys
import unittest

# Import regenerator.py as a module.
_SCRIPT_DIR = os.path.dirname(__file__)
sys.path.insert(0, _SCRIPT_DIR)
import generate_invocation_bazelrc as gen


class BBIDLinkTest(unittest.TestCase):
    def test_led_link(self):
        link = gen.bbid_link("infra/led/01234")
        self.assertEqual(
            link, "http://go/lucibuild/infra/led/01234/+/build.proto"
        )

    def test_regular_link(self):
        link = gen.bbid_link("87654321")
        self.assertEqual(link, "http://go/bbid/87654321")


class MetadataOptionTest(unittest.TestCase):
    def test_key_value(self):
        opt = gen.metadata_option("FOOD", "bbq")
        self.assertEqual(opt, "--build_metadata=FOOD=bbq")


class BuildConfigOptionTest(unittest.TestCase):
    def test_single_option(self):
        rc = gen.build_config_option("feature_x", "--bazel-flag=value")
        self.assertEqual(rc, "build:feature_x --bazel-flag=value")


class MetadataBazelrcTest(unittest.TestCase):
    def test_no_id(self):
        self.assertEqual(list(gen.metadata_bazelrc(dict())), [])

    def test_uuid_top_build(self):
        uuid = "uuid-6767"
        rc = set(gen.metadata_bazelrc({"FX_BUILD_UUID": uuid}))
        self.assertEqual(
            rc,
            {
                f"build:_bes_common --build_metadata=FX_BUILD_UUID={uuid}",
            },
        )

    def test_uuid_sub_build(self):
        uuid = "uuid-5678"
        parent_id = "dada"
        parent_link = f"go/to/{parent_id}"
        siblings_link = f"go/search_for/?q=PARENT={parent_id}"
        rc = set(
            gen.metadata_bazelrc(
                {
                    "FX_BUILD_UUID": uuid,
                    "RESULTSTORE_PARENT_BUILD_ID": parent_id,
                    "RESULTSTORE_PARENT_BUILD_LINK": parent_link,
                    "RESULTSTORE_SIBLING_BUILDS_LINK": siblings_link,
                }
            )
        )
        self.assertEqual(
            rc,
            {
                f"build:_bes_common --build_metadata=FX_BUILD_UUID={uuid}",
                f"build:_bes_common --build_metadata=PARENT_BUILD_ID={parent_id}",
                f"build:_bes_common --build_metadata=PARENT_BUILD_LINK={parent_link}",
                f"build:_bes_common --build_metadata=SIBLING_BUILDS_LINK={siblings_link}",
            },
        )

    def test_bbid_top_build(self):
        bbid = "9988"
        rc = set(gen.metadata_bazelrc({"BUILDBUCKET_ID": bbid}))
        self.assertEqual(
            rc,
            {
                f"build:_bes_common --build_metadata=BUILDBUCKET_ID={bbid}",
                f"build:_bes_common --build_metadata=PARENT_BUILD_ID={bbid}",
                f"build:_bes_common --build_metadata=PARENT_BUILD_LINK=http://go/bbid/{bbid}",
            },
        )

    def test_bbid_sub_build(self):
        bbid = "8877"
        parent_id = "baba"
        parent_link = f"go/to/{parent_id}"
        siblings_link = f"go/search_for/?q=PARENT={parent_id}"
        rc = set(
            gen.metadata_bazelrc(
                {
                    "BUILDBUCKET_ID": bbid,
                    "RESULTSTORE_PARENT_BUILD_ID": parent_id,
                    "RESULTSTORE_PARENT_BUILD_LINK": parent_link,
                    "RESULTSTORE_SIBLING_BUILDS_LINK": siblings_link,
                }
            )
        )
        self.assertEqual(
            rc,
            {
                f"build:_bes_common --build_metadata=BUILDBUCKET_ID={bbid}",
                f"build:_bes_common --build_metadata=PARENT_BUILD_ID={parent_id}",
                f"build:_bes_common --build_metadata=PARENT_BUILD_LINK={parent_link}",
                f"build:_bes_common --build_metadata=SIBLING_BUILDS_LINK={siblings_link}",
            },
        )


if __name__ == "__main__":
    unittest.main()
