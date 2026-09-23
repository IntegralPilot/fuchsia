# Copyright 2021 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Python module for creating depfiles that can be read by ninja.

ninja supports depfiles for tracking implicit dependencies that are needed by
future incremental rebuilds:  https://ninja-build.org/manual.html#_depfile

The format of these is a Makefile, with a single output listed (see
https://gn.googlesource.com/gn/+/main/docs/reference.md#var_depfile)

Examples:

The simplest form is a single line for output and each inputs::

    path/to/an/output:  path/to/input_a, path/to/input_b

paths are all relative to the root build dir (GN's root_build_dir)

Ninja also supports a multiline format which uses backslashes as a continuation
character::

    path/to/an/output: \
        path/to/input_a \
        path/to/input_b \

For readability, this is the format that is used by this module.

Basic usage:

>>> dep_file = DepFile("path/to/output")
>>> dep_file.add_input("path/to/input_a")
>>> dep_file.add_input("path/to/input_b")
>>> print(dep_file)
path/to/output: \
    path/to/input_a \
    path/to/input_b \

>>>

By default, paths are made relative to the current working directory, but the
paths can all be rebased (made relative from) a different absolute path:

Assuming that the current working dir is ``/foo/bar``, and the paths to the
output and inputs are relative

>>> dep_file = DepFile("baz/melon/output", rebase="/foo/bar/baz")
>>> dep_file.add_input("baz/input_a")
>>> dep_file.add_input("/foo/bar/baz/input_b")
>>> dep_file.add_input("/foo/bar/monkey/panda/input_c")
>>> print(dep_file)
melon/output: \
    input_a input_b \
    ../monkey/panda/input/_c \

>>>
"""
import os
import shlex
from collections.abc import Iterable
from os import PathLike
from typing import Any, Self, TextIO, Union

FilePath = Union[str, PathLike[Any]]


def escape_path(path: FilePath) -> str:
    return str(path).replace(" ", "\\ ")


class DepFile:
    """A helper class for collecting implicit dependencies and writing them to
    depfiles that ninja can read.

    Each DepFile instance supports collecting the inputs used to create a single
    output file.
    """

    def __init__(
        self, output: FilePath, rebase: None | FilePath = None
    ) -> None:
        if rebase is not None:
            self.rebase_from = rebase
        else:
            self.rebase_from = os.getcwd()
        self.outputs = [self._rebase(output)]
        self.deps: set[FilePath] = set()

    def _rebase(self, path: FilePath) -> FilePath:
        return os.path.relpath(path, start=self.rebase_from)

    def add_output(self, output: str | FilePath) -> None:
        if output not in self.outputs:
            self.outputs.append(output)

    def add_input(self, input: FilePath) -> None:
        """Add an input to the depfile"""
        self.deps.add(self._rebase(input))

    def update(self, other: Self | Iterable[FilePath]) -> None:
        """Add each input to this depfile"""
        # If other is another DepFile, just snag the values from it's internal
        # dict.
        inputs: Iterable[FilePath] = set()
        if isinstance(other, self.__class__):
            inputs = other.deps
        elif isinstance(other, Iterable):
            inputs = other
        else:
            raise TypeError(
                "update() can only accept a DepFile or an iterable of paths"
            )

        # Rebase them all in a bulk operation.
        inputs = [self._rebase(input) for input in inputs]

        # And then update the set of deps.
        for input in inputs:
            self.deps.add(input)

    @classmethod
    def from_deps(
        cls,
        output: FilePath,
        inputs: Iterable[FilePath],
        rebase: None | FilePath = None,
    ) -> "DepFile":
        dep_file = cls(output, rebase)
        dep_file.update(inputs)
        return dep_file

    @classmethod
    def read_from(cls, file: TextIO) -> "DepFile":
        depfile = None
        found_continuation = False

        for line in file.readlines():
            line = line.strip()
            # ignore empty lines
            if not line:
                continue
            # ignore comment lines
            if line.startswith("#"):
                continue
            # We currently don't allow consecutive backslashes in filenames to
            # simplify depfile parsing. Support can be added if use cases come up.
            #
            # Ninja's implementation:
            # https://github.com/ninja-build/ninja/blob/5993141c0977f563de5e064fbbe617f9dc34bb8d/src/depfile_parser.cc#L39
            if r"\\" in line:
                raise ValueError(
                    f'Consecutive backslashes found in depfile line "{line}", this is not supported by action tracer'
                )

            if found_continuation:
                if line.endswith("\\"):
                    line = line[:-1].rstrip()
                else:
                    found_continuation = False
                inputs = shlex.split(line)
                assert depfile is not None
                for input in inputs:
                    depfile.add_input(input)
            elif ":" in line:
                outputs_string, _, inputs_string = line.partition(":")
                outputs = shlex.split(outputs_string)
                if len(outputs) == 0:
                    raise ValueError(
                        "Failed to parse depfile, no outputs found:\n" + line
                    )

                # Create the depfile when we finally have an output:
                if not depfile:
                    depfile = DepFile(outputs[0])
                    outputs = outputs[1:]

                # Add all outputs after the first.
                for output in outputs:
                    depfile.add_output(output)

                inputs_string = inputs_string.strip()
                if inputs_string.endswith("\\"):
                    found_continuation = True
                    inputs_string = inputs_string[:-1].rstrip()
                else:
                    found_continuation = False

                inputs = shlex.split(inputs_string)
                for input in inputs:
                    depfile.add_input(input)
            else:
                raise ValueError(
                    "Found non-empty line without preceding continuation marker.\n"
                    + line
                )

        if depfile:
            return depfile
        else:
            raise ValueError("Depfile was empty")

    def __repr__(self) -> str:
        formatted_outputs = " ".join([escape_path(s) for s in self.outputs])
        if self.deps:
            sorted_deps = sorted(escape_path(d) for d in self.deps)
            input_continuation = " \\\n  "
            return f"{formatted_outputs}: \\\n  {input_continuation.join(sorted_deps)}\n"
        else:
            return f"{formatted_outputs}:\n"

    def write_to(self, file: TextIO) -> None:
        """Write out the depfile contents to the given writeable `file-like` object."""
        file.write(str(self))
