# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Utility module for different type of property decorators in Honeydew."""

import functools
from collections.abc import Callable
from functools import cached_property
from typing import Any, TypeVar

R = TypeVar("R")


class DynamicProperty(property):
    """A property that is dynamic and involves a device query to return."""

    def __init__(
        self,
        fget: Callable[[Any], Any],
        fset: Callable[[Any, Any], None] | None = None,
        fdel: Callable[[Any], None] | None = None,
        doc: str | None = None,
    ) -> None:
        if not doc:
            doc = fget.__doc__
        super().__init__(fget, fset=fset, fdel=fdel, doc=doc)
        self.name: str = fget.__name__


class PersistentProperty(cached_property[R]):
    """A property that is persistent throughout device interaction.

    Value is queried only once and cached.
    """

    def __init__(self, func: Callable[[Any], R]) -> None:
        super().__init__(func)
        self.name: str = func.__name__


class Affordance(cached_property[R]):
    """A property that represents an affordance."""

    def __init__(self, func: Callable[[Any], R]) -> None:
        super().__init__(func)
        self.name: str = func.__name__


class Transport(cached_property[R]):
    """A property that represents a transport."""

    def __init__(self, func: Callable[[Any], R]) -> None:
        super().__init__(func)
        self.name: str = func.__name__


def persistent_method(func: Callable[..., Any]) -> Callable[..., Any]:
    """An async method decorator that is persistent throughout device interaction.

    Value is queried only once and cached on the instance.
    """
    cache_name = f"_{func.__name__}_async_cached_value"

    @functools.wraps(func)
    async def wrapper(self: Any, *args: Any, **kwargs: Any) -> Any:
        if not hasattr(self, cache_name):
            setattr(self, cache_name, await func(self, *args, **kwargs))
        return getattr(self, cache_name)

    return wrapper
