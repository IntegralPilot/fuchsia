#!/usr/bin/env python3
#
# Copyright 2025 The Fuchsia Authors
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

from __future__ import annotations

from typing import Collection, Literal, Mapping, TypeGuard, TypeVar, overload

from mobly import signals


class ValidatorError(signals.TestAbortClass):
    pass


class FieldNotFoundError(ValidatorError):
    pass


class FieldTypeError(ValidatorError):
    pass


T = TypeVar("T")


class _NO_DEFAULT:
    pass


class MapValidator:
    def __init__(self, map: Mapping[str, object]) -> None:
        self.map = map

    @overload
    def get(self, type: type[T], key: str, default: None) -> T | None:
        ...

    @overload
    def get(
        self, type: type[T], key: str, default: T | _NO_DEFAULT = _NO_DEFAULT()
    ) -> T:
        ...

    def get(
        self,
        type: type[T],
        key: str,
        default: T | None | _NO_DEFAULT = _NO_DEFAULT(),
    ) -> T | None:
        """Access the map requiring a value type at the specified key.

        If default is set and the map does not contain the specified key, the
        default will be returned.

        Args:
            type: Expected type of the value
            key: Key to index into the map with
            default: Default value when the map does not contain key

        Returns:
            Value of the expected type, or None if default is None.

        Raises:
            FieldNotFound: when default is not set and the map does not contain
                the specified key
            FieldTypeError: when the value at the specified key is not the
                expected type
        """
        if key not in self.map:
            if isinstance(default, type) or default is None:
                return default
            raise FieldNotFoundError(
                f'Required field "{key}" is missing; expected {type.__name__}'
            )
        val = self.map[key]
        if val is None and default is None:
            return None
        if not isinstance(val, type):
            raise FieldTypeError(
                f'Expected "{key}" to be {type.__name__}, got {describe_type(val)}'
            )
        return val

    @overload
    def list(self, key: str) -> ListValidator:
        ...

    @overload
    def list(self, key: str, optional: Literal[False]) -> ListValidator:
        ...

    @overload
    def list(self, key: str, optional: Literal[True]) -> ListValidator | None:
        ...

    def list(self, key: str, optional: bool = False) -> ListValidator | None:
        """Access the map requiring a list at the specified key.

        If optional is True and the map does not contain the specified key, None
        will be returned.

        Args:
            key: Key to index into the map with
            optional: If True, will return None if the map does not contain key

        Returns:
            ListValidator or None if optional is True.

        Raises:
            FieldNotFound: when optional is False and the map does not contain
                the specified key
            FieldTypeError: when the value at the specified key is not a list
        """
        if optional:
            val = self.get(list, key, None)
        else:
            val = self.get(list, key)
        return None if val is None else ListValidator(key, val)


class ListValidator:
    def __init__(self, name: str, val: list[object]) -> None:
        self.name = name
        self.val = val

    def all(self, type: type[T]) -> list[T]:
        """Access the list requiring all elements to be the specified type.

        Args:
            type: Expected type of all elements

        Raises:
            FieldTypeError: when an element is not the expected type
        """
        if not is_list_of(self.val, type):
            raise FieldTypeError(
                f'Expected "{self.name}" to be list[{type.__name__}], '
                f"got {describe_type(self.val)}"
            )
        return self.val


def describe_type(o: object) -> str:
    """Describe the complete type of the object.

    Different from type() by recursing when a mapping or collection is found.
    """
    if isinstance(o, Mapping):
        keys = set([describe_type(k) for k in o.keys()])
        values = set([describe_type(v) for v in o.values()])
        return f'dict[{" | ".join(keys)}, {" | ".join(values)}]'
    if isinstance(o, Collection) and not isinstance(o, str):
        elements = set([describe_type(x) for x in o])
        return f'list[{" | ".join(elements)}]'
    return type(o).__name__


def is_list_of(val: list[object], type: type[T]) -> TypeGuard[list[T]]:
    return all(isinstance(x, type) for x in val)
