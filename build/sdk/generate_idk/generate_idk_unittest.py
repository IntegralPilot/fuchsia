# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import pathlib
import tempfile
import typing
import unittest
from pathlib import Path

import generate_idk

# Access to type hints via get_type_hints is well-defined and sound,
# resolving string annotations that arise from 'from __future__ import annotations'.
UNMERGEABLE_TYPES = typing.get_args(
    typing.get_type_hints(generate_idk.UnmergeableMeta)["type"]
)


class GenerateIdkTests(unittest.TestCase):
    def test_unmergeables_error(self) -> None:
        for atom_type in UNMERGEABLE_TYPES:
            meta_a: generate_idk.UnmergeableMeta = {
                "name": "fallback",
                "type": atom_type,
                "stable": True,
            }
            a: generate_idk.PartialIDK = generate_idk.PartialIDK(
                manifest_src=Path("thingy/meta/manifest.json"),
                atoms={
                    Path("foo/bar.json"): generate_idk.PartialAtom(
                        meta_src=pathlib.Path("src/foo/bar.json"),
                        meta=meta_a,
                        dest_to_src={},
                    )
                },
            )
            meta_b: generate_idk.UnmergeableMeta = {
                "name": "different_fallback",
                "type": atom_type,
                "stable": True,
            }
            b: generate_idk.PartialIDK = generate_idk.PartialIDK(
                manifest_src=Path("thingy/meta/manifest.json"),
                atoms={
                    Path("foo/bar.json"): generate_idk.PartialAtom(
                        meta_src=pathlib.Path("src/foo/bar.json"),
                        meta=meta_b,
                        dest_to_src={},
                    )
                },
            )

            with self.assertRaises(generate_idk.AtomMergeError) as e:
                generate_idk.MergedIDK().merge_with(a).merge_with(b)
            self.assertIn(
                "Key 'name' does not match", str(e.exception.__cause__)
            )

    def test_unmergeables_pass(self) -> None:
        for atom_type in UNMERGEABLE_TYPES:
            meta_a1: generate_idk.UnmergeableMeta = {
                "name": "fallback",
                "type": atom_type,
                "stable": True,
            }
            meta_a2: generate_idk.UnmergeableMeta = {
                "name": "fallback",
                "type": "cc_source_library",
                "stable": True,
            }
            a: generate_idk.PartialIDK = generate_idk.PartialIDK(
                manifest_src=Path("thingy/meta/manifest.json"),
                atoms={
                    Path("foo/bar.json"): generate_idk.PartialAtom(
                        meta_src=pathlib.Path("src/foo/bar.json"),
                        meta=meta_a1,
                        dest_to_src={},
                    ),
                    Path("foo/qux.json"): generate_idk.PartialAtom(
                        meta_src=pathlib.Path("src/foo/qux.json"),
                        meta=meta_a2,
                        dest_to_src={},
                    ),
                },
            )
            meta_b1: generate_idk.UnmergeableMeta = {
                "name": "fallback",
                "type": atom_type,
                "stable": True,
            }
            meta_b2: generate_idk.UnmergeableMeta = {
                "name": "fallback",
                "type": atom_type,
                "stable": True,
            }
            b: generate_idk.PartialIDK = generate_idk.PartialIDK(
                manifest_src=Path("thingy/meta/manifest.json"),
                atoms={
                    Path("foo/bar.json"): generate_idk.PartialAtom(
                        meta_src=pathlib.Path("src/foo/bar.json"),
                        meta=meta_b1,
                        dest_to_src={},
                    ),
                    Path("foo/baz.json"): generate_idk.PartialAtom(
                        meta_src=pathlib.Path("src/foo/baz.json"),
                        meta=meta_b2,
                        dest_to_src={},
                    ),
                },
            )

            expected_meta_1: generate_idk.UnmergeableMeta = {
                "name": "fallback",
                "type": atom_type,
                "stable": True,
            }
            expected_meta_2: generate_idk.UnmergeableMeta = {
                "name": "fallback",
                "type": atom_type,
                "stable": True,
            }
            expected_meta_3: generate_idk.UnmergeableMeta = {
                "name": "fallback",
                "type": "cc_source_library",
                "stable": True,
            }
            self.assertEqual(
                generate_idk.MergedIDK().merge_with(a).merge_with(b),
                generate_idk.MergedIDK(
                    atoms={
                        Path("foo/bar.json"): expected_meta_1,
                        Path("foo/baz.json"): expected_meta_2,
                        Path("foo/qux.json"): expected_meta_3,
                    },
                    dest_to_src={},
                ),
            )

    def test_merge_cc_prebuilt_library_pass(self) -> None:
        meta_a: generate_idk.CCPrebuiltLibraryMeta = {
            "name": "fallback",
            "type": "cc_prebuilt_library",
            "binaries": {"a": "a"},
            "variants": [{"constraints": "a"}],
            "stable": True,
        }
        a = generate_idk.PartialIDK(
            manifest_src=Path("thingy/meta/manifest.json"),
            atoms={
                Path("atom0.json"): generate_idk.PartialAtom(
                    meta_src=pathlib.Path("src/atom0.json"),
                    meta=meta_a,
                    dest_to_src={},
                ),
            },
        )
        meta_b: generate_idk.CCPrebuiltLibraryMeta = {
            "name": "fallback",
            "type": "cc_prebuilt_library",
            "binaries": {"b": "b", "c": "c"},
            "variants": [{"constraints": "b"}, {"constraints": "c"}],
            "stable": True,
        }
        b = generate_idk.PartialIDK(
            manifest_src=Path("thingy/meta/manifest.json"),
            atoms={
                Path("atom0.json"): generate_idk.PartialAtom(
                    meta_src=pathlib.Path("src/atom0.json"),
                    meta=meta_b,
                    dest_to_src={},
                ),
            },
        )

        expected_meta: generate_idk.CCPrebuiltLibraryMeta = {
            "name": "fallback",
            "type": "cc_prebuilt_library",
            "binaries": {"a": "a", "b": "b", "c": "c"},
            "variants": [
                {"constraints": "a"},
                {"constraints": "b"},
                {"constraints": "c"},
            ],
            "stable": True,
        }
        self.assertEqual(
            generate_idk.MergedIDK().merge_with(a).merge_with(b),
            generate_idk.MergedIDK(
                atoms={
                    Path("atom0.json"): expected_meta,
                },
                dest_to_src={},
            ),
        )

    def test_merge_package_pass(self) -> None:
        meta_a: generate_idk.PackageMeta = {
            "name": "fallback",
            "type": "package",
            "variants": [
                {"api_level": 12, "arch": "x64"},
                {"api_level": 13, "arch": "x64"},
            ],
            "stable": True,
        }
        a = generate_idk.PartialIDK(
            manifest_src=Path("thingy/meta/manifest.json"),
            atoms={
                Path("atom0.json"): generate_idk.PartialAtom(
                    meta_src=pathlib.Path("src/atom0.json"),
                    meta=meta_a,
                    dest_to_src={},
                ),
            },
        )
        meta_b: generate_idk.PackageMeta = {
            "name": "fallback",
            "type": "package",
            "variants": [
                {"api_level": 13, "arch": "arm64"},
                {"api_level": 14, "arch": "arm64"},
            ],
            "stable": True,
        }
        b = generate_idk.PartialIDK(
            manifest_src=Path("thingy/meta/manifest.json"),
            atoms={
                Path("atom0.json"): generate_idk.PartialAtom(
                    meta_src=pathlib.Path("src/atom0.json"),
                    meta=meta_b,
                    dest_to_src={},
                ),
            },
        )

        expected_meta: generate_idk.PackageMeta = {
            "name": "fallback",
            "type": "package",
            "variants": [
                {"api_level": 12, "arch": "x64"},
                {"api_level": 13, "arch": "x64"},
                {"api_level": 13, "arch": "arm64"},
                {"api_level": 14, "arch": "arm64"},
            ],
            "stable": True,
        }
        self.assertEqual(
            generate_idk.MergedIDK().merge_with(a).merge_with(b),
            generate_idk.MergedIDK(
                atoms={
                    Path("atom0.json"): expected_meta,
                },
                dest_to_src={},
            ),
        )

    def test_merge_loadable_module_pass(self) -> None:
        meta_a: generate_idk.LoadableModuleMeta = {
            "name": "fallback",
            "type": "loadable_module",
            "binaries": {"a": "a"},
            "stable": True,
        }
        a = generate_idk.PartialIDK(
            manifest_src=Path("thingy/meta/manifest.json"),
            atoms={
                Path("atom0.json"): generate_idk.PartialAtom(
                    meta_src=pathlib.Path("src/atom0.json"),
                    meta=meta_a,
                    dest_to_src={},
                ),
            },
        )
        meta_b: generate_idk.LoadableModuleMeta = {
            "name": "fallback",
            "type": "loadable_module",
            "binaries": {"b": "b", "c": "c"},
            "stable": True,
        }
        b = generate_idk.PartialIDK(
            manifest_src=Path("thingy/meta/manifest.json"),
            atoms={
                Path("atom0.json"): generate_idk.PartialAtom(
                    meta_src=pathlib.Path("src/atom0.json"),
                    meta=meta_b,
                    dest_to_src={},
                ),
            },
        )

        expected_meta: generate_idk.LoadableModuleMeta = {
            "name": "fallback",
            "type": "loadable_module",
            "binaries": {"a": "a", "b": "b", "c": "c"},
            "stable": True,
        }
        self.assertEqual(
            generate_idk.MergedIDK().merge_with(a).merge_with(b),
            generate_idk.MergedIDK(
                atoms={
                    Path("atom0.json"): expected_meta,
                },
                dest_to_src={},
            ),
        )

    def test_merge_sysroot_pass(self) -> None:
        meta_a: generate_idk.SysrootMeta = {
            "name": "fallback",
            "type": "sysroot",
            "versions": {"a": "a"},
            "variants": [{"constraints": "a"}],
            "stable": True,
        }
        a = generate_idk.PartialIDK(
            manifest_src=Path("thingy/meta/manifest.json"),
            atoms={
                Path("atom0.json"): generate_idk.PartialAtom(
                    meta_src=pathlib.Path("src/atom0.json"),
                    meta=meta_a,
                    dest_to_src={},
                ),
            },
        )
        meta_b: generate_idk.SysrootMeta = {
            "name": "fallback",
            "type": "sysroot",
            "versions": {"b": "b", "c": "c"},
            "variants": [{"constraints": "b"}, {"constraints": "c"}],
            "stable": True,
        }
        b = generate_idk.PartialIDK(
            manifest_src=Path("thingy/meta/manifest.json"),
            atoms={
                Path("atom0.json"): generate_idk.PartialAtom(
                    meta_src=pathlib.Path("src/atom0.json"),
                    meta=meta_b,
                    dest_to_src={},
                ),
            },
        )

        expected_meta: generate_idk.SysrootMeta = {
            "name": "fallback",
            "type": "sysroot",
            "versions": {"a": "a", "b": "b", "c": "c"},
            "variants": [
                {"constraints": "a"},
                {"constraints": "b"},
                {"constraints": "c"},
            ],
            "stable": True,
        }
        self.assertEqual(
            generate_idk.MergedIDK().merge_with(a).merge_with(b),
            generate_idk.MergedIDK(
                atoms={
                    Path("atom0.json"): expected_meta,
                },
                dest_to_src={},
            ),
        )

    def test_merge_variants_cc_prebuilt_library_pass(self) -> None:
        meta_a: generate_idk.CCPrebuiltLibraryMeta = {
            "name": "fallback",
            "type": "cc_prebuilt_library",
            "binaries": {},
            "variants": [{"constraints": "a"}],
            "stable": True,
        }
        a = generate_idk.PartialIDK(
            manifest_src=Path("thingy/meta/manifest.json"),
            atoms={
                Path("atom0.json"): generate_idk.PartialAtom(
                    meta_src=pathlib.Path("src/atom0.json"),
                    meta=meta_a,
                    dest_to_src={},
                ),
            },
        )
        meta_b: generate_idk.CCPrebuiltLibraryMeta = {
            "name": "fallback",
            "type": "cc_prebuilt_library",
            "binaries": {},
            "variants": [{"constraints": "b"}, {"constraints": "c"}],
            "stable": True,
        }
        b = generate_idk.PartialIDK(
            manifest_src=Path("thingy/meta/manifest.json"),
            atoms={
                Path("atom0.json"): generate_idk.PartialAtom(
                    meta_src=pathlib.Path("src/atom0.json"),
                    meta=meta_b,
                    dest_to_src={},
                ),
            },
        )

        expected_meta: generate_idk.CCPrebuiltLibraryMeta = {
            "name": "fallback",
            "type": "cc_prebuilt_library",
            "binaries": {},
            "variants": [
                {"constraints": "a"},
                {"constraints": "b"},
                {"constraints": "c"},
            ],
            "stable": True,
        }
        self.assertEqual(
            generate_idk.MergedIDK().merge_with(a).merge_with(b),
            generate_idk.MergedIDK(
                atoms={
                    Path("atom0.json"): expected_meta,
                },
                dest_to_src={},
            ),
        )

    def test_merge_binaries_cc_prebuilt_library_fail(self) -> None:
        meta_a: generate_idk.CCPrebuiltLibraryMeta = {
            "name": "fallback",
            "type": "cc_prebuilt_library",
            "binaries": {"a": "a"},
            "variants": [],
            "stable": True,
        }
        a = generate_idk.PartialIDK(
            manifest_src=Path("thingy/meta/manifest.json"),
            atoms={
                Path("atom0.json"): generate_idk.PartialAtom(
                    meta_src=pathlib.Path("src/atom0.json"),
                    meta=meta_a,
                    dest_to_src={},
                ),
            },
        )
        meta_b: generate_idk.CCPrebuiltLibraryMeta = {
            "name": "fallback",
            "type": "cc_prebuilt_library",
            "binaries": {"a": "a", "c": "c"},
            "variants": [],
            "stable": True,
        }
        b = generate_idk.PartialIDK(
            manifest_src=Path("thingy/meta/manifest.json"),
            atoms={
                Path("atom0.json"): generate_idk.PartialAtom(
                    meta_src=pathlib.Path("src/atom0.json"),
                    meta=meta_b,
                    dest_to_src={},
                ),
            },
        )

        with self.assertRaises(generate_idk.AtomMergeError) as e:
            generate_idk.MergedIDK().merge_with(a).merge_with(b)
        self.assertIn("overlapping keys", str(e.exception.__cause__))

    def test_merge_binaries_loadable_module_fail(self) -> None:
        meta_a: generate_idk.LoadableModuleMeta = {
            "name": "fallback",
            "type": "loadable_module",
            "binaries": {"a": "a"},
            "stable": True,
        }
        a = generate_idk.PartialIDK(
            manifest_src=Path("thingy/meta/manifest.json"),
            atoms={
                Path("atom0.json"): generate_idk.PartialAtom(
                    meta_src=pathlib.Path("src/atom0.json"),
                    meta=meta_a,
                    dest_to_src={},
                ),
            },
        )
        meta_b: generate_idk.LoadableModuleMeta = {
            "name": "fallback",
            "type": "loadable_module",
            "binaries": {"a": "a", "c": "c"},
            "stable": True,
        }
        b = generate_idk.PartialIDK(
            manifest_src=Path("thingy/meta/manifest.json"),
            atoms={
                Path("atom0.json"): generate_idk.PartialAtom(
                    meta_src=pathlib.Path("src/atom0.json"),
                    meta=meta_b,
                    dest_to_src={},
                ),
            },
        )

        with self.assertRaises(generate_idk.AtomMergeError) as e:
            generate_idk.MergedIDK().merge_with(a).merge_with(b)
        self.assertIn("overlapping keys", str(e.exception.__cause__))

    def test_merge_versions_fail(self) -> None:
        meta_a: generate_idk.SysrootMeta = {
            "name": "sysroot",
            "type": "sysroot",
            "versions": {
                "a": "a",
            },
            "variants": [],
            "stable": True,
        }
        a = generate_idk.PartialIDK(
            manifest_src=Path("thingy/meta/manifest.json"),
            atoms={
                Path("atom0.json"): generate_idk.PartialAtom(
                    meta_src=pathlib.Path("src/atom0.json"),
                    meta=meta_a,
                    dest_to_src={},
                ),
            },
        )
        meta_b: generate_idk.SysrootMeta = {
            "name": "sysroot",
            "type": "sysroot",
            "versions": {
                "a": "a",
                "c": "c",
            },
            "variants": [],
            "stable": True,
        }
        b = generate_idk.PartialIDK(
            manifest_src=Path("thingy/meta/manifest.json"),
            atoms={
                Path("atom0.json"): generate_idk.PartialAtom(
                    meta_src=pathlib.Path("src/atom0.json"),
                    meta=meta_b,
                    dest_to_src={},
                ),
            },
        )

        with self.assertRaises(generate_idk.AtomMergeError) as e:
            generate_idk.MergedIDK().merge_with(a).merge_with(b)
        self.assertIn("overlapping keys", str(e.exception.__cause__))

    def test_merge_variants_cc_prebuilt_library_fail(self) -> None:
        meta_a: generate_idk.CCPrebuiltLibraryMeta = {
            "name": "fallback",
            "type": "cc_prebuilt_library",
            "binaries": {},
            "variants": [{"constraints": "a"}],
            "stable": True,
        }
        a = generate_idk.PartialIDK(
            manifest_src=Path("thingy/meta/manifest.json"),
            atoms={
                Path("atom0.json"): generate_idk.PartialAtom(
                    meta_src=pathlib.Path("src/atom0.json"),
                    meta=meta_a,
                    dest_to_src={},
                ),
            },
        )
        meta_b: generate_idk.CCPrebuiltLibraryMeta = {
            "name": "fallback",
            "type": "cc_prebuilt_library",
            "binaries": {},
            "variants": [{"constraints": "a"}, {"constraints": "c"}],
            "stable": True,
        }
        b = generate_idk.PartialIDK(
            manifest_src=Path("thingy/meta/manifest.json"),
            atoms={
                Path("atom0.json"): generate_idk.PartialAtom(
                    meta_src=pathlib.Path("src/atom0.json"),
                    meta=meta_b,
                    dest_to_src={},
                ),
            },
        )

        with self.assertRaises(generate_idk.AtomMergeError) as e:
            generate_idk.MergedIDK().merge_with(a).merge_with(b)
        self.assertIn("duplicate variants", str(e.exception.__cause__))

    def test_merge_variants_sysroot_fail(self) -> None:
        meta_a: generate_idk.SysrootMeta = {
            "name": "fallback",
            "type": "sysroot",
            "versions": {},
            "variants": [{"constraints": "a"}],
            "stable": True,
        }
        a = generate_idk.PartialIDK(
            manifest_src=Path("thingy/meta/manifest.json"),
            atoms={
                Path("atom0.json"): generate_idk.PartialAtom(
                    meta_src=pathlib.Path("src/atom0.json"),
                    meta=meta_a,
                    dest_to_src={},
                ),
            },
        )
        meta_b: generate_idk.SysrootMeta = {
            "name": "fallback",
            "type": "sysroot",
            "versions": {},
            "variants": [{"constraints": "a"}, {"constraints": "c"}],
            "stable": True,
        }
        b = generate_idk.PartialIDK(
            manifest_src=Path("thingy/meta/manifest.json"),
            atoms={
                Path("atom0.json"): generate_idk.PartialAtom(
                    meta_src=pathlib.Path("src/atom0.json"),
                    meta=meta_b,
                    dest_to_src={},
                ),
            },
        )

        with self.assertRaises(generate_idk.AtomMergeError) as e:
            generate_idk.MergedIDK().merge_with(a).merge_with(b)
        self.assertIn("duplicate variants", str(e.exception.__cause__))

    def test_merge_variants_package_fail(self) -> None:
        meta_a: generate_idk.PackageMeta = {
            "name": "fallback",
            "type": "package",
            "variants": [{"api_level": 12, "arch": "x64"}],
            "stable": True,
        }
        a = generate_idk.PartialIDK(
            manifest_src=Path("thingy/meta/manifest.json"),
            atoms={
                Path("atom0.json"): generate_idk.PartialAtom(
                    meta_src=pathlib.Path("src/atom0.json"),
                    meta=meta_a,
                    dest_to_src={},
                ),
            },
        )
        meta_b: generate_idk.PackageMeta = {
            "name": "fallback",
            "type": "package",
            "variants": [
                {"api_level": 12, "arch": "x64"},
                {"api_level": 12, "arch": "arm64"},
            ],
            "stable": True,
        }
        b = generate_idk.PartialIDK(
            manifest_src=Path("thingy/meta/manifest.json"),
            atoms={
                Path("atom0.json"): generate_idk.PartialAtom(
                    meta_src=pathlib.Path("src/atom0.json"),
                    meta=meta_b,
                    dest_to_src={},
                ),
            },
        )

        with self.assertRaises(generate_idk.AtomMergeError) as e:
            generate_idk.MergedIDK().merge_with(a).merge_with(b)
        self.assertIn("duplicate variants", str(e.exception.__cause__))

    def test_merge_files(self) -> None:
        meta_a: generate_idk.UnmergeableMeta = {
            "name": "fallback",
            "type": "cc_source_library",
            "stable": True,
        }
        a: generate_idk.PartialIDK = generate_idk.PartialIDK(
            manifest_src=Path("thingy/meta/manifest.json"),
            atoms={
                Path("foo/bar.json"): generate_idk.PartialAtom(
                    meta_src=pathlib.Path("src/foo/bar.json"),
                    meta=meta_a,
                    dest_to_src={
                        pathlib.Path("dest/foo"): pathlib.Path("src/foo"),
                        pathlib.Path("dest/bar"): pathlib.Path("src/bar"),
                    },
                )
            },
        )
        meta_b: generate_idk.UnmergeableMeta = {
            "name": "fallback",
            "type": "cc_source_library",
            "stable": True,
        }
        b: generate_idk.PartialIDK = generate_idk.PartialIDK(
            manifest_src=Path("thingy/meta/manifest.json"),
            atoms={
                Path("foo/bar.json"): generate_idk.PartialAtom(
                    meta_src=pathlib.Path("src/foo/bar.json"),
                    meta=meta_b,
                    dest_to_src={
                        pathlib.Path("dest/foo"): pathlib.Path("src/foo"),
                        pathlib.Path("dest/baz"): pathlib.Path("src/baz"),
                    },
                )
            },
        )
        print(generate_idk.MergedIDK().merge_with(a))
        print(generate_idk.MergedIDK().merge_with(a).merge_with(b))

        expected_meta: generate_idk.UnmergeableMeta = {
            "name": "fallback",
            "type": "cc_source_library",
            "stable": True,
        }
        self.assertEqual(
            generate_idk.MergedIDK().merge_with(a).merge_with(b),
            generate_idk.MergedIDK(
                atoms={
                    Path("foo/bar.json"): expected_meta,
                },
                dest_to_src={
                    pathlib.Path("dest/foo"): pathlib.Path("src/foo"),
                    pathlib.Path("dest/bar"): pathlib.Path("src/bar"),
                    pathlib.Path("dest/baz"): pathlib.Path("src/baz"),
                },
            ),
        )

    def test_merge_files_check_equality_pass(self) -> None:
        with tempfile.TemporaryDirectory() as dir:
            d = pathlib.Path(dir)

            file_a = d / "a.txt"
            file_b = d / "b.txt"

            file_a.write_text("foo")
            file_b.write_text("foo")

            meta_a: generate_idk.UnmergeableMeta = {
                "name": "fallback",
                "type": "cc_source_library",
                "stable": True,
            }
            a = generate_idk.PartialIDK(
                manifest_src=Path("thingy/meta/manifest.json"),
                atoms={
                    Path("foo/bar.json"): generate_idk.PartialAtom(
                        meta_src=pathlib.Path("src/foo/bar.json"),
                        meta=meta_a,
                        dest_to_src={
                            pathlib.Path("dest/foo"): file_a,
                        },
                    ),
                },
            )
            meta_b: generate_idk.UnmergeableMeta = {
                "name": "fallback",
                "type": "cc_source_library",
                "stable": True,
            }
            b = generate_idk.PartialIDK(
                manifest_src=Path("thingy/meta/manifest.json"),
                atoms={
                    Path("foo/bar.json"): generate_idk.PartialAtom(
                        meta_src=pathlib.Path("src/foo/bar.json"),
                        meta=meta_b,
                        dest_to_src={
                            pathlib.Path("dest/foo"): file_b,
                        },
                    ),
                },
            )
            expected_meta: generate_idk.UnmergeableMeta = {
                "name": "fallback",
                "type": "cc_source_library",
                "stable": True,
            }
            self.assertEqual(
                generate_idk.MergedIDK().merge_with(a).merge_with(b),
                generate_idk.MergedIDK(
                    atoms={
                        Path("foo/bar.json"): expected_meta,
                    },
                    dest_to_src={
                        pathlib.Path("dest/foo"): file_a,
                    },
                ),
            )

    def test_merge_files_check_equality_fail(self) -> None:
        with tempfile.TemporaryDirectory() as dir:
            d = pathlib.Path(dir)

            file_a = d / "a.txt"
            file_b = d / "b.txt"

            file_a.write_text("foo")
            file_b.write_text("bar")

            meta_a: generate_idk.UnmergeableMeta = {
                "name": "fallback",
                "type": "cc_source_library",
                "stable": True,
            }
            a = generate_idk.PartialIDK(
                manifest_src=Path("thingy/meta/manifest.json"),
                atoms={
                    Path("foo/bar.json"): generate_idk.PartialAtom(
                        meta_src=pathlib.Path("src/foo/bar.json"),
                        meta=meta_a,
                        dest_to_src={
                            pathlib.Path("dest/foo"): file_a,
                        },
                    ),
                },
            )
            meta_b: generate_idk.UnmergeableMeta = {
                "name": "fallback",
                "type": "cc_source_library",
                "stable": True,
            }
            b = generate_idk.PartialIDK(
                manifest_src=Path("thingy/meta/manifest.json"),
                atoms={
                    Path("foo/bar.json"): generate_idk.PartialAtom(
                        meta_src=pathlib.Path("src/foo/bar.json"),
                        meta=meta_b,
                        dest_to_src={
                            pathlib.Path("dest/foo"): file_b,
                        },
                    ),
                },
            )

            with self.assertRaises(AssertionError) as e:
                generate_idk.MergedIDK().merge_with(a).merge_with(b)
            self.assertIn("Multiple non-identical files", str(e.exception))


if __name__ == "__main__":
    unittest.main()
