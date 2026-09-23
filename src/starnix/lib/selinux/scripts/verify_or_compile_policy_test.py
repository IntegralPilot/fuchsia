#!/usr/bin/env fuchsia-vendored-python
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import os
import stat
import tempfile
import unittest
from pathlib import Path
from unittest import mock

import verify_or_compile_policy


class VerifyOrCompilePolicyTest(unittest.TestCase):
    def setUp(self) -> None:
        self.temp_dir = tempfile.TemporaryDirectory()
        self.test_dir = Path(self.temp_dir.name)

    def tearDown(self) -> None:
        self.temp_dir.cleanup()

    def _create_fake_checkpolicy(
        self, output_bytes: bytes = b"COMPILED_POLICY"
    ) -> Path:
        """Creates a mock checkpolicy executable that writes binary output to the output file specified by -o or --output."""
        script_path = self.test_dir / "fake_checkpolicy"
        payload_path = self.test_dir / "fake_checkpolicy_payload.bin"
        payload_path.write_bytes(output_bytes)

        script_content = f"""#!/bin/sh
out=""
while [ $# -gt 0 ]; do
    if [ "$1" = "-o" ] || [ "$1" = "--output" ]; then
        out="$2"
        shift 2
    else
        shift
    fi
done
if [ -n "$out" ]; then
    mkdir -p "$(dirname "$out")"
    cp '{payload_path}' "$out"
fi
exit 0
"""
        script_path.write_text(script_content, encoding="utf-8")
        script_path.chmod(
            script_path.stat().st_mode
            | stat.S_IXUSR
            | stat.S_IXGRP
            | stat.S_IXOTH
        )
        return script_path

    def test_compute_policy_hash_single_file(self) -> None:
        source = self.test_dir / "single.conf"
        source.write_text(
            "class file\nallow s0 s1:file read;\n", encoding="utf-8"
        )

        merged, hash1 = verify_or_compile_policy._compute_policy_hash(
            str(source), "deny"
        )
        self.assertEqual(merged, "class file\nallow s0 s1:file read;\n")
        self.assertEqual(len(hash1), 64)

        # Same input yields identical hash
        _, hash2 = verify_or_compile_policy._compute_policy_hash(
            str(source), "deny"
        )
        self.assertEqual(hash1, hash2)

    def test_compute_policy_hash_handle_unknown_difference(self) -> None:
        source = self.test_dir / "single.conf"
        source.write_text("class file\n", encoding="utf-8")

        _, hash_deny = verify_or_compile_policy._compute_policy_hash(
            str(source), "deny"
        )
        _, hash_allow = verify_or_compile_policy._compute_policy_hash(
            str(source), "allow"
        )
        self.assertNotEqual(hash_deny, hash_allow)

    def test_find_checkpolicy_precedence(self) -> None:
        explicit = self.test_dir / "explicit_cp"
        explicit.touch()
        explicit.chmod(0o755)

        env_cp = self.test_dir / "env_cp"
        env_cp.touch()
        env_cp.chmod(0o755)

        # 1. Explicit path takes highest precedence
        self.assertEqual(
            verify_or_compile_policy._find_checkpolicy(str(explicit)),
            str(explicit),
        )

        # 2. CHECKPOLICY_PATH environment variable
        with mock.patch.dict(os.environ, {"CHECKPOLICY_PATH": str(env_cp)}):
            self.assertEqual(
                verify_or_compile_policy._find_checkpolicy(None),
                str(env_cp),
            )

        # 3. None found when no binary exists
        with mock.patch.dict(
            os.environ, {"CHECKPOLICY_PATH": ""}, clear=True
        ), mock.patch("shutil.which", return_value=None):
            self.assertIsNone(verify_or_compile_policy._find_checkpolicy(None))

    def test_write_depfile(self) -> None:
        target = str(self.test_dir / "target.stamp")
        depfile = str(self.test_dir / "target.stamp.d")
        hash_file = self.test_dir / "policy.hash"
        prebuilt = self.test_dir / "policy.bin"
        source = self.test_dir / "source.conf"

        hash_file.touch()
        prebuilt.touch()
        source.touch()

        verify_or_compile_policy._write_depfile(
            depfile,
            target,
            str(hash_file),
            str(prebuilt),
            str(source),
        )

        self.assertTrue(os.path.exists(depfile))
        content = Path(depfile).read_text(encoding="utf-8")
        self.assertTrue(content.startswith(f"{target}: "))
        self.assertIn(str(hash_file), content)
        self.assertIn(str(prebuilt), content)
        self.assertIn(str(source), content)

    def test_fast_path_when_hash_matches_and_prebuilt_exists(self) -> None:
        source = self.test_dir / "policy.conf"
        source.write_text("class file\n", encoding="utf-8")
        _, hash_val = verify_or_compile_policy._compute_policy_hash(
            str(source), "deny"
        )

        hash_file = self.test_dir / "policy.hash"
        hash_file.write_text(hash_val, encoding="utf-8")

        prebuilt = self.test_dir / "policy.bin"
        prebuilt.write_bytes(b"FAST_PATH_PREBUILT")

        output = self.test_dir / "out" / "policy.bin"
        stamp = self.test_dir / "out" / "stamp"
        depfile = self.test_dir / "out" / "stamp.d"

        code = verify_or_compile_policy.main(
            [
                "--policy-name",
                "test_policy",
                "--source",
                str(source),
                "--prebuilt",
                str(prebuilt),
                "--hash-file",
                str(hash_file),
                "--output",
                str(output),
                "--stamp",
                str(stamp),
                "--depfile",
                str(depfile),
            ]
        )

        self.assertEqual(code, 0)
        self.assertEqual(output.read_bytes(), b"FAST_PATH_PREBUILT")
        self.assertTrue(stamp.exists())
        self.assertTrue(depfile.exists())

    def test_missing_checkpolicy_fails_when_hash_mismatched(self) -> None:
        source = self.test_dir / "policy.conf"
        source.write_text("class file modified\n", encoding="utf-8")

        hash_file = self.test_dir / "policy.hash"
        hash_file.write_text("OLD_MISMATCHED_HASH", encoding="utf-8")

        prebuilt = self.test_dir / "policy.bin"
        prebuilt.write_bytes(b"OLD_PREBUILT")

        output = self.test_dir / "out" / "policy.bin"

        with mock.patch.dict(os.environ, {}, clear=True), mock.patch(
            "shutil.which", return_value=None
        ):
            code = verify_or_compile_policy.main(
                [
                    "--policy-name",
                    "test_policy",
                    "--source",
                    str(source),
                    "--prebuilt",
                    str(prebuilt),
                    "--hash-file",
                    str(hash_file),
                    "--output",
                    str(output),
                ]
            )

        self.assertEqual(code, 1)

    def test_bless_updates_prebuilt_and_hash(self) -> None:
        source = self.test_dir / "policy.conf"
        source.write_text("class file new\n", encoding="utf-8")
        _, expected_hash = verify_or_compile_policy._compute_policy_hash(
            str(source), "deny"
        )

        hash_file = self.test_dir / "policy.hash"
        hash_file.write_text("OLD_HASH", encoding="utf-8")

        prebuilt = self.test_dir / "policy.bin"
        prebuilt.write_bytes(b"OLD_PREBUILT")

        output = self.test_dir / "out" / "policy.bin"
        fake_cp = self._create_fake_checkpolicy(
            output_bytes=b"NEW_COMPILED_POLICY"
        )

        code = verify_or_compile_policy.main(
            [
                "--policy-name",
                "test_policy",
                "--source",
                str(source),
                "--prebuilt",
                str(prebuilt),
                "--hash-file",
                str(hash_file),
                "--output",
                str(output),
                "--checkpolicy",
                str(fake_cp),
                "--bless",
            ]
        )

        self.assertEqual(code, 0)
        self.assertEqual(prebuilt.read_bytes(), b"NEW_COMPILED_POLICY")
        self.assertEqual(
            hash_file.read_text(encoding="utf-8").strip(), expected_hash
        )
        self.assertEqual(output.read_bytes(), b"NEW_COMPILED_POLICY")

    def test_verify_fails_when_candidate_differs_from_prebuilt(self) -> None:
        source = self.test_dir / "policy.conf"
        source.write_text("class file changed\n", encoding="utf-8")

        hash_file = self.test_dir / "policy.hash"
        hash_file.write_text("OLD_HASH", encoding="utf-8")

        prebuilt = self.test_dir / "policy.bin"
        prebuilt.write_bytes(b"OLD_PREBUILT")

        output = self.test_dir / "out" / "policy.bin"
        fake_cp = self._create_fake_checkpolicy(
            output_bytes=b"NEW_COMPILED_POLICY"
        )

        code = verify_or_compile_policy.main(
            [
                "--policy-name",
                "test_policy",
                "--source",
                str(source),
                "--prebuilt",
                str(prebuilt),
                "--hash-file",
                str(hash_file),
                "--output",
                str(output),
                "--checkpolicy",
                str(fake_cp),
            ]
        )

        self.assertEqual(code, 1)
        # Candidate and hash were written to out-dir for manual copy
        self.assertTrue(output.exists())
        self.assertTrue(Path(str(output) + ".hash").exists())

    def test_verify_fails_when_prebuilt_does_not_exist(self) -> None:
        source = self.test_dir / "policy.conf"
        source.write_text("class file\n", encoding="utf-8")

        hash_file = self.test_dir / "policy.hash"
        hash_file.write_text("SOME_HASH", encoding="utf-8")

        prebuilt = self.test_dir / "non_existent_policy.bin"
        output = self.test_dir / "out" / "policy.bin"
        fake_cp = self._create_fake_checkpolicy()

        code = verify_or_compile_policy.main(
            [
                "--policy-name",
                "test_policy",
                "--source",
                str(source),
                "--prebuilt",
                str(prebuilt),
                "--hash-file",
                str(hash_file),
                "--output",
                str(output),
                "--checkpolicy",
                str(fake_cp),
            ]
        )

        self.assertEqual(code, 1)


if __name__ == "__main__":
    unittest.main()
