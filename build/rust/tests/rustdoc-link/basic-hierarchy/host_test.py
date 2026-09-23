# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import unittest
from sys import argv
from zipfile import Path

assert (
    len(argv) > 0
), "host_test.py expects to be passed path to generated + linked docs"
_linked_doc_dir = argv.pop()


class Test(unittest.TestCase):
    _linked_doc_dir: Path

    @classmethod
    def setUpClass(cls) -> None:
        cls._linked_doc_dir = Path(_linked_doc_dir)

    def assert_is_dir(self, path: str) -> None:
        where = self._linked_doc_dir / path
        self.assertTrue(
            where.is_dir(),
            f"expected {where} to be a directory in generated docs",
        )

    def assert_is_file(self, path: str) -> None:
        where = self._linked_doc_dir / path
        self.assertTrue(
            where.is_file(), f"expected {where} to be a file in generated docs"
        )

    def assert_doesnt_exist(self, path: str) -> None:
        where = self._linked_doc_dir / path
        self.assertFalse(
            where.exists(), f"did not expect to find {where} in generated docs"
        )

    def test_host_and_fuchsia_docs_are_separate(self) -> None:
        self.assert_is_dir("b")
        self.assert_is_dir("c")
        self.assert_doesnt_exist("d")
        self.assert_is_dir("host/d")

    def test_fuchsia_docs_have_source_files(self) -> None:
        self.assert_is_file("src/b/lib.rs.html")

    def test_host_docs_have_source_files(self) -> None:
        self.assert_is_file("host/src/d/lib.rs.html")

    def test_fuchsia_docs_contain_correct_items(self) -> None:
        self.assert_is_file("b/index.html")
        self.assert_is_file("b/struct.RequiredB.html")
        self.assert_doesnt_exist("b/fn.blah.html")

    def test_host_docs_contain_correct_items(self) -> None:
        self.assert_is_file("host/d/index.html")
        self.assert_is_file("host/d/fn.blah.html")
        self.assert_doesnt_exist("host/d/struct.RequiredB.html")
