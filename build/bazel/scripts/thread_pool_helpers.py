#!/usr/bin/env python3
# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import typing as T
from concurrent.futures import ThreadPoolExecutor

U = T.TypeVar("U")
V = T.TypeVar("V")


def filter_threaded(
    predicate: T.Callable[[U], bool], values: T.Iterable[U]
) -> list[U]:
    """An version of filter() that runs the filter operations in a threadpool

    This asynchronously calls the predicate function for each value, and then returns the filtered
    list.
    """

    def _wrapper(
        value: U,
    ) -> tuple[U, bool]:
        return value, predicate(value)

    with ThreadPoolExecutor() as pool:
        results = pool.map(_wrapper, values)
    return [value for value, result in results if result]


def map_threaded(
    func: T.Callable[..., V], values: T.Iterable[T.Any]
) -> T.Iterable[V]:
    """A version of map() that executes the operations using a threadpool.

    This uses a threadpool to asynchronously call the given function on the given values,
    returning the results after all calls have completed.
    """
    with ThreadPoolExecutor() as pool:
        return pool.map(func, values)


def starmap_threaded(
    func: T.Callable[..., V], args_list: T.Iterable[T.Iterable[T.Any]]
) -> T.Iterable[V]:
    """An version of map() that executes the operations using a threadpool, for functions that take multiple arguments.

    This uses a threadpool to asynchronously call the given function on the given values,
    returning the results after all calls have completed.
    """

    def _wrapper(args: T.Iterable[T.Any]) -> V:
        return func(*args)

    with ThreadPoolExecutor() as pool:
        return pool.map(_wrapper, args_list)
