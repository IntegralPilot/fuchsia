# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Tests for classification_types.py"""

import unittest

from fuchsia.tools.licenses.classification_types import *


class TestClassificationTypes(unittest.TestCase):
    def test_StringMatcher_to_json(self):
        sm = StringMatcher.create(["foo", "bar"])
        self.assertEqual(sm.to_json(), ["foo", "bar"])

    def test_StringMatcher_exact_matches(self):
        sm = StringMatcher.create(["foo", "bar"])

        self.assertTrue(sm.matches("foo"))
        self.assertTrue(sm.matches("bar"))
        self.assertFalse(sm.matches("baz"))

        self.assertEqual(sm.get_matches(["foo", "bar", "baz"]), ["foo", "bar"])
        self.assertEqual(
            sm.get_matches(["foo", "foo", "foo"]), ["foo", "foo", "foo"]
        )

        self.assertTrue(sm.matches_all(["foo"]))
        self.assertTrue(sm.matches_all(["foo", "bar"]))
        self.assertTrue(sm.matches_all(["foo", "bar", "foo", "bar"]))
        self.assertFalse(sm.matches_all(["foo", "bar", "baz"]))
        self.assertFalse(sm.matches_all(["baz"]))

    def test_AsterixStringExpression_asterix_matches_everything(self):
        exp = AsterixStringExpression.create("*")

        self.assertTrue(exp.matches(""))
        self.assertTrue(exp.matches("foo"))

    def test_AsterixStringExpression_asterix_prefix(self):
        exp = AsterixStringExpression.create("*foo")

        self.assertTrue(exp.matches("foo"))
        self.assertTrue(exp.matches("  foo"))
        self.assertFalse(exp.matches("foo  "))

    def test_AsterixStringExpression_asterix_suffix(self):
        exp = AsterixStringExpression.create("foo*")

        self.assertTrue(exp.matches("foo"))
        self.assertTrue(exp.matches("foo  "))
        self.assertFalse(exp.matches("  foo"))

    def test_AsterixStringExpression_asterix_middle(self):
        exp = AsterixStringExpression.create("fo*o")

        self.assertTrue(exp.matches("foo"))
        self.assertTrue(exp.matches("fo   o"))
        self.assertFalse(exp.matches("fo   o  "))
        self.assertFalse(exp.matches(" fo   o"))

    def test_AsterixStringExpression_mulitple_asterix(self):
        exp = AsterixStringExpression.create("b*a*r")

        self.assertTrue(exp.matches("bar"))
        self.assertTrue(exp.matches("b a r"))
        self.assertTrue(exp.matches("ba r"))
        self.assertTrue(exp.matches("b ar"))
        self.assertFalse(exp.matches(" bar"))
        self.assertFalse(exp.matches("bar "))

        exp = AsterixStringExpression.create("*b*a*r")

        self.assertTrue(exp.matches(" bar"))
        self.assertTrue(exp.matches(" b a r"))
        self.assertFalse(exp.matches(" b a r "))

        exp = AsterixStringExpression.create("b*a*r*")

        self.assertTrue(exp.matches("bar "))
        self.assertTrue(exp.matches("b a r "))
        self.assertFalse(exp.matches(" b a r "))

        exp = AsterixStringExpression.create("*b*a*r*")

        self.assertTrue(exp.matches("b a r"))
        self.assertTrue(exp.matches(" b a r"))
        self.assertTrue(exp.matches("b a r "))
        self.assertTrue(exp.matches(" b a r "))

    def test_StringMatcher_asterix_matches(self):
        sm = StringMatcher.create(["fo*o", "*bar", "baz*", "*w*a*z*"])

        self.assertTrue(sm.matches("foo"))
        self.assertTrue(sm.matches("bar"))
        self.assertTrue(sm.matches("baz"))
        self.assertTrue(sm.matches("waz"))

        self.assertFalse(sm.matches("XYZ"))

        self.assertTrue(sm.matches("foXYZo"))
        self.assertFalse(sm.matches("XYZfoo"))
        self.assertFalse(sm.matches("fooXYZ"))
        self.assertFalse(sm.matches("fXYZoo"))

        self.assertTrue(sm.matches("XYZbar"))
        self.assertFalse(sm.matches("bXYZar"))
        self.assertFalse(sm.matches("barXYZ"))

        self.assertTrue(sm.matches("bazXYZ"))
        self.assertFalse(sm.matches("bXYZaz"))
        self.assertFalse(sm.matches("XYZbaz"))

        self.assertTrue(sm.matches("waz"))
        self.assertTrue(sm.matches("w a z"))
        self.assertTrue(sm.matches(" w a z "))
        self.assertFalse(sm.matches("z a w"))
        self.assertTrue(sm.matches("XwYaZz"))
        self.assertTrue(sm.matches("wXaYzZ"))

    def test_StringMatcher_asterix_matches_greedily(self):
        sm = StringMatcher.create(["ba*r"])

        self.assertTrue(sm.matches("bar"))
        self.assertTrue(sm.matches("baar"))
        self.assertTrue(sm.matches("barr"))
        self.assertTrue(sm.matches("barbarr"))

    def test_select_majority_identifications_clear_majority(self):
        mit_snippet = IdentifiedSnippet(
            identified_as="MIT",
            confidence=1.0,
            start_line=1,
            end_line=20,
            conditions={"notice"},
        )
        unid_snippet = IdentifiedSnippet(
            identified_as=IdentifiedSnippet.UNIDENTIFIED_IDENTIFICATION,
            confidence=0.0,
            start_line=1,
            end_line=20,
            conditions={"unidentified"},
        )

        # 2 MIT runs vs 1 UNIDENTIFIED run -> MIT wins with count 2
        runs = [[unid_snippet], [mit_snippet], [mit_snippet]]
        (
            winning_snippets,
            count,
        ) = LicensesClassifications.select_majority_identifications(runs)
        self.assertEqual(count, 2)
        self.assertEqual(len(winning_snippets), 1)
        self.assertEqual(winning_snippets[0].identified_as, "MIT")

    def test_select_majority_identifications_unidentified_majority(self):
        mit_snippet = IdentifiedSnippet(
            identified_as="MIT",
            confidence=1.0,
            start_line=1,
            end_line=20,
            conditions={"notice"},
        )
        unid_snippet = IdentifiedSnippet(
            identified_as=IdentifiedSnippet.UNIDENTIFIED_IDENTIFICATION,
            confidence=0.0,
            start_line=1,
            end_line=20,
            conditions={"unidentified"},
        )

        # 2 UNIDENTIFIED runs vs 1 MIT run -> UNIDENTIFIED wins with count 2
        runs = [[unid_snippet], [mit_snippet], [unid_snippet]]
        (
            winning_snippets,
            count,
        ) = LicensesClassifications.select_majority_identifications(runs)
        self.assertEqual(count, 2)
        self.assertEqual(len(winning_snippets), 1)
        self.assertEqual(
            winning_snippets[0].identified_as,
            IdentifiedSnippet.UNIDENTIFIED_IDENTIFICATION,
        )

    def test_select_majority_identifications_tie_breaks_toward_identified(self):
        unid_snippet = IdentifiedSnippet(
            identified_as=IdentifiedSnippet.UNIDENTIFIED_IDENTIFICATION,
            confidence=0.0,
            start_line=1,
            end_line=20,
            conditions={"unidentified"},
        )
        bsd_snippet = IdentifiedSnippet(
            identified_as="BSD-3-Clause",
            confidence=0.85,
            start_line=1,
            end_line=20,
            conditions={"notice"},
        )
        mit_snippet = IdentifiedSnippet(
            identified_as="MIT",
            confidence=1.0,
            start_line=1,
            end_line=20,
            conditions={"notice"},
        )

        # 1-1-1 tie: prefers non-[UNIDENTIFIED] with highest confidence (MIT)
        runs = [[unid_snippet], [bsd_snippet], [mit_snippet]]
        (
            winning_snippets,
            count,
        ) = LicensesClassifications.select_majority_identifications(runs)
        self.assertEqual(count, 1)
        self.assertEqual(winning_snippets[0].identified_as, "MIT")

    def test_replace_classifications(self):
        c1 = LicenseClassification(
            license_id="lic1",
            identifications=[
                IdentifiedSnippet(
                    identified_as=IdentifiedSnippet.UNIDENTIFIED_IDENTIFICATION,
                    confidence=0.0,
                    start_line=1,
                    end_line=10,
                    conditions={"unidentified"},
                )
            ],
        )
        c2 = LicenseClassification(
            license_id="lic2",
            identifications=[
                IdentifiedSnippet(
                    identified_as="Apache-2.0",
                    confidence=1.0,
                    start_line=1,
                    end_line=50,
                    conditions={"notice"},
                )
            ],
        )
        classifications = LicensesClassifications(
            classifications_by_id={"lic1": c1, "lic2": c2}
        )

        replacement = LicenseClassification(
            license_id="lic1",
            identifications=[
                IdentifiedSnippet(
                    identified_as="MIT",
                    confidence=1.0,
                    start_line=1,
                    end_line=10,
                    conditions={"notice"},
                )
            ],
        )
        updated = classifications.replace_classifications([replacement])
        self.assertEqual(
            updated.classifications_by_id["lic1"]
            .identifications[0]
            .identified_as,
            "MIT",
        )
        self.assertEqual(
            updated.classifications_by_id["lic2"]
            .identifications[0]
            .identified_as,
            "Apache-2.0",
        )

    def test_retry_failing_files_sequential_majority(self):
        import os
        import stat
        import sys
        import tempfile

        from fuchsia.tools.licenses.generate_licenses_classification import (
            _get_failing_license_files,
            _retry_failing_files,
        )

        with tempfile.TemporaryDirectory() as tmpdir:
            lic_file = os.path.join(tmpdir, "LicenseRef-Flaky.txt")
            with open(lic_file, "w") as f:
                f.write("MIT License text\nLine 2\n")

            # Create a mock identify_license script that fails (empty classifications)
            # on call #1 (attempt 1), and succeeds (MIT) on calls #2 and #3.
            counter_file = os.path.join(tmpdir, "counter.txt")
            mock_bin = os.path.join(tmpdir, "mock_identify_license.py")
            with open(mock_bin, "w") as f:
                f.write(
                    f"""#!{sys.executable}
import json
import os
import sys

counter_path = {repr(counter_file)}
count = 0
if os.path.exists(counter_path):
    with open(counter_path, "r") as cf:
        count = int(cf.read().strip())
count += 1
with open(counter_path, "w") as cf:
    cf.write(str(count))

output_path = None
target_file = sys.argv[-1]
for arg in sys.argv[1:]:
    if arg.startswith("-json="):
        output_path = arg.split("=", 1)[1]

if count == 1:
    # Simulate flake (empty classifications)
    data = [{{"Filepath": target_file, "Classifications": []}}]
else:
    # Simulate successful classification
    data = [{{
        "Filepath": target_file,
        "Classifications": [{{
            "Name": "MIT",
            "Confidence": 1.0,
            "StartLine": 1,
            "EndLine": 2,
            "Condition": "notice"
        }}]
    }}]

with open(output_path, "w") as out:
    json.dump(data, out)
"""
                )
            os.chmod(mock_bin, stat.S_IRWXU)

            unid_snippet = IdentifiedSnippet(
                identified_as=IdentifiedSnippet.UNIDENTIFIED_IDENTIFICATION,
                confidence=0.0,
                start_line=1,
                end_line=2,
                conditions={"unidentified"},
                verified=False,
            )
            initial_classification = LicensesClassifications(
                classifications_by_id={
                    "LicenseRef-Flaky": LicenseClassification(
                        license_id="LicenseRef-Flaky",
                        identifications=[unid_snippet],
                    )
                }
            )
            license_files_by_id = {"LicenseRef-Flaky": lic_file}

            failing = _get_failing_license_files(
                initial_classification, license_files_by_id
            )
            self.assertEqual(failing, [lic_file])

            base_output = os.path.join(tmpdir, "output.json")
            retried = _retry_failing_files(
                raw_classification=initial_classification,
                failing_files=failing,
                license_files_by_id=license_files_by_id,
                identify_license_path=mock_bin,
                identify_license_output_path=base_output,
                num_retries=3,
            )

            snippets = retried.classifications_by_id[
                "LicenseRef-Flaky"
            ].identifications
            self.assertEqual(len(snippets), 1)
            self.assertEqual(snippets[0].identified_as, "MIT")
            self.assertEqual(snippets[0].conditions, {"notice"})


if __name__ == "__main__":
    unittest.main()
