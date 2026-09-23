# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

from dataclasses import dataclass, field

from zxdb_dap.models import AsyncTaskNode


@dataclass
class Thread:
    """Represents a thread in a target process."""

    id: int
    name: str = field(default="", compare=False)
    is_stopped: bool = field(default=False, compare=False)
    process: "Process | None" = field(default=None, repr=False, compare=False)

    def resume(self) -> None:
        """Marks this thread as not stopped, and ensures that any cached state on this thread and
        its process is cleared."""
        self._resume_internal(clear_process_cache=True)

    def _resume_internal(self, clear_process_cache: bool) -> None:
        """Marks the thread as resumed (is_stopped = False)."""
        self.is_stopped = False
        if clear_process_cache and self.process is not None:
            self.process.clear_cache()


@dataclass
class Process:
    """Represents a debugged target process."""

    id: int
    name: str = field(default="", compare=False)
    threads: dict[int, Thread] = field(
        default_factory=dict, repr=False, compare=False
    )

    # This is optional to differentiate between cases where there is a stopped process (all threads
    # are either suspended or blocked on some exception) and there is no async executor present in
    # any of the threads, and where any thread is running and the async backtrace cannot be
    # collected due to that fact.
    #
    # In the former situation, zxdb will still send us events to populate this with an empty task
    # list, indicating that there are no asynchronous tasks for this process. In the latter, it
    # means the process is not in the correct state to have produced any events yet and therefore
    # we cannot know yet whether or not there is an async executor present to produce tasks.
    async_backtrace: list[AsyncTaskNode] | None = field(
        default=None, repr=False, compare=False
    )

    def resume(self) -> None:
        """Clears our local cache and marks all threads in the process as resumed."""
        self.clear_cache()
        for t in self.threads.values():
            t._resume_internal(clear_process_cache=False)

    def clear_cache(self) -> None:
        """Cleans up cached process state that we've received from the debug adapter while all
        threads in this process were stopped. This is cleared any time a thread resumes from a
        stopped state."""
        self.async_backtrace = None

    @property
    def all_threads_stopped(self) -> bool:
        return bool(self.threads) and all(
            t.is_stopped for t in self.threads.values()
        )
