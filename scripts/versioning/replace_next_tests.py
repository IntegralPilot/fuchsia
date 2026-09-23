#!/usr/bin/env fuchsia-vendored-python
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import pathlib
import unittest

import replace_next


class TestSubstitution(unittest.TestCase):
    def assertSubstitution(
        self, path: pathlib.Path, contents: str, expected: str
    ) -> None:
        contents = contents + "\n"
        expected = expected + "\n"
        for p in replace_next.PATTERNS:
            if p.path_matches(path):
                contents = p.substitute(contents, "42")
        self.assertEqual(contents, expected)

    def test_cpp(self) -> None:
        path = pathlib.Path("test.h")
        self.assertSubstitution(
            path,
            "#if FUCHSIA_API_LEVEL_AT_LEAST(NEXT)",
            "#if FUCHSIA_API_LEVEL_AT_LEAST(42)",
        )

        self.assertSubstitution(
            path,
            "#if FUCHSIA_API_LEVEL_AT_LEAST(NEXT) || !defined(__Fuchsia__)",
            "#if FUCHSIA_API_LEVEL_AT_LEAST(42) || !defined(__Fuchsia__)",
        )

        self.assertSubstitution(
            path,
            "#if FUCHSIA_API_LEVEL_AT_LEAST(NEXT) && FUCHSIA_API_LEVEL_LESS_THAN(HEAD)",
            "#if FUCHSIA_API_LEVEL_AT_LEAST(42) && FUCHSIA_API_LEVEL_LESS_THAN(HEAD)",
        )

        self.assertSubstitution(
            path,
            "#if FUCHSIA_API_LEVEL_AT_LEAST(NEXT) && FUCHSIA_API_LEVEL_LESS_THAN(NEXT)",
            "#if FUCHSIA_API_LEVEL_AT_LEAST(42) && FUCHSIA_API_LEVEL_LESS_THAN(42)",
        )

        self.assertSubstitution(
            path,
            "#if FUCHSIA_API_LEVEL_AT_LEAST( NEXT )",
            # TODO(https://fxbug.dev/502591261): Fix behavior and use this line:
            # "#if FUCHSIA_API_LEVEL_AT_LEAST( 42 )",
            "#if FUCHSIA_API_LEVEL_AT_LEAST( NEXT )",
        )

        self.assertSubstitution(
            path, "} ZX_AVAILABLE_SINCE(NEXT)", "} ZX_AVAILABLE_SINCE(42)"
        )

        self.assertSubstitution(
            path,
            """) ZX_REMOVED_SINCE(1, 19,
                                NEXT,
                                "Use DoNewThing() instead")""",
            """) ZX_REMOVED_SINCE(1, 19,
                                42,
                                "Use DoNewThing() instead")""",
        )

        self.assertSubstitution(
            path,
            "ZX_REMOVED_SINCE((1), 19, NEXT)",
            # TODO(https://fxbug.dev/502591261): Fix behavior and use this line:
            # "ZX_REMOVED_SINCE((1), 19, 42)",
            "ZX_REMOVED_SINCE((1), 19, NEXT)",
        )

        self.assertSubstitution(
            path,
            """ZX_REMOVED_SINCE(1, NEXT, NEXT, "Use ProtocolReceive instead");""",
            # TODO(https://fxbug.dev/453685340): Fix behavior and use this line:
            # """ZX_REMOVED_SINCE(1, 42, 42, "Use ProtocolReceive instead");""",
            """ZX_REMOVED_SINCE(1, NEXT, 42, "Use ProtocolReceive instead");""",
        )

        # Test NEVER_REPLACE_NEXT.
        self.assertSubstitution(
            path,
            "#if FUCHSIA_API_LEVEL_AT_LEAST(NEXT) // NEVER_REPLACE_NEXT",
            "#if FUCHSIA_API_LEVEL_AT_LEAST(NEXT) // NEVER_REPLACE_NEXT",
        )
        self.assertSubstitution(
            path,
            """ZX_REMOVED_SINCE(1, 19,
                                NEXT,  // NEVER_REPLACE_NEXT
                                "Use DoNewThing() instead")""",
            """ZX_REMOVED_SINCE(1, 19,
                                NEXT,  // NEVER_REPLACE_NEXT
                                "Use DoNewThing() instead")""",
        )
        self.assertSubstitution(
            path,
            """ZX_REMOVED_SINCE(1, 19,
                                NEXT,
                                "Use DoNewThing() instead")  // NEVER_REPLACE_NEXT""",
            """ZX_REMOVED_SINCE(1, 19,
                                42,
                                "Use DoNewThing() instead")  // NEVER_REPLACE_NEXT""",
        )
        self.assertSubstitution(
            path,
            "#if FUCHSIA_API_LEVEL_AT_LEAST(NEXT)  // :NEVER_REPLACE_NEXT.",
            "#if FUCHSIA_API_LEVEL_AT_LEAST(NEXT)  // :NEVER_REPLACE_NEXT.",
        )
        self.assertSubstitution(
            path,
            "#if FUCHSIA_API_LEVEL_AT_LEAST(NEXT)  // Some text (NEVER REPLACE NEXT)",
            "#if FUCHSIA_API_LEVEL_AT_LEAST(42)  // Some text (NEVER REPLACE NEXT)",
        )
        self.assertSubstitution(
            path,
            "#if FUCHSIA_API_LEVEL_AT_LEAST(NEXT)  // Do this later (NEXT)",
            "#if FUCHSIA_API_LEVEL_AT_LEAST(42)  // Do this later (42)",
        )
        self.assertSubstitution(
            path,
            "#if FUCHSIA_API_LEVEL_AT_LEAST(NEXT)  // NEVER REPLACE NEXT",
            "#if FUCHSIA_API_LEVEL_AT_LEAST(42)  // NEVER REPLACE NEXT",
        )

    def test_rust(self) -> None:
        path = pathlib.Path("test.rs")
        self.assertSubstitution(
            path,
            """cfg(fuchsia_api_level_at_least = "NEXT")""",
            """cfg(fuchsia_api_level_at_least = "42")""",
        )

        self.assertSubstitution(
            path,
            """cfg(all(fuchsia_api_level_at_least = "NEXT", fuchsia_api_level_less_than = "HEAD"))""",
            """cfg(all(fuchsia_api_level_at_least = "42", fuchsia_api_level_less_than = "HEAD"))""",
        )

        # I don't think this is actually an interesting, valid case, but it's mentioned in the pattern definitions...
        self.assertSubstitution(
            path,
            """cfg(all(fuchsia_api_level_at_least = "NEXT", fuchsia_api_level_less_than = "NEXT"))""",
            # TODO(https://fxbug.dev/502591261): Fix behavior and use this line:
            # """cfg(all(fuchsia_api_level_at_least = "42", fuchsia_api_level_less_than = "42"))""",
            """cfg(all(fuchsia_api_level_at_least = "NEXT", fuchsia_api_level_less_than = "42"))""",
        )

        # Test NEVER_REPLACE_NEXT.
        self.assertSubstitution(
            path,
            """cfg(fuchsia_api_level_at_least = "NEXT") // NEVER_REPLACE_NEXT""",
            """cfg(fuchsia_api_level_at_least = "NEXT") // NEVER_REPLACE_NEXT""",
        )
        self.assertSubstitution(
            path,
            """cfg(
                fuchsia_api_level_at_least = "NEXT") // NEVER_REPLACE_NEXT""",
            """cfg(
                fuchsia_api_level_at_least = "NEXT") // NEVER_REPLACE_NEXT""",
        )
        self.assertSubstitution(
            path,
            """cfg(
                fuchsia_api_level_at_least = "NEXT"
                ) // NEVER_REPLACE_NEXT""",
            """cfg(
                fuchsia_api_level_at_least = "42"
                ) // NEVER_REPLACE_NEXT""",
        )
        self.assertSubstitution(
            path,
            """cfg(fuchsia_api_level_at_least = "NEXT") // Some text (NEVER REPLACE NEXT)""",
            """cfg(fuchsia_api_level_at_least = "42") // Some text (NEVER REPLACE NEXT)""",
        )
        self.assertSubstitution(
            path,
            """cfg(fuchsia_api_level_at_least = "NEXT") // Do this later (NEXT)""",
            """cfg(fuchsia_api_level_at_least = "42") // Do this later (NEXT)""",
        )
        self.assertSubstitution(
            path,
            """cfg(fuchsia_api_level_at_least = "NEXT") // NEVER REPLACE NEXT""",
            """cfg(fuchsia_api_level_at_least = "42") // NEVER REPLACE NEXT""",
        )

    def test_fidl(self) -> None:
        path = pathlib.Path("test.fidl")
        self.assertSubstitution(
            path,
            """@available(added=NEXT)""",
            """@available(added=42)""",
        )

        self.assertSubstitution(
            path,
            """@available(added=15, deprecated=22, removed=NEXT)""",
            """@available(added=15, deprecated=22, removed=42)""",
        )

        self.assertSubstitution(
            path,
            """@available(added=15,
                deprecated=22,
                removed=NEXT)""",
            """@available(added=15,
                deprecated=22,
                removed=42)""",
        )

        self.assertSubstitution(
            path,
            """@available(added=15, removed=NEXT, deprecated=22)""",
            """@available(added=15, removed=42, deprecated=22)""",
        )

        self.assertSubstitution(
            path,
            """@available(added=NEXT, deprecated=NEXT)""",
            """@available(added=42, deprecated=42)""",
        )

        # Test NEVER_REPLACE_NEXT.
        self.assertSubstitution(
            path,
            """@available(added=NEXT)  // NEVER_REPLACE_NEXT""",
            """@available(added=NEXT)  // NEVER_REPLACE_NEXT""",
        )
        self.assertSubstitution(
            path,
            """@available(added=15, deprecated=22,
                removed=NEXT)  // NEVER_REPLACE_NEXT""",
            """@available(added=15, deprecated=22,
                removed=NEXT)  // NEVER_REPLACE_NEXT""",
        )
        self.assertSubstitution(
            path,
            """@available(added=15,
                          deprecated=22,
                          removed=NEXT
            )  // NEVER_REPLACE_NEXT""",
            """@available(added=15,
                          deprecated=22,
                          removed=42
            )  // NEVER_REPLACE_NEXT""",
        )
        self.assertSubstitution(
            path,
            """@available(added=NEXT) // Some text (NEVER REPLACE NEXT)""",
            """@available(added=42) // Some text (NEVER REPLACE NEXT)""",
        )
        self.assertSubstitution(
            path,
            """@available(added=NEXT) // Do this later (NEXT)""",
            """@available(added=42) // Do this later (NEXT)""",
        )
        self.assertSubstitution(
            path,
            """@available(added=NEXT) // NEVER REPLACE NEXT""",
            """@available(added=42) // NEVER REPLACE NEXT""",
        )


if __name__ == "__main__":
    unittest.main()
