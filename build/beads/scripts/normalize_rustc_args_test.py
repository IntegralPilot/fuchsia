# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import unittest

import normalize_rustc_args
import path_normalizer


class MockPathNormalizer(path_normalizer.PathNormalizer):
    def normalize_path(self, path: str) -> str:
        return path


class TestNormalizeRustcArgs(unittest.TestCase):
    def test_normalize_rustc_arg(self) -> None:
        mock_normalizer = MockPathNormalizer()

        BASIC_TEST_CASES = [
            # Basic args
            ("params.rs", "params.rs"),
            # Flag conversions
            ("--codegen=foo=bar", "-Cfoo=bar"),
            ("--allow=dead_code", "-Adead_code"),
            ("--deny=warnings", "-Dwarnings"),
            ("--warn=unused_imports", "-Wunused_imports"),
            # Ignored args
            ("--extern", ""),
            ("-L", ""),
            ("-Ldependency", ""),
            ("@shell:foo", ""),
            ("--emit=dep-info", ""),
            ("-Zdep-info-omit-d-target", ""),
            ("--error-format=human", ""),
            ("-Cdebug-assertions=y", ""),
            ("-Cdebuginfo=2", ""),
            ("-Cembed-bitcode=no", ""),
            ("-Ccodegen-units=16", ""),
            ("-Cstrip=debuginfo", ""),
            ("-Copt-level=3", ""),
            ("--codegen=opt-level=3", ""),
            ("--cfg=__rust_toolchain=stable", ""),
            ("-Cmetadata=123", ""),
            ("RUST_BACKTRACE=1", ""),
            ("-Clink-arg=-s", ""),
            # Linker args normalization
            ("-Clinker=/path/to/clang", ""),
            ("-Clinker=lld", ""),
        ]
        for arg, expected in BASIC_TEST_CASES:
            self.assertEqual(
                normalize_rustc_args.normalize_rustc_arg(arg, mock_normalizer),
                expected,
                msg=f"For input '{arg}'",
            )


if __name__ == "__main__":
    unittest.main()
