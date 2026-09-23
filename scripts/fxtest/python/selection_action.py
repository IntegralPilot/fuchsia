# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Custom argument actions for argparse."""

import argparse
import typing


class SelectionAction(argparse.Action):
    """Support appending selections to a single list in argparse.

    This action stores any value for the options in the destination.
    If multiple option strings are provided, use the longest as the canonical version.

    Example:
        parser.add_argument('-a', '--and', action=SelectionAction, dest='selection')
        parser.add_argument('selection', action=SelectionAction, dest='selection')
        assert (
            parser.parse_args(['value', '-a', 'other', '--and', 'another']).selection ==
            ['value', '--and', 'other', '--and', 'another']
        )
    """

    def __init__(
        self,
        option_strings: list[str],
        dest: str,
        nargs: int | str | None = None,
        **kwargs: typing.Any,
    ) -> None:
        """Create a SelectionAction.

        Args:
            option_strings (list[str]): List of options. See argparse documentation.
            dest (str): Destination variable. See argparse documentation.
            nargs (Optional[Union[int, str]]): Number of arguments. See argparse documentation.
        """

        self._dest = dest
        if nargs is None:
            nargs = "*"
        super().__init__(list(option_strings), dest, nargs=nargs, **kwargs)

    def __call__(
        self,
        parser: argparse.ArgumentParser,
        namespace: argparse.Namespace,
        values: typing.Sequence[str] | None,
        option_string: str | None = None,
    ) -> None:
        """Call this parser.

        See argparse documentation for details.
        """

        if not values:
            return

        if getattr(namespace, self._dest) is None:
            setattr(namespace, self._dest, [])
        lst: list[str] = getattr(namespace, self._dest)
        lst += SelectionAction._postprocess_args(list(values))

    _COMPONENT_REPLACE = "__COMPONENT_____"
    _PACKAGE_REPLACE = "__PACKAGE_____"
    _AND_REPLACE = "__AND_____"

    @classmethod
    def _postprocess_args(cls, arg_list: list[str]) -> list[str]:
        """Convert args back from stand-in positionals to canonical values.

        See preprocess_args for details.

        Args:
            arg_list (list[str]): Arguments to process.

        Returns:
            list[str]: Processed args, suitable for selection parsing.
        """
        canonical_mapping = {
            cls._COMPONENT_REPLACE: "--component",
            cls._PACKAGE_REPLACE: "--package",
            cls._AND_REPLACE: "--and",
        }
        for i in range(len(arg_list)):
            if arg_list[i] in canonical_mapping:
                arg_list[i] = canonical_mapping[arg_list[i]]
        return arg_list

    @classmethod
    def preprocess_args(cls, arg_list: list[str]) -> list[str]:
        """Handle known selection flags, converting them to positionals.

        Argparse really does not like interleaving positional arguments and
        switches, and when you do it does not preserve the ordering.

        We avoid this situation by converting each switch for which ordering is
        needed (-a, -c, -p) and converting them to a stand-in value which is
        not a switch. This will be passed verbatim as a positional argument,
        and can then be post-processed later to change back to the canonical
        flag names for selection processing.

        Args:
            arg_list (list[str]): Arguments to process.

        Returns:
            list[str]: Processed list, suitable to pass to argparse.
        """
        replace_mapping = {
            "-c": cls._COMPONENT_REPLACE,
            "--component": cls._COMPONENT_REPLACE,
            "-p": cls._PACKAGE_REPLACE,
            "--package": cls._PACKAGE_REPLACE,
            "-a": cls._AND_REPLACE,
            "--and": cls._AND_REPLACE,
        }
        processed_args: list[str] = []
        for i, arg in enumerate(arg_list):
            if arg == "--":
                processed_args.extend(arg_list[i:])
                break
            if "=" in arg:
                flag, val = arg.split("=", 1)
                if flag in replace_mapping:
                    processed_args.extend([replace_mapping[flag], val])
                    continue
            processed_args.append(replace_mapping.get(arg, arg))
        return processed_args


class InvalidAction(argparse.Action):
    """Argparse action that raises an exception if it is ever called.

    We pre-process certain command line flags so they are treated
    as positional arguments for the purpose of having an ordered
    list of selections interleaved with other flags. This action
    is used to catch bugs where we inadvertently allow such an
    argument to be handled by argparse itself.
    """

    def __init__(
        self,
        *args: typing.Any,
        **kwargs: typing.Any,
    ) -> None:
        super().__init__(*args, **kwargs)

    def __call__(
        self,
        parser: argparse.ArgumentParser,
        namespace: argparse.Namespace,
        values: typing.Sequence[str] | None,
        option_string: str | None = None,
    ) -> None:
        """Call this parser.

        See argparse documentation for details.
        """

        raise RuntimeError("This action should not be possible")
