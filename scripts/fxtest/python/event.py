# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Event definitions for `fx test`.

This module contains the definitions for all events that can occur during
an execution of `fx test`. All user-visible operations are represented
as an Event structure, which supports serialization to JSON. Event handlers
support displaying events to a user, writing to a log file, uploading to a
server, or any other operation that is desired.
"""

import asyncio
from dataclasses import dataclass
from dataclasses import fields
import datetime
import enum
import time
import typing

from dataparse import dataparse
import environment
import selection_types
import tests_json_file

# Events may have a unique Id, represented as a monotonically increasing integer.
Id = typing.NewType("Id", int)

# Event Id 0 is special, referring to the entire invocation of `fx test`
GLOBAL_RUN_ID: Id = Id(0)


@dataparse
@dataclass
class FileParsingPayload:
    """Payload for a file parsing event."""

    # Name of the file being parsed.
    name: str

    # Path on the host system where the file was located.
    path: str


class MessageLevel(enum.Enum):
    """Valid user message types.

    Note that there is no ERROR level, because Events may themselves
    represent an error and they hold their own error messages.
    """

    # The message is an instruction to the user. Typically dimmed in terminal output.
    INSTRUCTION = "INSTRUCTION"
    # The message has actionable information for the user, typically shown normally in terminal output.
    INFO = "INFO"
    # The message is a warning to the user, typically shown in yellow in terminal output.
    WARNING = "WARNING"
    # The message should be displayed verbatim. This is used for command output.
    VERBATIM = "VERBATIM"


@dataparse
@dataclass
class Message:
    """Display this value to a user."""

    # The string to display.
    value: str

    # The level of the message to display. Different levels of messages
    # may have different styling applied.
    # See above for levels.
    level: MessageLevel


@dataparse
@dataclass
class TestsJsonFilePayload:
    """The result of loading a tests.json file."""

    # The complete list of entries parsed from the file.
    # This output is in the format parsed by tests_json_file, not the
    # verbatim input.
    test_entries: list[tests_json_file.TestEntry]

    # The path of the tests.json file that was parsed.
    file_path: str


@dataparse
@dataclass
class ProgramExecutionPayload:
    """Details about a program execution."""

    # The name of the command that was executed.
    command: str

    # List of flags passed to the command.
    flags: list[str]

    # The environment passed to the command.
    environment: dict[str, str]

    # If set, instruct viewers to hide this command.
    quiet_mode: bool = False

    def to_formatted_command_line(self) -> str:
        """Format this program execution to an approximation of the command line.

        This output can be shown to the user to describe the command invocation.

        Returns:
            str: Formatted command line.
        """
        return f"{self.command}{'' if not self.flags else ' ' + ' '.join(self.flags)}"


class ProgramOutputStream(enum.Enum):
    """Details about the source of program output."""

    # Designates output as coming from stdout.
    STDOUT = "STDOUT"

    # Designates output as coming from stderr.
    STDERR = "STDERR"


@dataparse
@dataclass
class ProgramOutputPayload:
    """Payload for output bytes from a program."""

    # The data, as a string.
    data: str

    # The stream this data came from. Either STDOUT or STDERR.
    stream: ProgramOutputStream

    # If true, the user asked for this output to be printed verbatim to their
    # console.
    print_verbatim: bool = False


@dataparse
@dataclass
class ProgramTerminationPayload:
    """Payload for a program terminating."""

    # The return code of the program. 0 is success, and any other
    # value is failure.
    return_code: int


@dataparse
@dataclass
class TestSelectionPayload:
    """Payload for test selection events.

    This payload provides the complete set of selected and not selected tests
    for this command invocation.
    """

    # Map of selected test names to their score that was below the threshold.
    selected: dict[str, int]

    # Map of not selected test names to their score that was above the threshold.
    not_selected: dict[str, int]

    # Map of selected but not run test names to their score that was below the threshold.
    selected_but_not_run: dict[str, int]

    # The distance threshold this selection run was configured with.
    fuzzy_distance_threshold: int


@dataparse
@dataclass
class EventGroupPayload:
    """Represents a group of events that may have their own children."""

    # The display name for this group.
    name: str

    # An optional count of events queued on the group.
    # If set, this value can be used to create a progress bar for this event group.
    queued_events: int | None = None

    # If true, instructs console displays to hide children.
    hide_children: bool = False


class TestGroupPayload(EventGroupPayload):
    """Test groups are specializations of event groups.

    They are stored separately to support more detailed output to users.
    """

    def __init__(self, tests_to_run: int):
        """Initialize a new test group.

        Args:
            tests_to_run (int): Number of tests to run. Used for
            formatting a name and to initialize the number of queued
            events.
        """
        super().__init__(
            name=f"Running {tests_to_run} tests", queued_events=tests_to_run
        )

    @classmethod
    def from_dict(cls, dict: dict[str, typing.Any]) -> typing.Self:
        s: EventGroupPayload = super().from_dict(dict)  # type:ignore
        ret: typing.Self = cls(0)
        ret.name = s.name
        ret.queued_events = s.queued_events
        return ret


@dataparse
@dataclass
class TestSuiteStartedPayload:
    """A test suite started."""

    # The name of the suite.
    name: str

    # If true, this test suite is hermetic and may be run in parallel.
    hermetic: bool | None = False


class TestSuiteStatus(enum.Enum):
    """Result status for a test suite's execution."""

    # A test suite passed.
    PASSED = "PASSED"

    # A test suite failed.
    FAILED = "FAILED"

    # A test suite was skipped for some reason.
    SKIPPED = "SKIPPED"

    # The test suite execution was aborted due to some condition.
    ABORTED = "ABORTED"

    # The test suite was aborted due to exceeding its timeout.
    TIMEOUT = "TIMEOUT"

    # The test suite failed to start due to a configuration issue. This
    # counts as a failure.
    FAILED_TO_START = "FAILED_TO_START"


@dataparse
@dataclass
class TestSuiteEndedPayload:
    """A test suite finished executing."""

    # The status message from above representing the result of this test suite.
    status: TestSuiteStatus

    # Optionally, a message describing what happened.
    message: str | None = None


@dataparse
@dataclass
class EnumerateTestCasesPayload:
    """A test suite's cases were enumerated."""

    # The name of the test being enumerated.
    test_name: str

    # The names of the test cases in the test.
    test_case_names: list[str]

    # Optional template for the run command.
    # Should contain {test_case} placeholder.
    command_template: str | None = None

    # format a command for running a specific test case.
    def maybe_format_command(
        self, test_name: str, test_case: str
    ) -> str | None:
        if self.command_template is None:
            return None
        return self.command_template.format(
            test_name=test_name, test_case=test_case
        )


@dataparse
@dataclass
class LoadConfigPayload:
    """A configuration file was loaded with flag defaults."""

    # Path to config file.
    path: str

    # The flag defaults parsed from the config file.
    flags: dict[str, typing.Any]

    # The command line provided in the config file.
    command_line: list[str]


@dataparse
@dataclass
class EventPayloadUnion:
    """Payload for event types.

    At most one of the below fields may be set.

    Rather than using derived classes, we use the following at-most-one-set
    union class for better compatibility with the @dataparse wrapper. This
    union implements an "externally tagged" enum, which could be replaced
    with a different form of tagging in the future.
    """

    def __post_init__(self) -> None:
        fields_present: typing.Set[str] = set(
            [f.name for f in fields(self) if getattr(self, f.name) is not None]
        )
        if len(fields_present) != 1:
            raise ValueError(
                "Only one field may be set on EventPayloadUnion. The following were found: "
                + str(fields_present)
            )

    def __str__(self) -> str:
        if self.start_timestamp is not None:
            return "Starting Run"
        elif self.load_config is not None:
            return "Loaded config: " + str(self.load_config)
        elif self.parse_flags is not None:
            return "Parsed flags: " + str(self.parse_flags)
        elif self.process_env is not None:
            return "Processed environment: " + str(self.process_env)
        elif self.user_message is not None:
            # Include the user message, except for VERBATIM payloads. In that
            # case, simply print the number of characters. This avoids duplication
            # of verbose command outputs.
            value = self.user_message.value
            level = self.user_message.level
            value_print = (
                value.strip()
                if level != MessageLevel.VERBATIM
                else (f"<{len(value)} characters of verbatim output>")
            )
            return f"Display user message: [{level}] {value_print}"
        elif self.parsing_file is not None:
            return (
                f"Parsing {self.parsing_file.name} at {self.parsing_file.path}"
            )
        elif self.program_execution is not None:
            return f"Running `{self.program_execution.command} {' '.join(self.program_execution.flags)}`"
        elif self.program_output is not None:
            return f"Got {len(self.program_output.data)} characters from {self.program_output.stream}"
        elif self.program_termination is not None:
            return f"Terminated with return_code = {self.program_termination.return_code}"
        elif self.test_selections is not None:
            return f"Selected {len(self.test_selections.selected)} tests"
        elif self.test_file_loaded is not None:
            return f"Loaded {len(self.test_file_loaded.test_entries)} tests from {self.test_file_loaded.file_path}"
        elif self.event_group is not None:
            return f"Starting group {self.event_group.name}"
        elif self.build_targets is not None:
            return f"Building {len(self.build_targets)} targets"
        elif self.test_group is not None:
            return f"Starting test group {self.test_group.name}"
        elif self.test_suite_started is not None:
            return f"Starting test suite {self.test_suite_started.name}"
        elif self.test_suite_ended is not None:
            end_msg = self.test_suite_ended.message
            suffix = "" if not end_msg else f": {end_msg}"
            return f"Ending test suite with state {self.test_suite_ended.status}{suffix}"
        elif self.enumerate_test_cases is not None:
            return f"Enumerated {len(self.enumerate_test_cases.test_case_names)} cases for {self.enumerate_test_cases.test_name}"
        else:
            return "BUG: UNKNOWN EVENT PAYLOAD " + str(self.__dict__)

    # If set, this event denotes the start time of the run.
    # Payload is the actual timestamp of the run start as a UNIX timestamp.
    #
    # Other timestamps are in monotonic time, so the mapping of the monotonic
    # time for the containing event to this UNIX timestamp must be used for all
    # time formatting.
    start_timestamp: float | None = None

    # This event denotes loading a configuration file containing flag defaults.
    #
    # The payload contains the file path and the parsed command line flags that
    # are used as defaults.
    load_config: LoadConfigPayload | None = None

    # This event denotes parsing command line flags.
    #
    # The parsed command line flags are included in the value.
    parse_flags: dict[str, typing.Any] | None = None

    # This event denotes processing the execution environment.
    #
    # The parsed environment is included in the value.
    process_env: environment.ExecutionEnvironment | None = None

    # This event denotes the computation of a final artifact directory.
    #
    # The absolute path to the directory is included in the value.
    # The path will be empty if this run will not save artifacts.
    artifact_directory_path: str | None = None

    # This event denotes a message to be shown to the user.
    #
    # The value provides display information.
    user_message: Message | None = None

    # This event denotes the beginning of a new event group.
    #
    # The value provides details on the group.
    event_group: EventGroupPayload | None = None

    # This event denotes a generic file parsing duration.
    #
    # The value provides details of the file being parsed.
    parsing_file: FileParsingPayload | None = None

    # This event denotes a program starting executing.
    #
    # The value provides details on the program.
    program_execution: ProgramExecutionPayload | None = None

    # This event denotes output from a running program.
    #
    # The value provides contents of the output.
    program_output: ProgramOutputPayload | None = None

    # This event denotes the termination of a program.
    #
    # The value provides details on the return code.
    program_termination: ProgramTerminationPayload | None = None

    # This event denotes the results of loading the tests.json file.
    #
    # The value provides details of the parsed data.
    test_file_loaded: TestsJsonFilePayload | None = None

    # This event denotes selection of a set of tests.
    #
    # The value provides details on selection decisions.
    test_selections: TestSelectionPayload | None = None

    # This event denotes the beginning of a build operation.
    #
    # The value lists the targets being built.
    build_targets: list[str] | None = None

    # This event denotes the beginning of a group of test suites.
    #
    # The value provides display information about the tests.
    test_group: TestGroupPayload | None = None

    # This event denotes the beginning of a test suite.
    #
    # The value provides details on the suite.
    test_suite_started: TestSuiteStartedPayload | None = None

    # This event denotes the end of a test suite.
    #
    # The value provides result information.
    test_suite_ended: TestSuiteEndedPayload | None = None

    # This event denotes the numeration of cases within a test suite.
    #
    # The value provides details on the cases that were found.
    enumerate_test_cases: EnumerateTestCasesPayload | None = None


@dataparse
@dataclass
class Event:
    # Unique Id for the event. If not set, this event is not
    # associated with a known duration.
    id: Id | None = None

    # Monotonic timestamp for the event.
    timestamp: float = 0

    # Parent Id for the event. If not set, treat GLOBAL_RUN_ID as
    # the implicit parent
    parent: Id | None = None

    # If set, a new duration is starting with the above Id.
    starting: bool | None = None

    # If set, a duration with the above Id has ended.
    ending: bool | None = None

    # If set, the duration ended with an error. The human-readable
    # message is stored in this field.
    error: str | None = None

    # Optional payload for the event. See EventPayloadUnion
    # documentation for details.
    payload: EventPayloadUnion | None = None


class EventStatCategory(enum.Enum):
    BUILDING = "Building"
    PARSING = "Parsing"
    TESTING = "Testing"
    SEARCHING = "Searching"
    OTHERS = "Others"
    IGNORE = "Ignore"


@dataclass
class EventSpan:
    start_time: float
    start_event: Event
    category: EventStatCategory | None = None
    duration: float | None = None

    def __post_init__(self) -> None:
        if self.category is None:
            self.category = self.classify()

    def should_ignore_for_stats(self, payload: EventPayloadUnion) -> bool:
        # Event groups and test groups are containers for other events. They are
        # ignored to avoid double-counting durations, as their time is covered
        # by their children.
        if payload.test_group is not None or payload.event_group is not None:
            return True
        if payload.program_execution is not None:
            # fx serve is a long-running background process that runs throughout
            # the entire test execution.
            if (
                payload.program_execution.command == "fx"
                and "serve" in payload.program_execution.flags
            ):
                return True
        return False

    def classify(self) -> EventStatCategory:
        """Classify the event based on its payload.

        We use heuristics to determine the category. Events that do not represent
        a distinct operation (like structural containers) are classified as IGNORE.
        """
        payload = self.start_event.payload
        if payload is None:
            return EventStatCategory.OTHERS
        elif self.should_ignore_for_stats(payload):
            return EventStatCategory.IGNORE
        elif payload.parsing_file is not None:
            return EventStatCategory.PARSING
        elif (
            payload.program_execution is not None
            and "dldist" in payload.program_execution.flags
        ):
            return EventStatCategory.SEARCHING
        elif payload.test_suite_started is not None:
            return EventStatCategory.TESTING
        elif payload.build_targets is not None:
            return EventStatCategory.BUILDING
        return EventStatCategory.OTHERS


class EventRecorder:
    """Entry point to emitting and listening for events.

    EventRecorder has methods to emit each type of event as well as iterate over
    all events as they are being emitted. An instance should be passed
    everywhere events are emitted or read.
    """

    def __init__(
        self,
    ) -> None:
        """Initialize a new EventRecorder"""

        # Keep track of the system time corresponding to the below monotonic
        # time. This represents the beginning of this EventRecorder's execution.
        self._system_time_start: float = time.time()

        # Keep track of the monotonic time corresponding to the above
        # system time.
        self._monotonic_time_start: float = time.monotonic()

        # Keep track of all events that were emitted as part of execution.
        self._events: list[Event] = []

        # Keep track of each asynchronous event consumer queue.
        self._queues: list[asyncio.Queue[Event | None]] = []

        # Async event designating that this recorder is done.
        self._done: asyncio.Event = asyncio.Event()

        # Keep track of the next unique Id to assign.
        self._next_id: Id = Id(1)

    def _get_timestamp(self) -> float:
        """Produce timestamps for events in this recorder.

        Returns:
            float: Monotonic timestamp for use in an event.
        """
        return time.monotonic()

    def _new_id(self) -> Id:
        """Produce a new unique Id in the context of this EventRecorder.

        Returns:
            Id: A unique Id.
        """
        ret = self._next_id
        self._next_id = Id(self._next_id + 1)
        return ret

    def _local_time_for_monotonic(self, monotonic: float) -> datetime.datetime:
        """Return the local datetime for a given monotonic value.

        This function uses the previously set monotonic time mapping, and it
        must be called after the first call to _get_timestamp.

        Args:
            monotonic (float): The monotonic time to translate.

        Returns:
            datetime.datetime: The system time represented by the monotonic time.
        """
        diff = monotonic - self._monotonic_time_start
        return datetime.datetime.fromtimestamp(self._system_time_start + diff)

    def _emit(self, event: Event) -> None:
        """Helper to emit an event to all listeners.

        Args:
            event (Event): The event to emit.
        """
        self._events.append(event)
        for queue in self._queues:
            queue.put_nowait(event)

    def end(self) -> None:
        """End this queue. No further events may be emitted, and all listeners
        will eventually terminate.
        """
        for queue in self._queues:
            queue.put_nowait(None)
        self._done.set()

    def iter(self) -> typing.AsyncIterable[Event]:
        """Create an iterator over the events of this recorder.

        Raises:
            StopAsyncIteration: Raised when iteration should end.
                Used internally by __anext__.

        Returns:
            typing.AsyncIterable[Event]: Async iterator over the events.

        Warning:
            All returned iterators must be read to completion. Failure to do so
            will result in a memory leak.

        Example:
            recorder = EventRecorder()
            recorder.emit_init()
            recorder.emit_info_message("This is a message")
            asyncio.create_task(/* spawn a task that generates more records,
                                   followed by recorder.emit_end() */)
            async for event in recorder.iter():
              print(recorder.event_string(event))
        """
        parent = self

        class Iter:
            def __aiter__(self) -> typing.Self:
                self._init_items: list[Event] = parent._events.copy()
                self._init_index: int = 0
                self._queue: asyncio.Queue[Event | None] = asyncio.Queue()
                if not parent._done.is_set():
                    parent._queues.append(self._queue)
                else:
                    # Immediately end when we get to reading from the queue, but still return the stored events.
                    self._queue.put_nowait(None)
                return self

            async def __anext__(self) -> Event:
                if self._init_index < len(self._init_items):
                    item = self._init_items[self._init_index]
                    self._init_index += 1
                    return item

                next = await self._queue.get()

                if not next:
                    try:
                        parent._queues.remove(self._queue)
                    except ValueError:
                        pass
                    raise StopAsyncIteration()
                return next

        return Iter()

    def event_string(self, event: Event) -> str:
        """Print out the string representation of an event returned
        by this recorder.

        Args:
            event (Event): The event to print.

        Returns:
            str: String representation of the event.
        """
        time_str = self._local_time_for_monotonic(event.timestamp).strftime(
            "%Y-%m-%d %H:%M:%S.%f"
        )
        if event.id is not None:
            start_stop = (
                "S"
                if event.starting and not event.ending
                else "E"
                if not event.starting and event.ending
                else "I"
            )
            id_line = f"[{event.id:05}:{start_stop}]"
        else:
            id_line = "[_______]"
        return f"{time_str} {id_line:9} {self._payload_string(event.payload)}"

    def _payload_string(self, payload: EventPayloadUnion | None) -> str:
        """Format the payload of an event as a string.

        Args:
            payload (EventPayloadUnion | None): Payload to format.

        Returns:
            str: String representation of the event payload.
        """
        return str(payload) if payload is not None else ""

    def emit_init(self) -> None:
        """Emit the initial event.

        This method must be called first following the creation of
        an EventRecorder.
        """
        self._emit(
            Event(
                GLOBAL_RUN_ID,
                self._monotonic_time_start,
                starting=True,
                payload=EventPayloadUnion(
                    start_timestamp=self._system_time_start
                ),
            )
        )

    def emit_end(self, error: str | None = None, id: Id | None = None) -> None:
        """Emit an end event for an event duration.

        By default, the global run duration is terminated with an error
        optionally given by the first argument.

        Optionally, a different duration may be ended by giving its id.

        Args:
            error (str | None): If set, end the
                given event with an error. Defaults to None.
            id (Id | None): If set, end this
                event instead of the global run. Defaults to None.
        """
        id = id or GLOBAL_RUN_ID
        self._emit(
            Event(
                id or GLOBAL_RUN_ID,
                self._get_timestamp(),
                ending=True,
                error=error,
            )
        )
        if id == GLOBAL_RUN_ID:
            self.end()

    def emit_load_config(
        self,
        path: str,
        flags: dict[str, typing.Any],
        command_line: list[str],
    ) -> None:
        """Emit a load_config event with details on the config.

        Args:
            path (str): The path to the loaded config file.
            flags (dict[str, typing.Any]): The flags passed to this invocation.
            command_line (list[str]): The command line parsed from the config file.
        """
        self._emit(
            Event(
                GLOBAL_RUN_ID,
                self._get_timestamp(),
                payload=EventPayloadUnion(
                    load_config=LoadConfigPayload(path, flags, command_line)
                ),
            )
        )

    def emit_parse_flags(self, flags: dict[str, typing.Any]) -> None:
        """Emit a parse_flags event with details on the flags.

        Args:
            flags (dict[str, typing.Any]): The flags passed to this invocation.
        """
        self._emit(
            Event(
                GLOBAL_RUN_ID,
                self._get_timestamp(),
                payload=EventPayloadUnion(parse_flags=flags),
            )
        )

    def emit_process_env(self, env: environment.ExecutionEnvironment) -> None:
        """Emit a process_env event with details of the environment.

        Args:
            env (ExecutionEnvironment): The environment parsed by this invocation.
        """
        self._emit(
            Event(
                GLOBAL_RUN_ID,
                self._get_timestamp(),
                payload=EventPayloadUnion(process_env=env),
            )
        )

    def _emit_user_message(
        self, message: str, level: MessageLevel = MessageLevel.INFO
    ) -> None:
        """Emit a message to display to a user.

        Args:
            message (str): Message to show.
            level (str, optional): Message level for display. Defaults to MESSAGE_LEVEL_INFO.
        """
        self._emit(
            Event(
                None,
                self._get_timestamp(),
                payload=EventPayloadUnion(
                    user_message=Message(value=message, level=level)
                ),
            )
        )

    def emit_artifact_directory_path(self, path: str | None) -> None:
        """Emit an artifact_directory_path event with details on output path.

        Args:
            path (str | None): The path to the artifact directory.
                None if artifacts will not be saved.
        """
        self._emit(
            Event(
                GLOBAL_RUN_ID,
                self._get_timestamp(),
                payload=EventPayloadUnion(artifact_directory_path=path or ""),
            )
        )

    def emit_instruction_message(self, message: str) -> None:
        """Emit a message to the user with level INSTRUCTION.

        Args:
            message (str): Message contents
        """
        self._emit_user_message(message, level=MessageLevel.INSTRUCTION)

    def emit_info_message(self, message: str) -> None:
        """Emit a message to the user with level INFO.

        Args:
            message (str): Message contents
        """
        self._emit_user_message(message, level=MessageLevel.INFO)

    def emit_warning_message(self, message: str) -> None:
        """Emit a message to the user with level WARNING.

        Args:
            message (str): Message contents
        """
        self._emit_user_message(message, level=MessageLevel.WARNING)

    def emit_verbatim_message(self, message: str) -> None:
        """Emit a message to the user with level VERBATIM.

        Args:
            message (str): Message contents
        """
        self._emit_user_message(message, level=MessageLevel.VERBATIM)

    def emit_start_file_parsing(
        self, name: str, path: str, parent: Id | None = None
    ) -> Id:
        """Start parsing a file.

        This call must be matched with an emit_end call for the returned Id.

        Args:
            name (str): The name of the file being parsed.
            path (str): The path to the file being parsed.
            parent (Id | None): Parent of the event, if set. Defaults
                to the global run.

        Returns:
            Id: New Id for the parsing event, which must be ended explicitly.
        """
        id = self._new_id()
        self._emit(
            Event(
                id,
                self._get_timestamp(),
                parent=parent,
                starting=True,
                payload=EventPayloadUnion(
                    parsing_file=FileParsingPayload(name, path)
                ),
            )
        )
        return id

    def emit_program_start(
        self,
        command: str,
        args: list[str],
        environment: dict[str, str] | None = None,
        parent: Id | None = None,
        quiet_mode: bool = False,
    ) -> Id:
        """A program is starting execution.

        This call must be matched with an emit_program_termination
        call for the returned Id.

        Args:
            command (str): The command being executed.
            args (listsed to the command.
            environment (dict[str, str] | None):
                The environment passed to the command.  Defaults to None.
            parent (Id, | None): Parent for this event. Defaults
                to the global run.
            quiet_mode (bool): If set, instruct viewers to not display this command in output.
                Defaults to False.


        Returns:
            Id: New Id for the program event, which must be ended explicitly.
        """
        id = self._new_id()
        self._emit(
            Event(
                id,
                self._get_timestamp(),
                parent=parent,
                starting=True,
                payload=EventPayloadUnion(
                    program_execution=ProgramExecutionPayload(
                        command,
                        args,
                        environment or dict(),
                        quiet_mode,
                    )
                ),
            )
        )
        return id

    def emit_program_output(
        self,
        id: Id,
        content: str,
        stream: ProgramOutputStream,
        print_verbatim: bool = False,
    ) -> None:
        """A program produced output on a stream.

        Args:
            id (Id): An Id returned by emit_program_start.
            content (str): The string content of the output.
            stream (str): The stream that produced the content.
            print_verbatim (bool, optional): True only if the user
                requested that this command output be printed verbatim
                back to the console. Defaults to False.
        """
        self._emit(
            Event(
                id,
                self._get_timestamp(),
                payload=EventPayloadUnion(
                    program_output=ProgramOutputPayload(
                        content, stream, print_verbatim=print_verbatim
                    )
                ),
            )
        )

    def emit_program_termination(
        self, id: Id, return_code: int, error: str | None = None
    ) -> None:
        """A program terminated.

        Args:
            id (Id): An Id returned by emit_program_start.
            return_code (int): The return code for the program.
            error (str | None): If set, this program terminated
                with an error represented by this string message.
                Defaults to None.
        """
        self._emit(
            Event(
                id,
                self._get_timestamp(),
                ending=True,
                payload=EventPayloadUnion(
                    program_termination=ProgramTerminationPayload(return_code)
                ),
                error=error,
            )
        )

    def emit_test_file_loaded(
        self, entries: list[tests_json_file.TestEntry], file_path: str
    ) -> None:
        """Event with details of loading the tests.json file.

        Args:
            entries (list[tests_json_file.TestEntry]): Parsed file contents.
            file_path (str): Path to the tests.json file.
        """
        self._emit(
            Event(
                None,
                self._get_timestamp(),
                payload=EventPayloadUnion(
                    test_file_loaded=TestsJsonFilePayload(entries, file_path)
                ),
            )
        )

    def emit_test_selections(
        self,
        selections: selection_types.TestSelections,
    ) -> None:
        """Event with details of test selection.

        Args:
            selections (selection.TestSelections): The processed selections.
            threshold (float): The score threshold used for selection.
        """
        selected_scores = {
            item.name(): selections.best_score[item.name()]
            for item in selections.selected
        }
        selected_but_not_run_scores = {
            item.name(): selections.best_score[item.name()]
            for item in selections.selected_but_not_run
        }
        not_selected_scores = {
            name: score
            for name, score in selections.best_score.items()
            if name not in selected_scores
        }
        self._emit(
            Event(
                None,
                self._get_timestamp(),
                payload=EventPayloadUnion(
                    test_selections=TestSelectionPayload(
                        selected_scores,
                        not_selected_scores,
                        selected_but_not_run_scores,
                        selections.fuzzy_distance_threshold,
                    )
                ),
            )
        )

    def emit_event_group(
        self,
        name: str,
        parent: Id | None = None,
        queued_events: int | None = None,
        hide_children: bool = False,
    ) -> Id:
        """Create a new event group.

        The returned Id must be passed to a subsequent emit_end call.

        Args:
            name (str): Name of the group.
            parent (Id | None): Parent Id for the group. Defaults to the global run.
            queued_events (int | None): If set, expect this number
                of events to call this group their parent. Defaults to
                None.

        Returns:
            Id: New Id for the created group.
        """
        id = self._new_id()
        self._emit(
            Event(
                id,
                self._get_timestamp(),
                parent=parent,
                starting=True,
                payload=EventPayloadUnion(
                    event_group=EventGroupPayload(
                        name, queued_events, hide_children=hide_children
                    )
                ),
            )
        )
        return id

    def emit_build_start(self, targets: list[str]) -> Id:
        """A build process is starting.

        The returned Id must be passed to a subsequent emit_end call.

        Args:
            targets (list[str]): List of targets being built.

        Returns:
            Id: New Id for the build event.
        """
        id = self._new_id()
        self._emit(
            Event(
                id,
                self._get_timestamp(),
                starting=True,
                payload=EventPayloadUnion(build_targets=targets),
            )
        )
        return id

    def emit_test_group(self, test_count: int) -> Id:
        """A group of tests will be executed.

        The returned Id must be passed to a subsequent emit_end call.

        Args:
            test_count (int): The number of tests that will be executed.

        Returns:
            Id: New Id for the test group.
        """
        id = self._new_id()
        self._emit(
            Event(
                id,
                self._get_timestamp(),
                starting=True,
                payload=EventPayloadUnion(
                    test_group=TestGroupPayload(test_count)
                ),
            )
        )
        return id

    def emit_test_suite_started(
        self, name: str, hermetic: bool, parent: Id | None = None
    ) -> Id:
        """A test suite has started executing.

        The returned Id must be passed to a subsequent
        emit_test_suite_ended call.

        Args:
            name (str): The name of the test suite.
            hermetic (bool): True only if this suite is executed hermetically.
            parent (Id | None ): Parent event. Defaults to global run.

        Returns:
            Id: _description_
        """
        id = self._new_id()
        self._emit(
            Event(
                id,
                self._get_timestamp(),
                starting=True,
                parent=parent,
                payload=EventPayloadUnion(
                    test_suite_started=TestSuiteStartedPayload(name, hermetic)
                ),
            )
        )
        return id

    def emit_test_suite_ended(
        self, id: Id, status: TestSuiteStatus, message: str | None
    ) -> None:
        """A test suite has finished executing.

        Args:
            id (Id): The Id of the
            status (str): Status string for the test suite.
            message (str | None): Optional message
                describing the outcome of this suite.
        """
        self._emit(
            Event(
                id,
                self._get_timestamp(),
                ending=True,
                payload=EventPayloadUnion(
                    test_suite_ended=TestSuiteEndedPayload(status, message)
                ),
            )
        )

    def emit_enumerate_test_cases(
        self,
        test_name: str,
        test_case_names: list[str],
        command_template: str | None = None,
    ) -> None:
        id = self._new_id()
        self._emit(
            Event(
                id,
                self._get_timestamp(),
                payload=EventPayloadUnion(
                    enumerate_test_cases=EnumerateTestCasesPayload(
                        test_name, test_case_names, command_template
                    )
                ),
            )
        )
