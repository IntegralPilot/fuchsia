# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import asyncio
import typing
from abc import ABC, abstractmethod
from dataclasses import dataclass
from pathlib import Path

import build_dir
from async_utils.command import (
    AsyncCommand,
    CommandEvent,
    CommandOutput,
    StderrEvent,
    StdoutEvent,
)


class ExecutableCommand(ABC):
    """Abstract base class for wrappers that can execute commands."""

    @abstractmethod
    async def start(self, *args: str) -> AsyncCommand:
        """Start this command with the given arguments.

        Returns:
            AsyncCommand: Wrapper for the started command.
        """

    async def sync(
        self,
        *args: str,
        stdout_callback: typing.Callable[[StdoutEvent], None] | None = None,
        stderr_callback: typing.Callable[[StderrEvent], None] | None = None,
    ) -> CommandOutput:
        """Run this command to completion as an async task.

        Returns:
            CommandOutput: The result of running the command to completion.
        """

        def local_callback(event: CommandEvent) -> None:
            if stdout_callback is not None and isinstance(event, StdoutEvent):
                stdout_callback(event)
            elif stderr_callback is not None and isinstance(event, StderrEvent):
                stderr_callback(event)

        running_command = await self.start(*args)
        return await running_command.run_to_completion(callback=local_callback)


class FxCmd(ExecutableCommand):
    """Wrapper for executing `fx` commands through Python.

    Usage (async):
        execution = FxCmd(timeout=30.0).start("build")
        result = await execution.run_to_completion()

    Usage (sync):
        def callback(line: StdoutEvent):
          print(line.text)
        FxCmd(timeout=30.0).sync("build", stdout_callback=callback)
    """

    def __init__(
        self,
        build_directory: str | Path | None = None,
        timeout: float | None = None,
    ):
        """Entry-point for running `fx` commands.

        This creates and configures a wrapper for running any fx command.

        Args:
            build_directory (Path | None, optional): If set, use
                this as the build directory. Otherwise, determine build
                directory from environment.
            timeout (float | None, optional): Timeout for the command
                in seconds. Default is no timeout.
        """
        if build_directory is not None:
            build_directory = str(build_directory)
        self._build_directory: str | None = build_directory
        self._timeout: float | None = timeout

    def command_line(self, *args: str) -> list[str]:
        """The formatted command line this command will execute for the given args."""
        build_directory = self._build_directory
        if build_directory is None:
            build_directory = str(build_dir.get_build_directory())

        return ["fx", "--dir", build_directory] + list(args)

    async def start(self, *args: str) -> AsyncCommand:
        """Start an invocation of fx asynchronously.

        The returned command can be iterated over for output of the
        command, or run_to_completion can be used to get the final
        result of the called process.

        Returns:
            AsyncCommand: An asynchronously running invocation of fx.
        """
        new_args = self.command_line(*args)
        return await AsyncCommand.create(
            new_args[0], *new_args[1:], timeout=self._timeout
        )


EventType = typing.TypeVar("EventType")
ReturnType = typing.TypeVar("ReturnType")


class QueueFinished:
    """Sentinel value to determine a queue is finished."""


@dataclass
class CommandFailed(Exception):
    """Exception for when a command fails to execute."""

    # The return code of the command
    return_code: int


@dataclass
class CommandTransformFailed(CommandFailed):
    """Exception for when a transformation operation failed."""

    # The exception raised by the transformer.
    inner: Exception


class CommandTimeout(CommandFailed):
    """Exception for when a command fails due to a timeout."""

    def __init__(self, return_code: int):
        super().__init__(return_code=return_code)


@dataclass
class RunningCommand(typing.Generic[EventType, ReturnType]):
    """Container for the output of a running command."""

    # The command being executed.
    command: AsyncCommand

    # Task computing the result of the command execution.
    # This must be awaited.
    result: asyncio.Task[ReturnType | CommandFailed]

    # Queue of events during the execution of the command.
    events: asyncio.Queue[EventType | QueueFinished]


class CommandTransformer(typing.Generic[EventType, ReturnType], ABC):
    """Generic transformer of command output."""

    def __init__(self, *args: str, inner: ExecutableCommand):
        """Create a transformer that executes a command with the given args
        and configured executor.

        Args:
            inner (ExecutableCommand): A configured wrapper around running
                a command.
        """
        self._args: list[str] = list(args)
        self._inner: ExecutableCommand = inner

    async def start(self) -> RunningCommand[EventType, ReturnType]:
        """Start the command asynchronously.

        Returns:
            RunningCommand[EventType, ReturnType]: Control object for the command.
        """
        cmd = await self._inner.start(*self._args)
        events = asyncio.Queue[EventType | QueueFinished]()

        async def task() -> ReturnType | CommandFailed:
            event_exception: Exception | None = None

            def do_event_add(event: EventType) -> None:
                events.put_nowait(event)

            def event_callback(event: CommandEvent) -> None:
                nonlocal event_exception
                try:
                    self._handle_event(event, do_event_add)
                except Exception as e:
                    event_exception = e

            final = await cmd.run_to_completion(callback=event_callback)
            events.put_nowait(QueueFinished())
            if final.was_timeout:
                return CommandTimeout(final.return_code)
            if final.return_code != 0:
                return CommandFailed(final.return_code)
            if event_exception is not None:
                return CommandTransformFailed(
                    final.return_code, event_exception
                )

            try:
                return self._to_output(final)
            except Exception as e:
                return CommandTransformFailed(final.return_code, e)

        t = asyncio.create_task(task())
        return RunningCommand(cmd, t, events)

    async def sync(
        self, event_callback: typing.Callable[[EventType], None] | None = None
    ) -> ReturnType:
        """Run the command to completion synchronously.

        Args:
            event_callback (typing.Callable[[EventType], None] | None, optional):
            Optional receiver for bespoke command events.

        Raises:
            CommandError: If the command's return code is not 0
            CommandTimeout: If the command reached a specified timeout and was cancelled.

        Returns:
            ReturnType: The return value for the wrapped command.
        """

        running_command = await self.start()

        async def drain_events() -> None:
            while (event := await running_command.events.get()) is not None:
                if isinstance(event, QueueFinished):
                    return

                if event_callback is not None:
                    event_callback(event)

        results = await asyncio.gather(running_command.result, drain_events())
        result = results[0]
        if isinstance(result, CommandFailed):
            raise result
        return result

    def _handle_event(
        self, event: CommandEvent, callback: typing.Callable[[EventType], None]
    ) -> None:
        """Optional method to override to convert command output into events.

        Args:
            event (CommandEvent): The incoming event.
            callback (typing.Callable[[EventType], None]): A callback
                to publish a new event in the bespoke event type for
                this transformer.
        """

    @abstractmethod
    def _to_output(
        self,
        output: CommandOutput,
    ) -> ReturnType:
        """Abstract method that must be implemented to turn command
        output into the declared return type.

        Args:
            output (CommandOutput): Output of the command.

        Returns:
            ReturnType: The output of the command.
        """
