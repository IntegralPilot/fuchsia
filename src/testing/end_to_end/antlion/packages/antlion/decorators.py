#!/usr/bin/env python3
#
# Copyright 2025 The Fuchsia Authors
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

from __future__ import annotations

import typing
from threading import RLock
from typing import Callable, Generic, TypeVar

S = TypeVar("S")
T = TypeVar("T")


_NOT_FOUND = object()


class cached_property(Generic[T, S]):  # pylint: disable=invalid-name
    """A property whose value is computed then cached; deleter can be overridden.

    Similar to functools.cached_property(), with the addition of deleter function that
    can be overridden to provide custom clean up. The deleter function doesn't throw an
    AttributeError if the value doesn't already exist.

    Useful for properties that are tied to the lifetime of a device and need to be
    recomputed upon reboot of said device.

    Example:

    ```
    class LinuxDevice:
        @cached_property
        def ssh(self) -> SSH:
            return SSH(self.ip)

        @ssh.deleter
        def ssh(self, ssh: SSH) -> None:
            ssh.terminate_connections()
    ```
    """

    def __init__(
        self,
        func: Callable[[S], T],
        deleter: Callable[[S, T], None] | None = None,
    ) -> None:
        self.func = func
        self._deleter = deleter
        self.name: str | None = None
        self.__doc__ = func.__doc__
        self.lock = RLock()

    def __set_name__(self, owner: object, name: str) -> None:
        if self.name is None:
            self.name = name
        elif name != self.name:
            raise TypeError(
                "Cannot assign the same cached_property to two different names "
                f"({self.name!r} and {name!r})."
            )

    def _cache(self, instance: S) -> dict[str, object]:
        if self.name is None:
            raise TypeError(
                "Cannot use cached_property instance without calling __set_name__ on it."
            )
        try:
            return instance.__dict__
        except (
            AttributeError
        ):  # not all objects have __dict__ (e.g. class defines slots)
            msg = (
                f"No '__dict__' attribute on {type(instance).__name__!r} "
                f"instance to cache {self.name!r} property."
            )
            raise TypeError(msg) from None

    def __get__(self, instance: S, owner: object | None = None) -> T:
        cache = self._cache(instance)
        assert self.name is not None
        val = cache.get(self.name, _NOT_FOUND)
        if val is _NOT_FOUND:
            with self.lock:
                # check if another thread filled cache while we awaited lock
                val = cache.get(self.name, _NOT_FOUND)
                if val is _NOT_FOUND:
                    val = self.func(instance)
                    try:
                        cache[self.name] = val
                    except TypeError:
                        msg = (
                            f"The '__dict__' attribute on {type(instance).__name__!r} instance "
                            f"does not support item assignment for caching {self.name!r} property."
                        )
                        raise TypeError(msg) from None
                    return val
        return typing.cast(T, val)

    def __delete__(self, instance: S) -> None:
        cache = self._cache(instance)
        assert self.name is not None
        with self.lock:
            val = cache.pop(self.name, _NOT_FOUND)
            if val is _NOT_FOUND:
                return
            if self._deleter:
                self._deleter(instance, typing.cast(T, val))

    def deleter(self, deleter: Callable[[S, T], None]) -> cached_property[T, S]:
        self._deleter = deleter
        prop = type(self)(self.func, deleter)
        prop.name = self.name
        prop.__doc__ = self.__doc__
        prop.lock = self.lock
        return prop
