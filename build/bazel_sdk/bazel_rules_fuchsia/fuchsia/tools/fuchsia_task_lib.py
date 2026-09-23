#!/usr/bin/env python3
# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import argparse
import json
import os
import sys
from abc import abstractmethod
from contextlib import contextmanager
from enum import Enum
from functools import cached_property, reduce, total_ordering
from pathlib import Path
from typing import Any


class TaskExecutionException(Exception):
    def __init__(self, *args: object, is_caught_failure: bool = False) -> None:
        super().__init__(*args)
        self._is_caught_failure = is_caught_failure

    @property
    def is_caught_failure(self) -> bool:
        return self._is_caught_failure


class Terminal:
    def if_no_color(text: str, if_colored: str = "") -> str:
        return if_colored if Terminal.supports_color() else text

    def bold(text: str) -> str:
        return Terminal._style(text, 1)

    def underline(text: str) -> str:
        return Terminal._style(text, 4)

    def red(text: str) -> str:
        return Terminal._style(text, 91)

    def green(text: str) -> str:
        return Terminal._style(text, 92)

    def yellow(text: str) -> str:
        return Terminal._style(text, 93)

    def purple(text: str) -> str:
        return Terminal._style(text, 95)

    def cyan(text: str) -> str:
        return Terminal._style(text, 96)

    def info(printable: Any, prefix: str = "Info") -> str:
        return f"{Terminal.bold(f'{prefix}:')} {printable}"

    def warn(printable: Any, prefix: str = "Warning") -> str:
        return f"{Terminal.yellow(f'{prefix}:')} {printable}"

    def error(printable: Any, prefix: str = "Error") -> str:
        return f"{Terminal.red(f'{prefix}:')} {printable}"

    def supports_color() -> bool:
        return (
            sys.stdout.isatty()
            and sys.stderr.isatty()
            and not os.environ.get("NO_COLOR")
        )

    def _style(text: str, escape_code: int) -> str:
        if Terminal.supports_color():
            return f"\033[{escape_code}m{text}\033[0m"
        else:
            # If neither stdout nor stderr is not a tty then any styles likely
            # won't get rendered correctly when the text is eventually printed,
            # so don't apply the style.
            return text


@total_ordering
class ArgumentScope(tuple, Enum):
    # Captures arguments that are passed directly to the current task:
    # 1. via command line: `bazel run :workflow -- 'TASK_MNEMONIC=--foo --bar'`
    # 2. via build rule: `arguments = ["--foo", "--bar"]`
    EXPLICIT = ()
    # Captures arguments that are passed to any parent workflow:
    # 1. via command line: `bazel run :workflow -- 'WORKFLOW_MNEMONIC=--foo --bar'`
    # 2. via build rule: `arguments = ["--foo", "--bar"]`
    # 3. Includes any EXPLICIT arguments.
    WORKFLOW = (*EXPLICIT, "__WORKFLOW_ARGUMENT__")
    # Captures any top level arguments:
    # 1. via command line: `bazel run :workflow -- --foo --bar`
    # 2. Includes any WORKFLOW and EXPLICIT arguments.
    GLOBAL = (*WORKFLOW, "__GLOBAL_ARGUMENT__")
    # Captures private arguments intended for internal use.
    META = ("__META_ARGUMENT__",)
    # Captures GLOBAL and META arguments.
    ALL = (*GLOBAL, *META)

    def __lt__(self, other: "ArgumentScope") -> bool:
        return len(self.value) < len(other.value)

    def __eq__(self, other: "ArgumentScope") -> bool:
        return self.value == other.value

    def __hash__(self) -> Any:
        return hash(self.value)


class ScopedArgumentParser:
    @classmethod
    def get_arguments(cls, scope: ArgumentScope) -> list[str]:
        return [
            arg
            for i, arg in list(enumerate(sys.argv))[1:]
            if sys.argv[i] not in ArgumentScope.ALL.value
            and (
                sys.argv[i - 1] not in ArgumentScope.ALL.value
                or sys.argv[i - 1] in scope.value
            )
        ]

    def get_default_arguments(self) -> list[str]:
        return self.get_arguments(self.default_argument_scope)

    @cached_property
    def default_argument_scope(self) -> ArgumentScope:
        return ArgumentScope[self.parse_args().default_argument_scope.upper()]

    def __init__(self, *argparse_args: Any, **argparse_kwargs: Any) -> None:
        self._scoped_parsers = {}
        self._argparse_init_args = argparse_args
        self._argparse_init_kwargs = argparse_kwargs
        self.add_argument(
            "--default_argument_scope",
            help=(
                "The default scope of arguments to use for this task. "
                "See the ArgumentScope class for additional information."
            ),
            scope=ArgumentScope.META,
            choices=["explicit", "workflow", "global"],
            default="explicit",
        )

    def _get_parser(self, scope: ArgumentScope) -> argparse.ArgumentParser:
        if scope not in self._scoped_parsers:
            self._scoped_parsers[scope] = argparse.ArgumentParser(
                *self._argparse_init_args,
                add_help=False,
                **self._argparse_init_kwargs,
            )
        return self._scoped_parsers[scope]

    def add_argument(
        self,
        *argparse_args: Any,
        scope: ArgumentScope = None,
        **argparse_kwargs: Any,
    ) -> Any:
        return self._get_parser(
            self.default_argument_scope if scope is None else scope
        ).add_argument(*argparse_args, **argparse_kwargs)

    def parse_args(
        self, *argparse_args: Any, **argparse_kwargs: Any
    ) -> argparse.Namespace:
        # TODO(chandarren): Handle `--help`.
        return argparse.Namespace(
            **reduce(
                lambda smaller_ns, larger_ns: {**vars(larger_ns), **smaller_ns},
                [
                    parser.parse_known_args(
                        *argparse_args,
                        args=self.get_arguments(scope),
                        **argparse_kwargs,
                    )[0]
                    for scope, parser in sorted(self._scoped_parsers.items())
                ],
                {},
            )
        )

    def parse_known_args(
        self, *argparse_args: Any, **argparse_kwargs: Any
    ) -> argparse.Namespace:
        return (
            self.parse_args(*argparse_args, **argparse_kwargs),
            self._scoped_parsers[ArgumentScope.GLOBAL].parse_known_args(
                *argparse_args,
                args=self.get_arguments(ArgumentScope.GLOBAL),
                **argparse_kwargs,
            )[1],
        )

    def path_arg(self, type="file"):
        def arg(path):
            path = Path(path)
            if path.is_file() != (type == "file") or path.is_dir() != (
                type == "directory"
            ):
                super(self).error(f'Path "{path}" is not a {type}!')
            return path

        return arg


class FuchsiaTask:
    @classmethod
    def read_workflow_state(cls, file: Path | None = None) -> dict[str, Any]:
        workflow_state = {
            "environment_variables": {},
            "workflow": {
                "halt_execution": False,
            },
        }
        workflow_state.update(json.loads(file.read_text()) if file else {})
        return workflow_state

    def __init__(
        self,
        *,
        task_name: str,
        is_final_task: bool,
        workflow_state: dict[str, Any],
    ) -> None:
        self._task_name = task_name
        self._is_final_task = is_final_task
        self._workflow_state = workflow_state

    @contextmanager
    def apply_environment(self) -> None:
        original_environ = os.environ.copy()
        try:
            os.environ.update(
                self.workflow_state["environment_variables"] or {}
            )
            yield
        finally:
            os.environ.clear()
            os.environ.update(original_environ)

    # Noop by default.
    @abstractmethod
    def run(self, parser: ScopedArgumentParser) -> None:
        pass

    @property
    def task_name(self) -> str:
        return self._task_name

    @property
    def is_final_task(self) -> bool:
        return self._is_final_task

    @property
    def workflow_state(self) -> dict[str, Any]:
        return self._workflow_state

    def get_task_arguments(self, scope: ArgumentScope) -> list[str]:
        return ScopedArgumentParser.get_arguments(scope)

    @classmethod
    def main(cls, *, task_name: str = None, is_final_task: bool = None) -> None:
        parser = ScopedArgumentParser()
        parser.add_argument(
            "--workflow_task_name",
            help="The mnemonic associated with this task.",
            scope=ArgumentScope.META,
            **({} if task_name is None else {"default": task_name}),
        )
        parser.add_argument(
            "--workflow_previous_state",
            help="A file to the previous workflow state.",
            scope=ArgumentScope.META,
            type=parser.path_arg(),
            required=False,
        )
        parser.add_argument(
            "--workflow_next_state",
            help="A file write to the next workflow state.",
            scope=ArgumentScope.META,
            type=parser.path_arg(None),
        )
        parser.add_argument(
            "--workflow_final_task",
            help="Whether this task is the root (final) task in the workflow.",
            action="store_true",
            scope=ArgumentScope.META,
            **({} if is_final_task is None else {"default": is_final_task}),
        )
        workflow_args = parser.parse_args()
        task = cls(
            task_name=workflow_args.workflow_task_name,
            is_final_task=workflow_args.workflow_final_task,
            workflow_state=FuchsiaTask.read_workflow_state(
                workflow_args.workflow_previous_state
            ),
        )
        try:
            with task.apply_environment():
                task.run(parser)
        except TaskExecutionException as e:
            print(Terminal.error(e, prefix="Fatal"))
            if e.is_caught_failure:
                task.workflow_state["workflow"]["halt_execution"] = True
            else:
                sys.exit(1)
        except KeyboardInterrupt:
            sys.exit(1)
        if workflow_args.workflow_next_state:
            workflow_args.workflow_next_state.write_text(
                json.dumps(task.workflow_state)
            )


if __name__ == "__main__":
    FuchsiaTask.main()
