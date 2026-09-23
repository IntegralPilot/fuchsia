#!/usr/bin/env fuchsia-vendored-python
# Copyright 2021 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Wraps a command so that its outputs are timestamp-fresh only if their contents change.

Every declared output is renamed with a temporary suffix in the command.
If the command succeeds, the temporary file is moved over the original declared
output if the output did not already exist or the contents are different.
This conditional move is done for every declared output that appears in the
arguments list.

This is intended to be used in build systems like Ninja that support `restat`:
treating unchanged outputs as up-to-date, which has the potential to prune
the action graph on-the-fly.

Assumptions:
  Output files can be whole shell tokens in the command's arguments.
    We also support filenames as lexical substrings in tokens like
    "--flag=out1,out2" or just "out1,out2".

  If x is a writeable path (output), then x.any_suffix is also writeable.

  If x is a writeable path (output), then dirname(x) is also writeable.

  Command being wrapped does not change behavior with the name of its output
  arguments.

If any of the above assumptions do not hold, then we recommend --disable
wrapping.
"""

import argparse
import dataclasses
import filecmp
import os
import shutil
import subprocess
import sys
import time
from collections.abc import Callable, Iterable, Sequence
from typing import Any

_SCRIPT_BASENAME = os.path.basename(__file__)


def files_match(file1: str, file2: str):
    """Compares two files, returns True if they both exist and match."""
    # filecmp.cmp does not invoke any subprocesses.
    return filecmp.cmp(file1, file2, shallow=False)


def ensure_file_exists(path):
    """Assert that a file exists, or wait for it to appear.

    It has been shown that some fault tolerance is needed
    regarding expecting files to be produced by a subprocess.

    Args:
      path: path to file that is expected to exist.

    Raises:
      FileNotFoundError if path does not exist, even after waiting.
    """
    for delay in (3, 6, 15):
        if os.path.exists(path):
            return

        # This branch should be highly unlikely, so it is allowed to be slow.
        # Either the original command failed to produce this file, or something
        # could be wrong with file system synchronization or delays.
        # Flush writes, sleep, try again.
        print(
            f"[{_SCRIPT_BASENAME}] Expected output file not found: {path} (Retrying after {delay}s ...)"
        )
        os.sync()
        time.sleep(delay)

    raise FileNotFoundError(
        f"[{_SCRIPT_BASENAME}] *** Expected output file not found: {path}"
    )


def retry_file_op_once_with_delay(
    fileop: Callable[[], Any], failmsg: str, delay: int
):
    """Insanity is doing the same thing and expecting a different result."""
    try:
        fileop()
    except FileNotFoundError:
        # one-time retry
        print(
            f"[{_SCRIPT_BASENAME}] {failmsg}  (Retrying once after {delay}s.)"
        )
        time.sleep(delay)
        fileop()
        # If this fails again, exception will be raised.


# Define here for ease of mocking.
def remove(path):
    """Remove, be it file or dir or link."""
    if os.path.isfile(path) or os.path.islink(path):
        return os.remove(path)
    elif os.path.isdir(path):
        return shutil.rmtree(path)


def move_if_identical(src: str, dest: str, verbose: bool = False) -> bool:
    """Moves src -> dest if their contents are identical.

    Args:
      src: source path
      dest: destination path
      verbose: if True, print what happened.

    Returns:
      True if move occurred (the two files matched),
      False if the files did not match, and thus, the src was removed.
    """
    ensure_file_exists(src)  # this was our backup
    if not os.path.exists(dest) or files_match(dest, src):
        if verbose:
            print(f"  === Cached: {dest}")
        shutil.move(src, dest)
        return True
    else:
        if verbose:
            print(f"  === Updated: {dest}")
        remove(src)
        return False


@dataclasses.dataclass
class TempFileTransform(object):
    """Represents a file name transform.

     At least temp_dir or suffix or basename_prefix must be non-blank.

    temp_dir: Write temporary files in here.
      If blank (default), paths are relative to working directory.
      We recommend writing to the same location as the intended output
      so that moves (on the same device) are extremely fast.
    suffix: Add this suffix to temporary files, e.g. ".tmp".
    basename_prefix: Add this prefix to the basename of the path.
      This can be a good choice over suffix when the underlying tool behavior
      is sensitive to the output file extension.
      Example: "foo/bar.txt", with prefix="tmp-" -> foo/tmp-bar.txt
    """

    temp_dir: str = ""
    suffix: str = ""
    basename_prefix: str = ""

    @property
    def valid(self):
        return self.temp_dir or self.suffix or self.basename_prefix

    def transform(self, path: str) -> str:
        return os.path.join(
            self.temp_dir,
            os.path.dirname(path),
            self.basename_prefix + os.path.basename(path) + self.suffix,
        )


def env_safe_command(command: Sequence[str]) -> Sequence[str]:
    """Automatically prefix a command with env if needed."""
    if command and "=" in command[0]:
        return ["/usr/bin/env"] + command
    return command


def record_existing_outputs(
    outputs: Iterable[str], transform: Callable[[str], str]
) -> dict[str, str]:
    """Map output file paths to their backup locations, using a transform.

    Args:
      outputs: collection of output files to backup.
      transform: name transformation to backup file name.

    Returns:
      Dictionary of outputs that already existed, mapped to their backup paths.
    """
    return {
        f: transform(f)
        for f in outputs
        if os.path.exists(f)  # accepts files or directories
    }


def backup_outputs(
    outputs: frozenset[str], tempfile_transform: TempFileTransform
) -> dict[str, str]:
    """Move pre-existing output files to backup locations.

    This is move, not copy, so this should be paired with some
    sort of restore operation, like `restore_if_unchanged()`.

    Returns:
      Dictionary of outputs that already existed, mapped to their backup paths.
    """
    outputs_to_restore = record_existing_outputs(
        outputs, tempfile_transform.transform
    )

    # mkdir when needed.
    if tempfile_transform.temp_dir:
        for f in outputs_to_restore.values():
            os.makedirs(os.path.dirname(f), exist_ok=True)

    for output, backup in outputs_to_restore.items():
        retry_file_op_once_with_delay(
            lambda: shutil.move(output, backup),
            f"Failed to backup {output} -> {backup}.",
            5,
        )

    return outputs_to_restore


def restore_if_unchanged(
    files_to_restore: dict[str, str], verbose: bool = False
) -> bool:
    """If backup contents match the new output, restore the backup.

    Otherwise remove the backup.

    Returns:
      True if there were any unrecoverable move errors.
    """
    move_err = False
    # TODO(fangism): This loop could be parallelized.
    for orig, backup in files_to_restore.items():
        try:
            retry_file_op_once_with_delay(
                lambda: move_if_identical(
                    src=backup, dest=orig, verbose=verbose
                ),
                f"Failed to restore {backup} -> {orig}.",
                5,
            )
        except FileNotFoundError as e:
            print(e)
            move_err = True
    return move_err


def restore_all(outputs_to_restore: dict[str, str]) -> bool:
    """Restores backups to their original location.

    This operation preserves modification timestamps.

    Returns:
      True if there were any unrecoverable move errors.
    """
    move_err = False
    # TODO(fangism): This loop could be parallelized.
    for orig, backup in outputs_to_restore.items():
        try:
            retry_file_op_once_with_delay(
                lambda: shutil.move(backup, orig),
                f"Failed to restore {backup} -> {orig}.",
                5,
            )
        except FileNotFoundError as e:
            print(e)
            move_err = True
    return move_err


@dataclasses.dataclass
class Action(object):
    """Represents a set of parameters of a single build action."""

    command: Sequence[str] = dataclasses.field(default_factory=list)
    outputs: frozenset[str] = dataclasses.field(default_factory=set)
    label: str = ""

    def run_cached(
        self, tempfile_transform: TempFileTransform, verbose: bool = False
    ) -> int:
        """Runs a modified command and conditionally moves outputs in-place.

        Args:
          tempfile_transform: describes transformation to temporary file name.
          verbose: If True, print substituted command before running it.
        """

        outputs_to_restore = backup_outputs(self.outputs, tempfile_transform)

        # Run the modified command.
        retval = subprocess.call(self.command)

        if retval != 0:
            # Old backups will appear stale after being restored.
            restore_all(outputs_to_restore)
            return retval

        # Otherwise command succeeded, so conditionally move outputs in-place.
        move_err = restore_if_unchanged(outputs_to_restore, verbose)
        if move_err:
            print("  *** Aborting due to previous error.")
            return 1

        return 0


def _main_arg_parser() -> argparse.ArgumentParser:
    """Construct the argument parser, called by main()."""
    parser = argparse.ArgumentParser(
        description="Wraps a GN action to preserve unchanged outputs",
        argument_default=[],
    )
    # label is only used for diagnostics
    parser.add_argument(
        "--label",
        type=str,
        default="",
        help="The wrapped target's label",
    )
    parser.add_argument(
        "--outputs",
        nargs="*",
        help="An action's declared outputs.  "
        + "The named output files will be backed-up before the command runs, "
        + "and compared against after the command finishes successfully.",
    )
    parser.add_argument(
        "--temp-suffix",
        type=str,
        default="",
        help="Suffix to use for temporary outputs",
    )
    parser.add_argument(
        "--temp-prefix",
        type=str,
        default="tmp-",
        help="Basename prefix to use for temporary outputs",
    )
    parser.add_argument(
        "--temp-dir",
        type=str,
        default="",
        help="Temporary directory for writing, can be relative to working directory or absolute.",
    )
    parser.add_argument(
        "--verbose",
        action="store_true",
        default=False,
        help="Print information about which outputs were renamed/cached.",
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        default=False,
        help="Show transformed command and exit.",
    )
    parser.add_argument(
        "--disable",
        action="store_false",
        dest="enable",
        default=True,
        help="If disabled, run the original command as-is.",
    )

    # Positional args are the command and arguments to run.
    parser.add_argument("command", nargs="*", help="The command to run")
    return parser


_MAIN_ARG_PARSER = _main_arg_parser()


def main(argv: Sequence[str]) -> int:
    args = _MAIN_ARG_PARSER.parse_args(argv)

    tempfile_transform = TempFileTransform(
        temp_dir=args.temp_dir,
        suffix=args.temp_suffix,
        basename_prefix=args.temp_prefix,
    )
    if not tempfile_transform.valid:
        raise ValueError(
            "Need either --temp-dir or --temp-suffix, but both are missing."
        )

    wrap = args.enable
    # Decided whether or not to wrap the action script.
    ignored_scripts = {
        # If the action is only copying or linking, don't bother wrapping.
        "ln",
        "cp",  # TODO: Could conditionally copy if different.
        "rsync",
    }
    script = args.command[0]
    if os.path.basename(script) in ignored_scripts:
        wrap = False

    command = env_safe_command(args.command)
    # If disabled, run the original command as-is.
    if not wrap:
        return subprocess.call(command)

    # Run a modified command that can leave unchanged outputs untouched.
    action = Action(
        command=command,
        outputs=set(args.outputs),
        label=args.label,
    )

    if args.dry_run:
        return 0

    return action.run_cached(
        tempfile_transform=tempfile_transform,
        verbose=args.verbose,
    )


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
