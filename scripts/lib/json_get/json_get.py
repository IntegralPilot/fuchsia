# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import collections.abc
import json
import typing
from typing import Iterable

Any = typing.Any


# JsonMatch (or None) will be produced by JsonGet.match().
# It will have attributes added to it: m.a instead of m["a"] as syntactic sugar.
# However, this can't be type-checked (a plain dictionary can't either),
# so type annotations should use type Any rather than JsonMatch to keep MyPy happy.
class JsonMatch:
    def __repr__(self) -> str:
        return f"JsonMatch<{vars(self)}>"


# Create a named singleton object to be used in match patterns,
# along with Any (=typing.Any) and None.
class Maybe:
    pass


class JsonGet:
    def __init__(
        self, json_string: str | None = None, value: Any = None
    ) -> None:
        """Exactly one of json_string or value must be not None.
        If it's json_string, that must be valid JSON."""
        assert (json_string is not None or value is not None) and (
            json_string is None or value is None
        )
        if json_string is not None:
            self.j = json.loads(json_string)
        if value is not None:
            self.j = value

    # if pattern matches the JSON data, calls callback(match) if given. `match` will be a JsonMatch.
    # If pattern doesn't match, calls no_match() if given.
    # Returns the match result.
    def match(
        self,
        pattern: Any,
        callback: collections.abc.Callable[[Any], None] | None = None,
        no_match: collections.abc.Callable[[], None] | None = None,
    ) -> Any:
        """Matches the contained JSON value against a pattern.

        Args:
            pattern: The pattern to match. List and dict patterns can be nested/recursive.
                - dict: Matches a dict. Keys in the pattern are matched against keys in the data.
                    keys not in the pattern are not added to the match object.
                    - `Any`: matches any value and fails if the key is missing.
                    - `Maybe`: matches any value, or a missing key, in which case the key will be
                      added to the JsonMatch with a value of None.
                    - `None`: fails the match if the key is in the root dict.
                    - any other value must match the value fetched from data by the key.
                - list: Extracts matches from a list. The pattern list must have exactly one
                    element, which is the pattern to match against each element of the data.
                - Any: matches any value; requires the key to be present.
                - Maybe: matches any value; a missing key is added with value None.
                - None: the match fails if the key is present.
                - other value: matches that value.
            callback: (optional) A function to call if the pattern matches. It will be called
                with a JsonMatch object.
            no_match: (optional) A function to call if the pattern does not match.

        Returns:
            The JsonMatch object if the pattern matches, or None otherwise.

        Example:
            j = JsonGet('{"a": 1, "b": 2}')
            # Match if the JSON has a key "a" with value 1, and a key "b" with any value.
            # The callback will be called with a JsonMatch object with attribute `a` set to 1 and
            # `b` set to the value of "b".
            j.match({"a": 1, "b": Any}, callback=lambda m: print(m.b)) # prints 2
            # Match if the JSON has a key "a" with value 1, and a key "c" which
            # may be missing or have any value.
            j.match({"a": 1, "c": Maybe}, callback=lambda m: print(m.c)) # prints None
            # Match if the JSON has a key "a" with value 1, and no key "b".
            j.match({"a": 1, "b": None}, no_match=lambda: print("no match")) # prints "no match"
            j = JsonGet('[1, 2, 3]')
            # Match if the JSON is a list.
            j.match([Any], callback=lambda m: print(m)) # prints [1, 2, 3]
            j = JsonGet('[{"t": "a", "n": 1}, {"t": "a", "n": 2}, {"t": "b", "n": {"x": 5}}])
            # Fetch the type "a" entries
            j.match([{"t": "a", "n": Any}], lambda m: print([e.n for e in m])) # prints [1, 2]
            # Recurse into dictionary
            j.match([{"n": {"x": Any}}], lambda m: print(len(m), m[0].b.x)) # prints 1, 5
            # No need to match all the way down
            j.match([{"t": "b", "n": Any}], lambda m: print(m[0].n)) # prints {"x": 5}
        """
        m = self._match_toplevel(pattern, self.j)
        if m is not None and callback:
            callback(m)
        if m is None and no_match:
            no_match()
        return m

    def _match_toplevel(self, pattern: Any, data: Any) -> Any:
        if isinstance(pattern, dict) and isinstance(data, dict):
            return self._match_dict(pattern, data)
        elif isinstance(pattern, list) and isinstance(data, (list, tuple)):
            assert len(pattern) == 1
            return self._match_list(pattern[0], data)
        elif pattern is Any or pattern is Maybe or pattern == data:
            return data
        else:
            return None

    # Returns a list of items of data that match pattern.
    def _match_list(self, pattern: Any, data: Iterable[Any]) -> list[Any]:
        return [
            matched
            for i in data
            if (matched := self._match_toplevel(pattern, i)) is not None
        ]

    # `pattern` is a dict, whose keys are matched to the root dict of `tree`.
    # Values of `pattern`:
    #  - `Any` matches any value and fails if the key is missing.
    #  - `Maybe` matches any value, or a missing key, in which case the key will be
    #      added to the JsonMatch with a value of None.
    #  - `None` fails the match if the key is in the root dict.
    #  - any other value must match the value fetched from data by the key.
    # If match fails, returns None.
    # If match succeeds, returns a JsonMatch with fields corresponding to the filled-in pattern.
    def _match_dict(
        self, pattern: dict[str, Any], tree: dict[str, Any]
    ) -> Any:  # -> JsonMatch | None:
        if not isinstance(tree, dict):
            return None
        o = JsonMatch()
        for k, v in pattern.items():
            if v is None:
                if k in tree:
                    return None
            elif v is Maybe:
                if k in tree:
                    matched = tree[k]
                else:
                    matched = None
            elif k not in tree:
                return None
            elif v == Any:
                matched = tree[k]
            else:
                matched = self._match_toplevel(v, tree[k])
                if matched is None:
                    return None
            setattr(o, k, matched)
        return o
