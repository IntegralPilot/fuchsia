# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import asyncio
import signal
import typing


def register_on_terminate_signal(fn: typing.Callable[[], None]) -> None:
    """Run a callable when a termination signal is caught by this program.

    When either SIGTERM or SIGINT is caught by this program, call
    the given function.

    Args:
        fn (typing.Callable[[typing.Any | None], None]): The function to call.
    """
    loop = asyncio.get_event_loop()
    for s in [signal.SIGTERM, signal.SIGINT]:
        loop.add_signal_handler(s, fn)


def unregister_all_termination_signals() -> None:
    """Unregister all existing termination signal handlers."""
    loop = asyncio.get_event_loop()
    for s in [signal.SIGTERM, signal.SIGINT]:
        loop.remove_signal_handler(s)
