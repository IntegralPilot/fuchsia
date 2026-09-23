# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import asyncio
import shlex
import uuid
from contextlib import asynccontextmanager
from typing import AsyncGenerator, Literal

import build_dir
from ffx_cmd import FfxCmd
from fx_cmd import FxCmd

_DEFAULT_SERVER_STARTUP_TIMEOUT = 30
_CHECK_SERVER_POLLING_INTERVAL = 0.2


class PackageServingException(Exception):
    """This exception is raised on unsuccessful package serving attempts."""


class PackageServingCLIException(SystemExit, PackageServingException):
    """
    Like `PackageServingException`, but does not print a stack trace when uncaught.

    Used in the `ensure_running()` asynccontextmanager function, where concise error messages are
    more helpful than noisy stack traces for CLI developer tools.
    """


async def is_running() -> bool:
    """
    Checks whether a package server is currently running.

    Returns:
        bool: Whether a package server is currently running.
    """
    cmd = await FxCmd().start("is-package-server-running")
    result = await cmd.run_to_completion()
    return result.return_code == 0


async def wait_for_package_server(
    process: asyncio.subprocess.Process, timeout: float, interval: float
) -> bool:
    """
    Waits for a package server to come online by periodically polling.
    Exits early if the given package serving process terminates.

    Args:
        process (Process): The package serving process. If it terminates early, this function
            automatically returns False.
        timeout (float): The maximum amount of time (in seconds) to wait for the package server.
        interval (float): The delay (in seconds) between each package server availability check.

    Returns:
        bool: Whether the package serving process is available within `timeout` seconds.
    """

    async def poll_is_running(interval: float) -> Literal[True]:
        while not await is_running():
            await asyncio.sleep(interval)
        return True

    async def wait_for_process(
        process: asyncio.subprocess.Process,
    ) -> Literal[False]:
        await process.wait()
        return False

    async def wait_for_timeout(timeout: float) -> Literal[False]:
        await asyncio.sleep(timeout)
        return False

    # This task represents the True condition: When `fx is-package-server-running` is True.
    poll_task = asyncio.create_task(poll_is_running(interval))

    # This task represents a False condition: When `process` terminates unexpectedly.
    process_task = asyncio.create_task(wait_for_process(process))

    # This task represents a False condition: When `timeout` has been reached.
    timeout_task = asyncio.create_task(wait_for_timeout(timeout))

    # Race the three tasks, we use the value of whichever task completes first.
    all_tasks = [poll_task, process_task, timeout_task]
    finished_task, _ = await asyncio.wait(
        all_tasks,
        return_when=asyncio.FIRST_COMPLETED,
    )

    is_serving = finished_task.pop().result() if finished_task else False

    # Cleanup other tasks.
    for task in all_tasks:
        task.cancel()
        try:
            await task
        except asyncio.CancelledError:
            pass

    return is_serving


def is_package_repository_built() -> bool:
    """
    Checks whether the in-tree package repository is built.

    Returns:
        bool: Whether the in-tree package repository is built.
    """
    return (
        build_dir.get_build_directory()
        / "amber-files"
        / "repository"
        / "9.root.json"
    ).is_file()


async def get_arguments(name: str | None = None) -> tuple[str, ...]:
    """
    Gets the ffx command line arguments for starting a package server.

    Args:
        name (str | None): Optionally specify the main repository name for this server.

    Returns:
        tuple[str, ...]: A tuple of ffx arguments for package serving.
    """
    repo_name = name or f"tmp-{uuid.uuid4()}"
    out_dir = build_dir.get_build_directory()
    ffx_default_port = await FfxCmd().start(
        "config",
        "get",
        "repository.server.default_port",
    )
    stdout = (await ffx_default_port.run_to_completion()).stdout
    try:
        port = int(stdout.strip(' \t\n\r"'))
    except ValueError:
        port = 8083

    # LINT.IfChange
    return (
        "repository",
        "server",
        "start",
        "--foreground",
        "--address",
        f"[::]:{port}",
        "--repository",
        repo_name,
        "--repo-path",
        str(out_dir / "amber-files"),
        "--trusted-root",
        str(out_dir / "amber-files" / "repository" / "9.root.json"),
        "--alias",
        "fuchsia.com",
        "--alias",
        "chromium.org",
    )
    # LINT.ThenChange(//tools/devshell/serve)


async def start(
    name: str | None = None, timeout: float = _DEFAULT_SERVER_STARTUP_TIMEOUT
) -> asyncio.subprocess.Process:
    """
    Starts a package server.

    Args:
        name (str | None): Optionally specify the main repository name for this server.
        timeout (float): The maximum amount of time (in seconds) to wait for the package server.

    Returns:
        Process: An asyncio Process instance for the package server.
    """
    if not is_package_repository_built():
        raise PackageServingException("The package repository is not built!")

    repo_name = name or f"tmp-{uuid.uuid4()}"
    fx = FxCmd()
    ffx = FfxCmd()
    args = await get_arguments(repo_name)
    process = await asyncio.create_subprocess_exec(
        *fx.command_line(*ffx.command_line(*args)),
        stdout=asyncio.subprocess.DEVNULL,
        stderr=asyncio.subprocess.DEVNULL,
    )

    if await wait_for_package_server(
        process, timeout, _CHECK_SERVER_POLLING_INTERVAL
    ):
        return process

    errmsg: str
    if process.returncode is None:
        errmsg = f"The package server didn't start within {timeout}s."
        process.terminate()
        await process.wait()
        await stop(repo_name)
    else:
        errmsg = f"The package server exited with code {process.returncode}."
    raise PackageServingException(errmsg + f"\nCommand: {shlex.join(args)}")


async def stop(name: str | None = None) -> None:
    """
    Stops running package servers.

    Args:
        name (str | None): Optionally specify the repository name of the server to terminate.
            If omitted, all package servers will be stopped.
    """
    process = await FfxCmd().start(
        "repository",
        "server",
        "stop",
        *([name] if name else []),
    )
    await process.run_to_completion()


@asynccontextmanager
async def ensure_running() -> AsyncGenerator[None, None]:
    """
    Starts a package server for the duration of the context block, if one is not currently running.
    """
    if await is_running():
        yield
        return

    try:
        repo_name = f"tmp-{uuid.uuid4()}"
        print(
            "You do not seem to have a package server running. "
            + "A temporary one will be started for the duration of this execution."
        )
        server = await start(repo_name)
    except PackageServingException as e:
        raise PackageServingCLIException(
            f"Error: {e}\nError: Failed to start the temporary package server. "
            + "You may need to manually start one via `fx serve`."
        )

    try:
        yield
    finally:
        server.terminate()
        await server.wait()

        # Sending SIGTERM doesn't seem to clean up child processes, so use `ffx` to stop the package
        # server we just started for good measure.
        await stop(repo_name)
