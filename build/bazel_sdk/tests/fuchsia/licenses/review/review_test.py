# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""A tiny example binary for the native Python rules of Bazel."""

import difflib
import io
import os
import unittest
import zipfile


class TestReview(unittest.TestCase):
    def test_zip_file_contents(self):
        goldens_dir = "fuchsia/licenses/review/goldens"

        relative_golden_files = []
        for folder, subs, files in os.walk(goldens_dir):
            for file in files:
                relative_golden_files.append(
                    os.path.relpath(os.path.join(folder, file), goldens_dir)
                )

        input_file = "fuchsia/licenses/review/review.zip"
        with zipfile.ZipFile(input_file, mode="r") as archive:
            actual_files = archive.namelist()
            self.assertEqual(
                sorted(relative_golden_files),
                sorted(archive.namelist()),
                f"The files in {input_file} differ from the files in {goldens_dir}",
            )

            for actual_file_name in actual_files:
                expected_file_name = os.path.join(goldens_dir, actual_file_name)

                with archive.open(actual_file_name, "r") as actual_file:
                    with open(expected_file_name, "r") as expected_file:
                        actual_text = io.TextIOWrapper(actual_file).readlines()
                        expected_text = expected_file.readlines()

                        if actual_text == expected_text:
                            continue
                        diff = difflib.unified_diff(expected_text, actual_text)
                        diff_text = "".join(diff)
                        self.fail(
                            f"""Files {expected_file_name} (golden) and {input_file}:{actual_file_name} (actual) don't match.
If the diff is expected, you may update the golden files via:
```
cp -a \\
   ~/fuchsia/build/bazel_sdk/tests/bazel-bin/fuchsia/licenses/review/review.zip.unzipped/. \\
   ~/fuchsia/build/bazel_sdk/tests/fuchsia/licenses/review/goldens/
```

Here is the diff:
{diff_text}
"""
                        )


if __name__ == "__main__":
    unittest.main()
