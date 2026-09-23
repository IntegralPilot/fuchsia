# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Utility classes to define shell script commands."""

import argparse
import typing as T


class ScriptCommandBase(object):
    """A base class for all objects modeling a given script command.

    By default, the command name is computed by converting the derived
    class name from CamelCase to snake_case after removing a required
    "Command" suffix.

    Similarly, the command help text is taken from the derived class'
    docstring.

    The command's description text is taken from the derived class'
    DESCRIPTION or DESCRIPTION_RAW definition, if one of them is
    provided. Otherwise the default is to use the command help.

    DESCRIPTION_RAW applies the RawTextHelpFormatter class to the
    description, while DESCRIPTION does not (and let argparse
    reformat the description text). Only one of DESCRIPTION or
    DESCRIPTION_RAW can be set.

    PARSER_KWARGS can be used to provide additional arguments to
    the subparsers.add_parser() call. Any parameter listed in this
    dictionary overrides the values computed by the rules above.

    Derived classes *may* provide their own definition for the
    add_arguments() method, if the command requires its own
    specific arguments.

    Derived classes *must* provide their own definition for the run()
    method, which can be either a static or a regular method. For
    example the two definitions below are functionally equivalent:

    ```
    class ListCommand(ScriptCommandBase):
        "List all available build API module names."

        def run(self, args: argparse.Namespace) -> int:
            ... implement the command here.
            return 0

    class AlternativeListCommand(ScriptCommandBase):
        PARSER_KWARGS = {
          "name": "list",
          "help": "List all available build API module names.",
        }

        def __init__(self, ...) -> None:
           ...

        def run(self, args: argparse.Namespace) -> int:
           ... implement the command here.
    ```

    Note that the methods are always invoked as `command.add_arguments(...)`
    and `command.run(...)` at runtime, so derived classes can define these
    as regular methods instead of static if they need to.
    """

    # Command description. Set DESCRIPTION or DESCRIPTION_RAW to a non-empty
    # string to set the command's description. Default is to use the command's
    # help if none are defined (and no override passed in PARSER_KWARGS).
    # Only one of these can be defined.
    DESCRIPTION: str = ""
    DESCRIPTION_RAW: str = ""

    # CommandFoo.PARSER_KWARGS is a keyword dictionary passed
    # to subparsers.add_parser() to create a new parser object.
    # It should provide at least a "name" and a "help" key.
    PARSER_KWARGS: dict[str, T.Any] = {}

    @staticmethod
    def add_arguments(parser: argparse.ArgumentParser) -> None:
        """Add command-specific arguments to the parser.

        Default implementation does nothing, but derived classes can override
        this method to call parser.add_argument() for their own specific
        needs.
        """

    def run(self, args: argparse.Namespace) -> int:
        """Run the command. Derived classes *must* override this method."""
        raise NotImplementedError
        return 0


class ScriptCommandList(object):
    """A global list of ScriptCommandBase instances. Usage is:

    1) Create instance, passing the main ArgumentParser as argument.

    2) Call add_command() each time a new command needs to be recorded.

    3) Call parser.parse_args() to parse the command-line.

    4) Call run() method instead of args.func(args).
    """

    def __init__(self, parser: argparse.ArgumentParser) -> None:
        """Create instance.

        Args:
            parser: the main argparse.ArgumentParser instance.
        """
        self._parser = parser
        self._subparsers = parser.add_subparsers(
            required=True, help="sub-command help."
        )
        self._parsers: list[argparse.ArgumentParser] = []

    @property
    def parsers(self) -> list[argparse.ArgumentParser]:
        """The list of command parsers created by add_command() calls."""
        return self._parsers

    def add_command(self, command: ScriptCommandBase) -> None:
        """Record a new command.

        If its PARSER_KWARGS does not have a "name" key, the command's name
        will be computed from |command|'s class name.

        If its PARSER_KWARGS does not have a "help" key, the help text
        will be taken from |command|'s class docstring.

        Args:
            command: A ScriptCommandBase derived instance.
        """
        kwargs: dict[str, T.Any] = command.PARSER_KWARGS.copy()
        assert isinstance(kwargs, dict)
        if "name" not in kwargs:
            # Compute name from class name, remove the Command suffix then
            # convert PascalCase into smaller_caps
            #
            # E.g. FooBarCommand -> "foo_bar"
            #
            class_name = type(command).__name__
            pascal_name = class_name.removesuffix("Command")
            assert pascal_name != class_name, (
                f"ScriptCommandBase derived class name ({class_name}) does not end with Command suffix. "
                + 'Please ensure its PARSER_KWARGS value provides a "name" value.'
            )
            import re

            small_caps = re.sub(r"(?<!^)(?=[A-Z])", "_", pascal_name).lower()
            kwargs["name"] = small_caps

        if "help" not in kwargs:
            # Get the description from the class' docstring.
            help = type(command).__doc__
            assert (help is not None) and (help != ScriptCommandBase.__doc__), (
                f"ScriptCommandBase derived class ({class_name}) has no docstring. "
                + 'Please ensure its PARSER_KWARGS value provides a "help" value.'
            )
            kwargs["help"] = help

        if "description" not in kwargs:
            description = command.DESCRIPTION
            description_raw = command.DESCRIPTION_RAW
            if description_raw:
                assert (
                    not description
                ), f"Do not set both DESCRIPTION and DESCRIPTION_RAW in {type(command).__name__} class"
                kwargs["description"] = description_raw.strip()
                kwargs["formatter_class"] = argparse.RawTextHelpFormatter
            elif description:
                kwargs["description"] = description

        cmd_parser = self._subparsers.add_parser(**kwargs)
        command.add_arguments(cmd_parser)

        # Define helper to allow derived classes to implement regular "run()"
        # method.
        def run_command(args: argparse.Namespace) -> int:
            return command.run(args)

        cmd_parser.set_defaults(func=run_command)
        self._parsers.append(cmd_parser)

    def run(
        self, args: argparse.Namespace, keep_exception: bool = False
    ) -> int:
        """Run the appropriate script command function.

        This is similar to calling args.func(args) except that it catches the
        AttributeError raised when the command is missing from the command-line
        invocation by default. See https://bugs.python.org/issue16308.

        Args:
            args: The result of calling parser.parse_args(), which will be
               passed as input to the command-specific run() method.

            keep_exception: Set to True to keep an exception stack trace
              in case of AttributeError exception.

              Default value is False, which just prints "Too few arguments"
              to stderr then abort the program, which unfortunately masks
              other causes for this exception that appear inside the
              run() implementation.

              Setting this to True keeps the full Python exception stack trace
              instead, which is useful for debugging problems. It is
              recommended to enable this for high verbosity levels only.
        """
        try:
            return args.func(args)
        except AttributeError as e:
            # If --verbose --verbose is used, raise the error, as
            # this is useful when debugging this script to catch
            # AttributeError exceptions that are not caused by a
            # missing command.
            if keep_exception:
                raise e

            self._parser.error("Too few arguments.")
            return 1
