#!/usr/bin/env python3
# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
from abc import ABC, abstractmethod
from collections import namedtuple
from functools import reduce
from operator import add
from pathlib import Path
from typing import Any

from fuchsia.tools.fuchsia_task_lib import (
    FuchsiaTask,
    TaskExecutionException,
    Terminal,
)

WorkflowArguments = namedtuple(
    "WorkflowArguments",
    [
        "manifest",
        "dump",
        "dry_run",
        "verbose",
        "help",
        "global_arguments",
    ],
)

ARGUMENTS = None


def parse_workflow_args() -> None:
    global ARGUMENTS
    ARG = 0
    BOOL_ARG = 1 << 0
    FORWARD_ARG = 1 << 1
    arg_opts = {
        "--workflow-manifest": ("manifest", ARG),
        "--workflow-dump": ("dump", BOOL_ARG),
        "--workflow-dry-run": ("dry_run", BOOL_ARG),
        "--workflow-verbose": ("verbose", BOOL_ARG),
        "--help": ("help", BOOL_ARG | FORWARD_ARG),
    }
    cla = sys.argv[1:]
    args = {
        "manifest": None,
        "dump": None,
        "dry_run": None,
        "verbose": None,
        "help": None,
    }
    workflow_arguments = []
    while cla:
        arg = cla.pop(0)
        if arg in arg_opts:
            target, opt = arg_opts[arg]
            if args[target] is None:
                args[target] = True if opt & BOOL_ARG else cla.pop(0)
                if opt & FORWARD_ARG:
                    forward_args = [arg] + (
                        [] if opt & BOOL_ARG else [args[target]]
                    )
                    workflow_arguments.extend(
                        [f"entity.task={arg}" for arg in forward_args]
                    )
            else:
                raise Exception(f"Duplicate argument {arg}")
        else:
            workflow_arguments.append(arg)

    args["help"] = bool(args["help"])
    args["dry_run"] = bool(args["dry_run"])
    args["verbose"] = args["verbose"] or args["dry_run"]
    args["dump"] = bool(args["dump"])
    if args["manifest"]:
        args["manifest"] = Path(args["manifest"])
    else:
        raise Exception(f"Expected argument --workflow-manifest")

    ARGUMENTS = WorkflowArguments(global_arguments=workflow_arguments, **args)


def run(*command: Path | str, **kwargs: dict[str, Any]) -> None:
    if ARGUMENTS.verbose:
        print("Executing command: ", command)
    if ARGUMENTS.dry_run:
        return

    # Workaround for https://github.com/bazel-contrib/rules_python/issues/3518
    # Clean up environment to avoid RUNFILES_DIR/RUNFILES_MANIFEST_FILE inheritance
    # which can confuse child Python processes.
    env = dict(kwargs.get("env", os.environ))
    env.pop("RUNFILES_DIR", None)
    env.pop("RUNFILES_MANIFEST_FILE", None)
    kwargs["env"] = env

    subprocess.check_call(command, **kwargs)


class Entity(ABC):
    def __init__(
        self,
        *,
        entity_collection: "EntityCollection",
        label: str,
        task_num: int = 1,
        mnemonic: str,
        mnemonic_suffix: str,
        explicit_args: list[tuple[float, str]],
        propagated_args: list[tuple[float, str]],
        global_args: list[tuple[float, str]],
    ) -> None:
        self._entity_collection = entity_collection
        self._label = label
        self._task_num = task_num
        self._mnemonic = mnemonic
        self._mnemonic_suffix = mnemonic_suffix
        self._explicit_args = explicit_args
        self._propagated_args = propagated_args
        self._global_args = global_args

    @property
    def name(self) -> str:
        # Uses the mnemonic, with the suffix if necessary.
        return self.mnemonic + (
            self.mnemonic_suffix if self.needs_suffix() else ""
        )

    @property
    def mnemonic(self) -> str:
        return self._mnemonic

    @property
    def mnemonic_suffix(self) -> str:
        return self._mnemonic_suffix

    def needs_suffix(self) -> bool:
        return self._entity_collection.is_ambiguous(self._mnemonic)

    @property
    def label(self) -> str:
        return self._label

    def sort_arguments(self, args: list[tuple[float, str]]) -> list[str]:
        return list(
            (list(zip(*sorted(args, key=lambda arg: arg[0]))) or [None, []])[1]
        )

    @property
    def explicit_args(self) -> list[str]:
        return self.sort_arguments(self._explicit_args)

    @property
    def propagated_args(self) -> list[str]:
        return self.sort_arguments(self._propagated_args)

    @property
    def global_args(self) -> list[str]:
        return self.sort_arguments(self._global_args)

    @property
    def task_num(self) -> int:
        return self._task_num

    @property
    @abstractmethod
    def task_count(self) -> int:
        raise Exception("Entity.task_count is not implemented.")

    @classmethod
    def create(cls, *_, **kwargs) -> "Entity":
        raise Exception("Entity.create is not implemented.")

    @abstractmethod
    def run(self, total_tasks: int) -> None:
        raise Exception("Entity.run is not implemented.")

    def __str__(self) -> str:
        return self.label


class Task(Entity):
    def __init__(
        self, *, task_runner: Path, default_argument_scope: str, **entity_kwargs
    ) -> None:
        super().__init__(**entity_kwargs)
        self._task_runner = task_runner.resolve()
        self._default_argument_scope = default_argument_scope
        self._previous_state_file = (
            self._entity_collection.last_task
            and self._entity_collection.last_task.incremental_workflow_state_file
        )

    @property
    def task_count(self) -> int:
        return 1

    @property
    def incremental_workflow_state_file(self) -> Path:
        return (
            self._entity_collection.global_working_dir
            / f"{self.mnemonic}{self.mnemonic_suffix}.state"
        )

    @property
    def task_runner(self) -> Path:
        return self._task_runner

    @property
    def default_argument_scope(self) -> str:
        return (
            "global"
            if self._entity_collection.entrypoint.task_count == 1
            else self._default_argument_scope
        )

    @classmethod
    def create(
        cls,
        entity: dict[str, Any],
        **entity_kwargs,
    ) -> "Task":
        return Task(
            task_runner=Path(entity["task_runner"]),
            default_argument_scope=entity["default_argument_scope"],
            explicit_args=[
                (i - len(entity["args"]), arg)
                for i, arg in enumerate(entity["args"])
            ],
            **entity_kwargs,
        )

    @property
    def effective_args(self) -> list[str]:
        meta_args = [
            arg
            for meta_arg in [
                "--workflow_task_name",
                self.name,
                *(
                    ["--workflow_final_task"]
                    if self._entity_collection.last_task == self
                    else []
                ),
                *(
                    ["--workflow_previous_state", self._previous_state_file]
                    if self._previous_state_file
                    else []
                ),
                "--workflow_next_state",
                self.incremental_workflow_state_file,
                "--default_argument_scope",
                self.default_argument_scope,
            ]
            for arg in ["__META_ARGUMENT__", meta_arg]
        ]
        global_args = [
            arg
            for orig_arg in self._global_args
            for arg in ["__GLOBAL_ARGUMENT__", orig_arg]
        ]
        workflow_args = [
            (idx, arg)
            for orig_idx, orig_arg in self._propagated_args
            for idx, arg in [
                (orig_idx - 0.5, "__WORKFLOW_ARGUMENT__"),
                (orig_idx, orig_arg),
            ]
        ]
        return [
            *meta_args,
            *self.sort_arguments(workflow_args + self._explicit_args)
            + global_args,
        ]

    def run(self, total_tasks: int = 1) -> None:
        task_description = (
            f"{Terminal.purple(self.name)} (step {self.task_num}/{total_tasks})"
        )
        print(
            Terminal.if_no_color("<progress> ")
            + f'{Terminal.bold("Running task:")} {task_description}'
        )
        try:
            run(self.task_runner, *self.effective_args)
        except subprocess.SubprocessError:
            raise TaskExecutionException(
                f'{Terminal.red("Error:")} Task {task_description} failed.'
            )
        except KeyboardInterrupt:
            raise TaskExecutionException(
                f'{Terminal.red("Task Interrupted")}: {task_description}.'
            )
        if FuchsiaTask.read_workflow_state(
            self.incremental_workflow_state_file
        )["workflow"]["halt_execution"]:
            raise TaskExecutionException("Workflow execution halted.")


class Workflow(Entity):
    def __init__(self, *, sequence: list[Entity], **entity_kwargs) -> None:
        super().__init__(**entity_kwargs)
        self._sequence = sequence

    @property
    def task_count(self) -> int:
        return sum([dep.task_count for dep in self.sequence])

    @classmethod
    def create(
        cls,
        entity_collection: "EntityCollection",
        entity: dict[str, Any],
        propagated_args: list[tuple[float, str]],
        task_num: int = 1,
        **entity_kwargs,
    ) -> "Workflow":
        explicit_args = [
            (i - len(entity["args"]), arg)
            for i, arg in enumerate(entity["args"])
        ]
        child_task_num = task_num
        sequence = []
        for step in entity["sequence"]:
            sequence.append(
                entity_collection.create(
                    label=step,
                    task_num=child_task_num,
                    propagated_args=propagated_args + explicit_args,
                )
            )
            child_task_num += sequence[-1].task_count
        return Workflow(
            entity_collection=entity_collection,
            sequence=sequence,
            explicit_args=explicit_args,
            propagated_args=propagated_args,
            task_num=task_num,
            **entity_kwargs,
        )

    @property
    def sequence(self) -> list[Entity]:
        return self._sequence

    def run(self, total_tasks: int | None = None) -> None:
        print(
            Terminal.if_no_color("<progress> ")
            + f'{Terminal.bold("Running workflow:")} {Terminal.green(self.name)}'
        )
        for entity in self.sequence:
            entity.run(total_tasks=total_tasks or self.task_count)


class EntityCollection:
    def __init__(
        self,
        entities: dict[str, dict[str, Any]],
        entrypoint: str,
        args: list[str],
        global_working_dir: Path,
    ) -> None:
        self._data_entities = entities
        self._args = args
        self._global_working_dir = global_working_dir
        self._mnemonics_by_label: dict[str, list[str]] = {}
        self._entity_by_mnemonic: dict[str, Entity] = {}
        self._last_task = None
        self._entrypoint = self.create(
            label=entrypoint,
            task_num=1,
            propagated_args=[],
        )

    def is_ambiguous(self, mnemonic: str) -> bool:
        return len(self._mnemonics_by_label.get(mnemonic, [])) > 1

    def _determine_mnemonics(self, label: str) -> tuple[str, str]:
        mnemonic = re.search(r"[\w\.\-_]+$", label)[0]
        associated_mnemonics = self._mnemonics_by_label.get(mnemonic, [])

        suffix = f".{len(associated_mnemonics) + 1}"
        self._mnemonics_by_label[mnemonic] = [
            *associated_mnemonics,
            mnemonic + suffix,
        ]

        return mnemonic, suffix

    def create(
        self,
        label: str,
        task_num: int,
        propagated_args: list[tuple[float, str]],
    ) -> Workflow | Task:
        mnemonic, suffix = self._determine_mnemonics(label)
        entity_type = self._data_entities[label]["type"]
        entity = {
            "task": Task,
            "workflow": Workflow,
        }[entity_type].create(
            entity_collection=self,
            label=label,
            mnemonic=mnemonic,
            mnemonic_suffix=suffix,
            propagated_args=propagated_args,
            global_args=self._args,
            entity=self._data_entities[label],
            task_num=task_num,
        )
        if entity_type == "task":
            self._last_task = entity
        self._entity_by_mnemonic[mnemonic + suffix] = entity
        return entity

    @property
    def last_task(self) -> Task | None:
        return self._last_task

    @property
    def global_working_dir(self) -> Path:
        return self._global_working_dir

    @property
    def entrypoint(self) -> Entity:
        return self._entrypoint

    def from_manifest(
        entities: dict[str, dict[str, Any]],
        entrypoint: str,
        args: list[str],
        global_working_dir: Path,
    ) -> Entity:
        return EntityCollection(
            entities, entrypoint, args, global_working_dir
        ).entrypoint


class WorkflowRunner:
    def __init__(
        self,
        entities: dict[str, dict[str, Any]],
        entrypoint: str,
        args: list[str],
    ) -> None:
        self._working_directory = Path(tempfile.gettempdir()) / (
            "workflow.%s" % re.search(r"[\w\.\-_]+$", entrypoint)[0]
        )
        self.entrypoint = EntityCollection.from_manifest(
            entities, entrypoint, args, self._working_directory
        )

    def run(self) -> None:
        if self._working_directory.exists():
            shutil.rmtree(self._working_directory)
        self._working_directory.mkdir()
        self.entrypoint.run()

    def dump_workflow_tree(self) -> "WorkflowRunner":
        CHILD = Terminal.bold("├── ")
        NESTED = Terminal.bold("│   ")
        LAST_CHILD = Terminal.bold("└── ")
        LAST_NESTED = Terminal.bold("    ")

        root = self.entrypoint

        def preorder_traversal(
            node: Entity,
            prefixes: list[str] = [],
            is_last_child: bool = False,
        ) -> None:
            def print_entry(first_line: Any, *additional_lines: Any):
                print(
                    reduce(add, prefixes[:-1], "")
                    + (
                        ""
                        if not prefixes
                        else LAST_CHILD
                        if is_last_child
                        else CHILD
                    )
                    + str(first_line)
                )
                for line in additional_lines:
                    print(reduce(add, prefixes, "") + str(line))

            suffix = (
                Terminal.cyan(node.mnemonic_suffix)
                if node.needs_suffix()
                else ""
            )
            if isinstance(node, Workflow):
                print_entry(
                    f'{Terminal.bold("Workflow:")} {Terminal.green(node.mnemonic) + suffix}',
                    f'{Terminal.bold("Label:")} {Terminal.underline(node.label)}',
                    f'{Terminal.bold("Subtask Arguments:")} {node.explicit_args}',
                )
                for child, is_last in zip(
                    node.sequence, (len(node.sequence) - 1) * [False] + [True]
                ):
                    preorder_traversal(
                        child,
                        [*prefixes, LAST_NESTED if is_last else NESTED],
                        is_last,
                    )
            else:
                print_entry(
                    f'{Terminal.bold("Task:")} {Terminal.purple(node.mnemonic) + suffix} (step {node.task_num}/{root.task_count})',
                    f'{Terminal.bold("Label:")} {Terminal.underline(node.label)}',
                    f'{Terminal.bold("Runner:")} {Terminal.underline(node.task_runner)}',
                    f'{Terminal.bold("Workflow Arguments:")} {node.propagated_args}',
                    f'{Terminal.bold("Default Argument Scope:")} {node.default_argument_scope}',
                    f'{Terminal.bold("Task Arguments:")} {node.explicit_args}',
                    f"Effective Arguments: {node.effective_args}",
                )

        print(Terminal.bold("Execution Sequence:"))
        print(
            f'{Terminal.bold("Global Arguments:")} {self.entrypoint.global_args}'
        )
        print(
            f'{Terminal.bold("Working Directory:")} {self._working_directory}'
        )
        preorder_traversal(root)
        return self


def main():
    # Parse arguments.
    parse_workflow_args()

    # Parse the workflow.
    workflow_spec = json.loads(Path(ARGUMENTS.manifest).read_text())

    # Run the workflow.
    runner = WorkflowRunner(
        workflow_spec["entities"],
        workflow_spec["entrypoint"],
        ARGUMENTS.global_arguments,
    )

    if ARGUMENTS.dump or ARGUMENTS.verbose or ARGUMENTS.help:
        runner.dump_workflow_tree()

    if ARGUMENTS.dump:
        return 0

    try:
        runner.run()
    except TaskExecutionException as e:
        print(e)
        return 1


if __name__ == "__main__":
    sys.exit(main())
