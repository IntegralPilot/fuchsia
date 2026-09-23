# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import gc
import resource
import sys
from types import FunctionType, ModuleType
from typing import Any, NamedTuple


class MaxRss(NamedTuple):
    self_bytes: int
    children_bytes: int


# Referents of Function objects include the modules they're in, which isn't
# really the kind of "size" we're trying to get at here. Module and type objects
# have similar issues.
_DENYLIST = type, ModuleType, FunctionType


def get_deep_size(obj: Any) -> int:
    """Traverse obj and its members, summing their sizes."""
    if isinstance(obj, _DENYLIST):
        raise TypeError(
            f"get_deep_size() does not take argument of type: {str(type(obj))}"
        )
    seen_ids: set[int] = set()
    size = 0
    objects = [obj]
    while objects:
        need_referents = []
        for obj in objects:
            if not isinstance(obj, _DENYLIST) and id(obj) not in seen_ids:
                seen_ids.add(id(obj))
                size += sys.getsizeof(obj)
                need_referents.append(obj)
        objects = gc.get_referents(*need_referents)
    return size


def get_max_rss_bytes() -> MaxRss:
    """Returns the maximum resident set size of the current process tree in bytes."""
    return MaxRss(
        self_bytes=_get_current_process_max_rss_kb() * 1024,
        children_bytes=_get_child_processes_max_rss_kb() * 1024,
    )


def _get_current_process_max_rss_kb() -> int:
    """Returns the maximum resident set size of the current process in kilobytes."""
    return resource.getrusage(resource.RUSAGE_SELF).ru_maxrss


def _get_child_processes_max_rss_kb() -> int:
    """
    Returns the maximum resident set size of all terminated child processes
    in kilobytes.
    """
    return resource.getrusage(resource.RUSAGE_CHILDREN).ru_maxrss
