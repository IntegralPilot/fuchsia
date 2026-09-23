#!/usr/bin/env fuchsia-vendored-python
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Unit tests for android_misc_info.py."""

import json
import pathlib
import tempfile
import unittest

import android_misc_info
import test_utils

# A real misc_info.txt copied from an AOSP build for testing.
_REAL_MISC_INFO_PATH = (
    pathlib.Path("android_misc_info")
    / "misc_info.aosp_cf_arm64_only_phone-userdebug.16102939.txt"
)
_REAL_MISC_INFO_PROPS = {
    "com.android.build.boot.fingerprint": (
        "generic/aosp_cf_arm64_only_phone/vsoc_arm64_only:17/CP2A.260605.016/16102939:userdebug/test-keys"
    ),
    "com.android.build.boot.os_version": "17",
    "com.android.build.boot.security_patch": "2026-06-05",
    "com.android.build.init_boot.fingerprint": (
        "generic/aosp_cf_arm64_only_phone/vsoc_arm64_only:17/CP2A.260605.016/16102939:userdebug/test-keys"
    ),
    "com.android.build.init_boot.os_version": "17",
    "com.android.build.init_boot.security_patch": "2026-06-05",
    "com.android.build.odm.fingerprint": (
        "generic/aosp_cf_arm64_only_phone/vsoc_arm64_only:17/CP2A.260605.016/16102939:userdebug/test-keys"
    ),
    "com.android.build.odm.os_version": "17",
    "com.android.build.odm_dlkm.fingerprint": (
        "generic/aosp_cf_arm64_only_phone/vsoc_arm64_only:17/CP2A.260605.016/16102939:userdebug/test-keys"
    ),
    "com.android.build.odm_dlkm.os_version": "17",
    "com.android.build.product.fingerprint": (
        "generic/aosp_cf_arm64_only_phone/vsoc_arm64_only:17/CP2A.260605.016/16102939:userdebug/test-keys"
    ),
    "com.android.build.product.os_version": "17",
    "com.android.build.product.security_patch": "2026-06-05",
    "com.android.build.recovery.fingerprint": (
        "generic/aosp_cf_arm64_only_phone/vsoc_arm64_only:17/CP2A.260605.016/16102939:userdebug/test-keys"
    ),
    "com.android.build.system.fingerprint": (
        "Android/generic_system/generic:17/CP2A.260605.016/16102939:userdebug/test-keys"
    ),
    "com.android.build.system.os_version": "17",
    "com.android.build.system.security_patch": "2026-06-05",
    "com.android.build.system_dlkm.fingerprint": (
        "generic/aosp_cf_arm64_only_phone/vsoc_arm64_only:17/CP2A.260605.016/16102939:userdebug/test-keys"
    ),
    "com.android.build.system_dlkm.os_version": "17",
    "com.android.build.system_ext.fingerprint": (
        "generic/aosp_cf_arm64_only_phone/vsoc_arm64_only:17/CP2A.260605.016/16102939:userdebug/test-keys"
    ),
    "com.android.build.system_ext.os_version": "17",
    "com.android.build.system_ext.security_patch": "2026-06-05",
    "com.android.build.vendor.fingerprint": (
        "generic/aosp_cf_arm64_only_phone/vsoc_arm64_only:17/CP2A.260605.016/16102939:userdebug/test-keys"
    ),
    "com.android.build.vendor.os_version": "17",
    "com.android.build.vendor.security_patch": "2026-06-05",
    "com.android.build.vendor_boot.fingerprint": (
        "generic/aosp_cf_arm64_only_phone/vsoc_arm64_only:17/CP2A.260605.016/16102939:userdebug/test-keys"
    ),
    "com.android.build.vendor_dlkm.fingerprint": (
        "generic/aosp_cf_arm64_only_phone/vsoc_arm64_only:17/CP2A.260605.016/16102939:userdebug/test-keys"
    ),
    "com.android.build.vendor_dlkm.os_version": "17",
}


def run_main(input: str, flags: list[str] = []) -> str:
    """Runs android_misc_info.main().

    Helps reduce some boilerplate by handling file I/O so that tests can just
    deal with strings.

    Args:
        input: the input misc_info.txt.
        flags: optional flags to pass to the commandline.

    Returns:
        The script output
    """
    with tempfile.TemporaryDirectory() as temp_dir_str:
        temp_dir = pathlib.Path(temp_dir_str)
        input_path = temp_dir / "input"
        output_path = temp_dir / "output"

        input_path.write_text(input)

        android_misc_info.main(
            [str(input_path), "--output", str(output_path)] + flags
        )

        return output_path.read_text()


class AndroidMiscInfoTests(unittest.TestCase):
    def test_single_prop(self) -> None:
        output = run_main("avb_foo_args=--prop foo:bar")
        self.assertEqual(json.loads(output), {"foo": "bar"})

    def test_multiple_props(self) -> None:
        output = run_main("avb_foo_args=--prop foo:bar --prop abc:123")
        self.assertEqual(json.loads(output), {"foo": "bar", "abc": "123"})

    def test_skip_unknown_args(self) -> None:
        output = run_main(
            "avb_foo_args=--arg1 --prop foo:bar --arg2=0 --prop abc:123 --arg3"
        )
        self.assertEqual(json.loads(output), {"foo": "bar", "abc": "123"})

    def test_special_characters(self) -> None:
        output = run_main("avb_foo_args=--prop foo.bar.baz:ABC/123:000.000")
        self.assertEqual(json.loads(output), {"foo.bar.baz": "ABC/123:000.000"})

    def test_extract_props_missing_value_fails(self) -> None:
        with self.assertRaises(ValueError) as ctx:
            run_main("avb_foo_args=--prop invalid_arg_no_colon")
        self.assertIn("Invalid property format", str(ctx.exception))

    def test_extract_props_double_quoting_fails(self) -> None:
        with self.assertRaises(NotImplementedError) as ctx:
            run_main('avb_foo_args=--prop "quotes not supported"')
        self.assertIn("Unsupported meta-char", str(ctx.exception))

    def test_extract_props_single_quoting_fails(self) -> None:
        with self.assertRaises(NotImplementedError) as ctx:
            run_main("avb_foo_args=--prop 'quotes not supported'")
        self.assertIn("Unsupported meta-char", str(ctx.exception))

    def test_extract_props_escape_fails(self) -> None:
        with self.assertRaises(NotImplementedError) as ctx:
            run_main("avb_foo_args=--prop escape\\character")
        self.assertIn("Unsupported meta-char", str(ctx.exception))

    def test_multiple_commandlines(self) -> None:
        output = run_main(
            "avb_foo_args=--prop foo.version:123\n"
            "avb_bar_args=--prop bar.version:456"
        )
        self.assertEqual(
            json.loads(output), {"foo.version": "123", "bar.version": "456"}
        )

    def test_multiple_commandlines_property_overlap_fails(self) -> None:
        with self.assertRaises(ValueError) as ctx:
            run_main(
                "avb_foo_args=--prop same_prop:123\n"
                "avb_bar_args=--prop same_prop:456"
            )
        self.assertIn("Duplicate properties", str(ctx.exception))

    def test_single_prop_filter(self) -> None:
        output = run_main(
            "avb_foo_args=--prop foo:123 --prop bar:456 --prop baz:789",
            flags=["--prop", "foo"],
        )
        # Output should skip everything except `foo`.
        self.assertEqual(json.loads(output), {"foo": "123"})

    def test_multi_prop_filter(self) -> None:
        output = run_main(
            "avb_foo_args=--prop foo:123 --prop bar:456 --prop baz:789",
            flags=["--prop", "foo", "--prop", "baz"],
        )
        # Output should skip everything except `foo` and `baz`.
        self.assertEqual(json.loads(output), {"foo": "123", "baz": "789"})

    def test_prop_rename(self) -> None:
        output = run_main(
            "avb_foo_args=--prop foo:123 --prop bar:456 --prop baz:789",
            flags=["--prop", "foo:new_foo", "--prop", "baz:new_baz"],
        )
        # `foo` and `baz` should be renamed.
        self.assertEqual(
            json.loads(output), {"new_foo": "123", "new_baz": "789"}
        )

    def test_raw_output_single_prop(self) -> None:
        output = run_main(
            "avb_foo_args=--prop foo:123 --prop bar:456 --prop baz:789",
            flags=["--prop", "foo", "--format", "raw_value"],
        )
        # Output should be the raw value of "foo" without any trailing newline so that
        # it can be accurately read by avbtool.
        self.assertEqual(output, "123")

    def test_raw_output_multiple_props_fails(self) -> None:
        with self.assertRaises(ValueError) as ctx:
            run_main(
                "avb_foo_args=--prop foo:123 --prop bar:456",
                flags=["--format", "raw_value"],
            )
        self.assertIn(
            "raw_value is only supported for a single property",
            str(ctx.exception),
        )

    def test_real_misc_info(self) -> None:
        """Tests a real misc_info.txt from an Android build."""
        contents = test_utils.load_test_data(_REAL_MISC_INFO_PATH).decode(
            "utf-8"
        )
        output = run_main(contents)

        self.assertEqual(json.loads(output), _REAL_MISC_INFO_PROPS)


if __name__ == "__main__":
    unittest.main()
