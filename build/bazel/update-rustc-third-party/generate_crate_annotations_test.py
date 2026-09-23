#!/usr/bin/env python3
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Unit tests for generate_crate_annotations.py."""

import os
import sys
import unittest
from typing import Any

sys.path.insert(0, os.path.dirname(__file__))

from generate_crate_annotations import (
    BazelCrateAnnotation,
    CrateConfig,
    CrateSelect,
    build_crate_annotations,
    is_gn_specific_env_var,
    merge_annotations,
    parse_env_vars,
    render_starlark,
    validate_annotation_overwrites,
)


class GenerateBazelCrateAnnotationsTest(unittest.TestCase):
    def test_is_gn_specific_env_var(self) -> None:
        self.assertTrue(is_gn_specific_env_var("OUT_DIR", "compat/dir"))
        self.assertTrue(
            is_gn_specific_env_var("CARGO_MANIFEST_DIR", "/some/path")
        )
        self.assertTrue(
            is_gn_specific_env_var("CUSTOM", "${gn_source_root}/path")
        )
        self.assertFalse(is_gn_specific_env_var("CARGO_PKG_NAME", "foo"))
        self.assertFalse(is_gn_specific_env_var("CUSTOM", "normal_value"))

    def test_parse_env_vars(self) -> None:
        env_list = [
            "FOO=bar",
            "BAZ=qux=123",
            "CARGO_MANIFEST_DIR=${gn_source_root}/some/path",
            "OUT_DIR=../../compat/some/path",
            "CUSTOM_VAR=${gn_source_root}/other/path",
        ]
        res = parse_env_vars(env_list)
        self.assertEqual(res, {"FOO": "bar", "BAZ": "qux=123"})

        with self.assertRaises(ValueError):
            parse_env_vars(["NO_EQUALS"])

    def test_build_crate_annotations(self) -> None:
        gn_packages = {
            "test-crate": {
                "1.0.0": CrateConfig(
                    rustflags=["--cfg=feature_common"],
                    env_vars={"KEY": "VALUE"},
                    platform_configs={
                        'cfg(target_os = "linux")': ["--cfg=linux_flag"],
                        'cfg(target_os = "freebsd")': ["--cfg=freebsd_flag"],
                        'cfg(any(target_arch = "x86_64", target_arch = "aarch64"))': [
                            "--cfg=arch64_flag"
                        ],
                    },
                )
            },
            "empty-crate": {
                "0.1.0": CrateConfig(
                    rustflags=[],
                    env_vars={},
                    platform_configs={},
                )
            },
        }

        annotations = build_crate_annotations(gn_packages)
        self.assertIn("test-crate", annotations)
        self.assertNotIn("empty-crate", annotations)

        ann = annotations["test-crate"][0]
        self.assertEqual(ann.version, "1.0.0")
        rustc_flags = ann.attributes["rustc_flags"]
        assert isinstance(rustc_flags, CrateSelect)
        self.assertEqual(
            rustc_flags.common,
            ["--cfg=feature_common", "--cfg=arch64_flag"],
        )
        self.assertEqual(
            rustc_flags.selects,
            {
                "@platforms//os:linux": ["--cfg=linux_flag"],
                "@platforms//os:freebsd": ["--cfg=freebsd_flag"],
            },
        )
        self.assertEqual(ann.attributes["rustc_env"], {"KEY": "VALUE"})

    def test_merge_annotations(self) -> None:
        gn_anns = {
            "crate": [
                BazelCrateAnnotation(
                    version="1.0.0", attributes={"rustc_flags": ["--cfg=gn"]}
                ),
                BazelCrateAnnotation(version="1.1.0", attributes={}),
            ]
        }
        ann_overwrites = {
            "crate": [
                BazelCrateAnnotation(
                    version="*",
                    attributes={
                        "rustc_env": {"GLOBAL": "1"},
                        "rustc_flags": ["--cfg=overwrite_all"],
                    },
                ),
                BazelCrateAnnotation(
                    version="1.0.0",
                    attributes={"rustc_flags": ["--cfg=overwrite_specific"]},
                ),
            ],
            "new-crate": [
                BazelCrateAnnotation(version="*", attributes={}),
            ],
        }

        merged = merge_annotations(gn_anns, ann_overwrites)
        self.assertIn("crate", merged)
        self.assertIn("new-crate", merged)

        crate_anns = {a.version: a for a in merged["crate"]}

        # 1.0.0 gets specific override for flags, wildcard override for env
        self.assertEqual(
            crate_anns["1.0.0"].attributes["rustc_flags"],
            ["--cfg=overwrite_specific"],
        )
        self.assertEqual(
            crate_anns["1.0.0"].attributes["rustc_env"], {"GLOBAL": "1"}
        )

        # 1.1.0 gets wildcard override for both
        self.assertEqual(
            crate_anns["1.1.0"].attributes["rustc_flags"],
            ["--cfg=overwrite_all"],
        )
        self.assertEqual(
            crate_anns["1.1.0"].attributes["rustc_env"], {"GLOBAL": "1"}
        )

    def test_render_starlark(self) -> None:
        annotations = {
            "foo": [
                BazelCrateAnnotation(
                    version="1.2.3",
                    attributes={
                        "rustc_flags": CrateSelect(
                            common=["--cfg=foo_cfg"],
                            selects={
                                "@platforms//os:linux": ["--cfg=linux_only"],
                            },
                        ),
                        "rustc_env": {"FOO_ENV": "1"},
                    },
                )
            ]
        }
        starlark = render_starlark(annotations)
        self.assertNotIn("_GENERATED_CRATE_ANNOTATIONS = {", starlark)
        self.assertIn("CRATE_ANNOTATIONS = {", starlark)
        self.assertNotIn("_merge_annotations", starlark)
        self.assertIn('"foo": [', starlark)
        self.assertIn('version = "1.2.3"', starlark)
        self.assertIn("rustc_flags = crate.select(", starlark)
        self.assertIn("'@platforms//os:linux': [", starlark)
        self.assertIn("'--cfg=linux_only'", starlark)
        self.assertIn("'FOO_ENV': '1'", starlark)

    def test_validate_annotation_overwrites(self) -> None:
        annotation_overwrites = {
            "valid-exact": [
                BazelCrateAnnotation(version="1.0.0", attributes={})
            ],
            "valid-wildcard": [
                BazelCrateAnnotation(version="*", attributes={})
            ],
            "stale-crate": [
                BazelCrateAnnotation(version="0.9.0", attributes={})
            ],
        }
        active_versions: dict[str, dict[str, Any]] = {
            "valid-exact": {"1.0.0": {}},
            "valid-wildcard": {"2.0.0": {}},
            "stale-crate": {"1.0.0": {}},
        }

        errors = validate_annotation_overwrites(
            annotation_overwrites, active_versions
        )
        self.assertEqual(len(errors), 1)
        self.assertIn(
            "Crate 'stale-crate' specifies stale version '0.9.0'", errors[0]
        )
        self.assertIn("Active version(s) in Cargo.toml: [1.0.0]", errors[0])


if __name__ == "__main__":
    unittest.main()
