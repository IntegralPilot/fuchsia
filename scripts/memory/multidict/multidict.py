# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import collections
from typing import Any, Dict, Iterable, Tuple


def multidict(
    k_v_generator: Iterable[Tuple[Any, Any]], value_container: type = list
) -> Dict[Any, Any]:
    """Builds a dictionary from (key, value) pairs.

    The returned dictionary uses either a list or a set to hold the
    values of a given key.
    """
    assert value_container in (list, set)
    # Giving up on annotating `add`: for MyPy `set.add` and
    # `list.append` are function and not Callable which looks
    # incorrect.
    add: Any = set.add if value_container is set else list.append
    result: Dict[Any, Any] = collections.defaultdict(value_container)
    for k, v in k_v_generator:
        add(result[k], v)
    return result
