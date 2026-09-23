#!/usr/bin/env fuchsia-vendored-python
# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Generic utilities for working with command lines and argparse.
"""

import argparse
import asyncio
import collections
import contextlib
import dataclasses
import datetime
import fcntl
import filecmp
import os
import platform
import re
import shlex
import shutil
import subprocess
import sys
from collections.abc import (
    Callable,
    Generator,
    Iterable,
    Iterator,
    Mapping,
    Sequence,
)
from pathlib import Path
from typing import (
    IO,
    Any,
    TextIO,
)

_SCRIPT_BASENAME = Path(__file__).name

# Local subprocess and remote environment calls need this when a
# command is prefixed with an X=Y environment variable.
_ENV = "/usr/bin/env"


def msg(text: str) -> None:
    print(f"[{_SCRIPT_BASENAME}] {text}")


def tfmt(t: datetime.datetime) -> str:
    return f"{t.hour:02.0f}:{t.minute:02.0f}:{t.second:02.0f}.{t.microsecond:06.0f}"


def tmsg(dt: datetime.datetime, text: str) -> None:
    print(f"[{tfmt(dt)}] {text}")


# Global control switch for time profiling using timer_cm.
_ENABLE_TIMERS = False


@contextlib.contextmanager
def timer_cm(text: str) -> Generator[Any, Any, Any]:
    """Times execution of a body of code for profiling performance.

    Globally enabled/disabled with _ENABLE_TIMERS.
    """
    try:
        if _ENABLE_TIMERS:
            start_time = datetime.datetime.now()
            tmsg(start_time, "start: " + text)
        yield
    finally:
        if _ENABLE_TIMERS:
            end_time = datetime.datetime.now()
            elapsed = end_time - start_time
            tmsg(end_time, "end  : " + text + f" (elapsed: {elapsed})")


def auto_env_prefix_command(command: list[str]) -> list[str]:
    if not command:
        return []
    if "=" in command[0]:
        # Commands that start with X=Y local environment variables
        # need to be run with 'env'.
        return [_ENV] + command
    return command


def bool_golang_flag(value: str) -> bool:
    """Interpret a Go-lang flag style boolean.

    See https://pkg.go.dev/flag

    This can be used as a 'type' parameter to 'ArgumentParser.add_argument()'
    """
    return {
        "1": True,
        "0": False,
        "t": True,
        "f": False,
        "true": True,
        "false": False,
    }[value.lower()]


def copy_preserve_subpath(src: Path, dest_dir: Path) -> None:
    """Like copy(), but preserves the relative path of src in the destination.

    Example: src='foo/bar/baz.txt', dest_dir='save/stuff/here' will result in
      'save/stuff/here/foo/bar/baz.txt'

    Args:
      src: path to file or dir, relative to working dir.
      dest_dir: root directory to copy to (can be absolute or relative to
        working dir).
    """
    assert (
        not src.is_absolute()
    ), f"source path to be copied should be relative, but got: {src}"

    if src.is_dir():
        # copytree() already creates intermediate parent dirs,
        # existing files in dest_dir will be overwritten.
        shutil.copytree(src, dest_dir / src, dirs_exist_ok=True)
    else:
        dest_subdir = dest_dir / src.parent
        dest_subdir.mkdir(parents=True, exist_ok=True)
        dest_file = dest_subdir / src.name
        if dest_file.exists():
            # If files are identical, don't copy again.
            # This helps if dest_file is not write-able.
            if not filecmp.cmp(src, dest_file, shallow=True):
                # files are different, keep the existing copy.
                msg(
                    f"[copy_preserve_subpath()] Warning: Files {src} and {dest_file} are different.  Not copying to the latter."
                )
            return

        shutil.copy2(src, dest_subdir)


# TODO: move this to library for abstract data operations
def partition_sequence(
    seq: Sequence[Any], sep: Any
) -> tuple[Sequence[Any], Any, Sequence[Any]]:
    """Similar to string.partition, but for arbitrary sequences.

    Args:
      seq: sequence of values.
      sep: a value to be sought as the separator

    Returns:
      if sep is not found, returns (the original sequence, None, [])
      otherwise returns a triple:
        the subsequence before the first occurrencee of sep,
        sep (the separator),
        the subsequence after the first occurrence of sep.
    """
    try:
        sep_position = seq.index(sep)
    except ValueError:
        return seq, None, []

    left = seq[:sep_position]
    remainder = seq[sep_position + 1 :]
    return left, sep, remainder


# TODO: move this to library for abstract data operations
def split_into_subsequences(
    seq: Iterable[Any], sep: Any
) -> Iterable[Sequence[Any]]:
    """Similar to string.split, but for arbitrary sequences.

    Args:
      seq: sequence of values.
      sep: a value to be sought as the separator

    Returns:
      sequence of subsequences between occurrences of the separator.
    """
    subseq: list[Any] = []
    for elem in seq:
        if elem == sep:
            yield subseq
            subseq = []
        else:
            subseq.append(elem)

    yield subseq


# TODO: move this to library for abstract data operations
def match_prefix_transform_suffix(
    text: str, prefix: str, transform: Callable[[str], str]
) -> str | None:
    """If text matches prefix, transform the text after the prefix.

    This can be useful for transforming command flags.

    Args:
       text: string to match and possibly transform.
       prefix: check if text starts with this string
       transform: function to apply to remainder of text after the
          matched prefix.

    Returns:
      Transformed text if prefix was matched, else None.
    """
    if not text.startswith(prefix):
        return None
    suffix = text[len(prefix) :]
    transformed = transform(suffix)
    return prefix + transformed


def command_quoted_str(command: Iterable[str]) -> str:
    return " ".join(shlex.quote(t) for t in command)


def flatten_comma_list(items: Iterable[str]) -> Iterable[str]:
    """Flatten ["a,b", "c,d"] -> ["a", "b", "c", "d"].

    This is useful for merging repeatable flags, which also
    have comma-separated values.

    Yields:
      Elements that were separated by commas, flattened over
      the original sequence..
    """
    for item in items:
        yield from item.split(",")


def remove_hash_comments(lines: Iterable[str]) -> Iterable[str]:
    for line in lines:
        if not line.startswith("#"):
            yield line


_C_COMMENT_RE = re.compile(
    r'//.*?$|/\*.*?\*/|\'(?:\\.|[^\\\'])*\'|"(?:\\.|[^\\"])*"',
    re.DOTALL | re.MULTILINE,
)


def remove_c_comments(text: str) -> str:
    """Remove C and C++ style comments from text.

    Adapted from https://stackoverflow.com/questions/241327/remove-c-and-c-comments-using-python
    """

    def replacer(match: re.Match[str]) -> str:
        s = match.group(0)
        if s.startswith("/"):
            # Note: space and not empty string (prevent token concatenation)
            # Alternatively, we could return an equivalent number of newlines
            # to preserve line numbers.
            return " "
        else:
            return s

    return _C_COMMENT_RE.sub(replacer, text)


def expand_response_files(
    command: Iterable[str], rspfiles: list[Path]
) -> Iterator[str]:
    """Expand response files in a command into tokens contained therein.

    Args:
      command: sequence of command-line tokens, may contain @response_files.
      rspfiles: (modify-by-reference) list of response files encountered during expansion.

    Yields:
      tokens, possibly expanded from response files.
    """
    for tok in command:
        if tok.startswith("@"):
            # rustc supports an alternative syntax: @shell:RSPFILE
            rspfile = Path(tok[1:].removeprefix("shell:"))
            rspfiles.append(rspfile)
            rsp_lines = rspfile.read_text().splitlines()
            # remove blanks and comments
            rsp_lines_stripped = (line.strip() for line in rsp_lines)
            filtered_lines = remove_hash_comments(
                line for line in rsp_lines_stripped if line
            )
            for line in filtered_lines:
                yield from expand_response_files(shlex.split(line), rspfiles)
        else:
            yield tok


def expand_fused_flags(
    command: Iterable[str], flags: Sequence[str]
) -> Iterable[str]:
    """Expand "fused" flags like '-I/foo/bar' into ('-I', '/foo/bar').

    argparse.ArgumentParser does not handle fused flags well,
    so expanding them first makes it easier to parse.
    Do not expect the intended tool to be able to parse these expanded flags.

    The reverse transformation is `fuse_expanded_flags()`.

    Args:
      command: sequence of command tokens.
      flags: flag prefixes that are to be separated from their values.

    Yields:
      command tokens, possibly expanded.
    """
    for tok in command:
        matched = False
        for prefix in flags:
            if tok.startswith(prefix) and len(tok) > len(prefix):
                # Separate value from flag to make it easier for argparse.
                yield prefix
                yield tok[len(prefix) :]
                matched = True
                break

        if not matched:
            yield tok


def fuse_expanded_flags(
    command: Iterable[str], flags: frozenset[str]
) -> Iterable[str]:
    """Turns flags like ('-D' 'foo') into '-Dfoo'.

    Reverse transformation of `expand_fused_flags()`.

    Args:
      command: sequence of command tokens.
      flags: flag prefixes that are to be joined with their values.

    Yields:
      command tokens, possibly fused.
    """
    prefix = None
    for tok in command:
        if prefix:
            yield prefix + tok
            prefix = None
            continue
        if tok in flags:
            # defer to next iteration to fuse
            prefix = tok
            continue

        yield tok


class StringSetAdd(argparse.Action):
    """An argparse.Action for adding an element to a set of strings."""

    def __init__(
        self,
        option_strings: Sequence[str],
        dest: str,
        nargs: Any | None = None,
        **kwargs: Any,
    ) -> None:
        if nargs is not None:
            raise ValueError("nargs not allowed")
        super().__init__(option_strings, dest, **kwargs)

    def __call__(
        self,
        parser: Any,
        namespace: Any,
        values: Any,
        option_string: str | None = None,
    ) -> None:
        collection = getattr(namespace, self.dest).copy()
        collection.add(values)
        setattr(namespace, self.dest, collection)


class StringSetRemove(argparse.Action):
    """An argparse.Action for removing an element from a set of strings."""

    def __init__(
        self,
        option_strings: Sequence[str],
        dest: str,
        nargs: Any | None = None,
        **kwargs: Any,
    ) -> None:
        if nargs is not None:
            raise ValueError("nargs not allowed")
        super().__init__(option_strings, dest, **kwargs)

    def __call__(
        self,
        parser: Any,
        namespace: Any,
        values: Any,
        option_string: str | None = None,
    ) -> None:
        collection = getattr(namespace, self.dest).copy()
        # special case: values == "all", clear entire set
        if values == "all":
            setattr(namespace, self.dest, set())
            return
        try:
            collection.remove(values)
        except KeyError:
            pass
        setattr(namespace, self.dest, collection)


def expand_paths_from_files(files: Iterable[Path]) -> Iterable[Path]:
    """Expand paths from files that list other paths."""
    for path_list in files:
        with open(path_list) as f:
            for line in f:
                stripped = line.strip()
                if stripped:
                    # Response files can list more than one path per line,
                    # separated by un-escaped whitespace.
                    for p in shlex.split(stripped):
                        yield Path(p.replace(" ", "\\ "))  # preserve escape


def read_config_file_lines(lines: Iterable[str]) -> dict[str, str]:
    """Parser for reading RBE config files.

    RBE config files are text files with lines of "VAR=VALUE"
    (ignoring whole-line #-comments and blank lines).
    Spec details can be found at:
    https://github.com/bazelbuild/reclient/blob/main/internal/pkg/rbeflag/rbeflag.go

    Args:
      lines: lines of config from a file

    Returns:
      dictionary of key-value pairs read from the config file.
    """
    result = {}
    for line in lines:
        stripped = line.strip()
        if stripped and not stripped.startswith("#"):
            key, sep, value = stripped.partition("=")
            if sep == "=":
                result[key] = value
    return result


def values_dict_to_config_value(
    values: dict[str, str], eq: str = "=", sep: str = ","
) -> str:
    """Return a string representation of a dictionary for config lines.

    It is assumed that the order among different keys is inconsequential,
    so we sort the output by keys for determinism.

    Args:
      values: arbitrary string dictionary of key-values
      eq: string to join keys and values
      sep: string to join key-value pairs

    Returns:
      Joined string representation, ordered by keys (deterministic).
    """
    # Keys are unique, so this effectively sorts (key, value) pairs
    # by key without regard to value.
    return sep.join(k + eq + v for k, v in sorted(values.items()))


def keyed_flags_to_values_dict(
    flags: Iterable[str], convert_type: Callable[[str], Any] | None = None
) -> Mapping[str, Sequence[str]]:
    """Convert a series of key[=value]s into a dictionary.

    All dictionary values are accumulated sequences of 'value's,
    so repeated keys like 'k=x' and 'k=y' will result in k:[x,y].
    It is up to the caller to interpret the values.

    This is useful for parsing and organizing tool flags like
    '-Cfoo=bar', '-Cbaz', '-Cquux=foo'.

    Args:
      flags: strings with the following forms:
        'key' -> key: (no value)
        'key=' -> key: "" (empty string)
        'key=value' -> key: value
      convert_type: type to convert string to, e.g. int, Path

    Returns:
      Strings dictionary of key and (possibly multiple) values.
    """
    partitions = (f.partition("=") for f in flags)
    # each partition is a tuple (left, sep, right)
    d: dict[str, list[str]] = collections.defaultdict(list)
    for key, sep, value in partitions:
        if sep == "=":
            d[key].append(convert_type(value) if convert_type else value)
        else:
            d[key]
    return d


def last_value_or_default(values: Sequence[str], default: str) -> str:
    if values:
        return values[-1]
    return default


def last_value_of_dict_flag(
    d: Mapping[str, Sequence[str]], key: str, default: str = ""
) -> str:
    """This selects the last value among repeated occurrences of a flag as a winner."""
    return last_value_or_default(d.get(key, []), default)


def filter_out_option_with_arg(
    args: Iterable[str], prefix_flag: str
) -> Iterable[str]:
    """Remove option and arguments that start with a given prefix.

    Example (with prefix_flag="--foo"):
      removes "--foo=bar" and ("--foo" "bar") from the token sequence.
      This also removes "--foo" at the end of the sequence.

    Args:
      args: command tokens
      prefix_flag: the flag that should be filtered out (along with its argument).

    Yields:
      a filtered command
    """
    fused_prefix_flag = prefix_flag + "="
    delete_optarg = False
    for arg in args:
        if delete_optarg:
            delete_optarg = False
            continue

        if arg == prefix_flag:
            delete_optarg = True
            continue

        if arg.startswith(fused_prefix_flag):
            continue

        yield arg


def strip_option_prefix(args: Iterable[str], prefix_flag: str) -> Iterable[str]:
    """Copy over flags, but strip away the prefix from a specific flag.

    Example (with prefix_flag="--foo"):
      Both "--foo=bar" and ("--foo" "bar") become just "bar".
      This also removes "--foo" at the end of the sequence.

    Args:
      args: command tokens
      prefix_flag: the flag that should be stripped away while preserving
        its argument.

    Yields:
      a filtered command
    """
    fused_prefix_flag = prefix_flag + "="
    keep_optarg = False
    for arg in args:
        if keep_optarg:
            keep_optarg = False
            yield arg
            continue

        if arg == prefix_flag:
            keep_optarg = True
            continue

        if arg.startswith(fused_prefix_flag):
            yield arg.removeprefix(fused_prefix_flag)
            continue

        yield arg


@dataclasses.dataclass
class ForwardedFlag(object):
    # The original name of the flag to match (including leading '-' or '--').
    name: str

    # If true, expect the following token to be the VALUE of --flag VALUE.
    has_optarg: bool

    # Substitute the original flag name with this name.
    # If this is "", then delete the flag name (still forward its optarg if
    # applicable)
    mapped_name: str


class FlagForwarder(object):
    """Separate and transform (rename) a set of flags and their values.

    Unlike using argparse.ArgumentParser, this forwarding approach
    preserves the left-to-right order in which flags appear.
    """

    def __init__(self, flag_mappings: Iterable[ForwardedFlag]):
        self._map = {m.name: m for m in flag_mappings}

    def sift(self, argv: Iterable[str]) -> tuple[Sequence[str], Sequence[str]]:
        """Sifts out known flags while transforming them.

        Args:
          argv: command tokens

        Returns:
          1) forwarded and transformed flags and args
          2) filtered out copy of argv
        """
        forwarded_flags = []
        filtered_argv = []

        next_token_is_optarg = False
        for tok in argv:
            if next_token_is_optarg:
                forwarded_flags.append(tok)
                next_token_is_optarg = False
                continue

            # match --flag without optarg
            flag = self._map.get(tok, None)
            if flag:
                if flag.mapped_name:
                    forwarded_flags.append(flag.mapped_name)
                next_token_is_optarg = flag.has_optarg
                continue

            # check for --flag=optarg
            left, sep, right = tok.partition("=")
            if sep == "=":
                left_flag = self._map.get(left, None)
                if left_flag:
                    prefix = (
                        left_flag.mapped_name + "="
                        if left_flag.mapped_name
                        else ""
                    )
                    forwarded_flags.append(prefix + right)
                    continue

            filtered_argv.append(tok)

        return forwarded_flags, filtered_argv


@contextlib.contextmanager
def chdir_cm(d: Path) -> Iterator[None]:
    """FIXME: replace with contextlib.chdir(), once Python 3.11 is default."""
    save_dir = os.getcwd()
    os.chdir(d)  # could raise OSError
    try:
        yield
    finally:
        os.chdir(save_dir)


def relpath(path: Path, start: Path) -> Path:
    """Relative path (using Path objects).

    Path.relative_to() requires self to be a subpath of the argument,
    but here, the argument is often the subpath of self.
    Hence, we need os.path.relpath() in the general case.

    Args:
      path (Path): target path
      start (Path): starting directory (required, unlike os.path.relpath)

    Returns:
      relative path
    """
    return Path(os.path.relpath(path, start=start))


def symlink_relative(dest: Path, src: Path) -> None:
    """Create a relative-path symlink from src to dest.

    Like os.symlink(), but using relative path.
    Any intermediate directories to src are automatically created.

    This is done without any os.chdir(), and can be done in parallel.

    Args:
      dest: target to link-to (not required to exist)
      src: new symlink path pointing to dest
    """
    if src.is_dir():
        src.rmdir()
    elif src.is_symlink() or src.is_file():
        src.unlink()
    src.parent.mkdir(parents=True, exist_ok=True)
    src.symlink_to(relpath(dest, start=src.parent))


def qualify_tool_path(path: Path) -> str:
    """Automatically prepend ./ to a tool path if needed.

    The relative path to a tool can sometimes be unqualified
    if it is in the current working directory, but to execute it
    it either needs to be in PATH or be relative-path qualified.
    Prepending './' makes it relative-path qualified.

    Args:
      path: path to a tool to be used in a subprocess.Popen.

    Returns:
      Relative-path qualified string because Path('./foo')
      is represented as Path('foo') (lossy).
    """
    if path.is_absolute():
        # absolute, no need to adjust
        return str(path)

    if path.parent == Path("."):
        # unqualified, needs './' prepended
        return os.path.join(".", str(path))

    # else already relative-path qualified
    return str(path)


def exec_relaunch(command: Sequence[str]) -> None:
    """Re-launches a command without returning.

    Works like an os.exec*() call, by replacing the current process with
    a new one.

    Tip: When mocking this function, give it a side-effect that
    raises a test-only Exception to quickly simulate an exit, without
    having to mock any other code that would normally not be reached.

    Args:
      command: command to execute.  Must start with an executable, specified
        with a relative or absolute path.

    Returns: (it does not return)
    """
    # TODO(https://fxbug.dev/42076617): use os.execv(), but figure out
    # how to get in-python print() of the new process to appear.
    # os.execv(command[0], command[1:])

    # Workaround: fork a subprocess
    sys.exit(subprocess.call(command))
    assert False, "exec_relaunch() should never return"


#####################################################################
class BlockingFileLock(object):
    """A minimal mutual exclusion lock (blocking)."""

    def __init__(self, lockfile: Path):
        self._lockfile: Path = lockfile
        self._lockfile_fd: int | None = None

    def _acquire(self) -> None:
        lockfile_fd = os.open(self._lockfile, os.O_RDWR | os.O_CREAT, 0o644)
        fcntl.flock(lockfile_fd, fcntl.LOCK_EX)  # Blocking
        self._lockfile_fd = lockfile_fd

    def _release(self) -> None:
        lockfile_fd = self._lockfile_fd
        if not lockfile_fd:
            return
        self._lockfile_fd = None
        fcntl.flock(lockfile_fd, fcntl.LOCK_UN)
        os.close(lockfile_fd)
        # Do not remove the lock file:
        # https://stackoverflow.com/questions/17708885/flock-removing-locked-file-without-race-condition

    def __enter__(self) -> "BlockingFileLock":
        self._acquire()
        return self

    def __exit__(self, exc_type: Any, exc_value: Any, traceback: Any) -> None:
        self._release()


#####################################################################
# The following code implements subprocess 'tee' behavior based on:
# https://stackoverflow.com/questions/2996887/how-to-replicate-tee-behavior-in-python-when-using-subprocess


class SubprocessResult(object):
    def __init__(
        self,
        returncode: int,
        stdout: Sequence[str] | None = None,  # lines
        stderr: Sequence[str] | None = None,  # lines
        # The process id may come in handy when looking for logs
        pid: int | None = None,
    ):
        self.returncode = returncode
        self.stdout = stdout or []
        self.stderr = stderr or []
        self.pid = pid if pid is not None else -1

    @property
    def stdout_text(self) -> str:
        return "\n".join(self.stdout)

    @property
    def stderr_text(self) -> str:
        return "\n".join(self.stderr)

    def verbose_returncode(self, label: str) -> int:
        """Prints any stdout/stderr, and returns only the exit code."""
        if self.stdout:
            print(f"---- {label}: stdout ----")
            for line in self.stdout:
                print(line)
        if self.stderr:
            print(f"---- {label}: stderr ----")
            for line in self.stderr:
                print(line)
        return self.returncode


async def _read_stream(
    stream: asyncio.StreamReader | None, callback: Callable[[bytes], None]
) -> None:
    if not stream:
        return
    while True:
        line = await stream.readline()
        if line:
            callback(line)
        else:
            break


async def _stream_subprocess(
    cmd: Sequence[str],
    stdin: IO[Any] | None = None,
    stdout: TextIO | None = None,
    stderr: TextIO | None = None,
    quiet: bool = False,
    **kwargs: Any,
) -> SubprocessResult:
    popen_kwargs: Any = {}
    if platform.system() == "Windows":
        platform_settings = {"env": os.environ}
    else:
        platform_settings = {}
        # default interpreter is sufficient: {"executable": "/bin/sh"}

    popen_kwargs.update(platform_settings)
    popen_kwargs.update(kwargs)

    cmd_str = command_quoted_str(cmd)
    p = await asyncio.create_subprocess_shell(
        cmd_str,
        stdin=stdin,
        stdout=asyncio.subprocess.PIPE,
        stderr=asyncio.subprocess.PIPE,
        limit=1024 * 1024,  # 1MB buffer for stdout/stderr StreamReaders
        **popen_kwargs,
    )
    pid = p.pid

    out_text: list[str] = []
    err_text: list[str] = []

    def tee(line: bytes, sink: list[str], pipe: TextIO) -> None:
        line_str = line.decode("utf-8").rstrip()
        sink.append(line_str)
        if not quiet:
            print(line_str, file=pipe)

    out_task = asyncio.create_task(
        _read_stream(p.stdout, lambda l: tee(l, out_text, stdout or sys.stdout))
    )
    err_task = asyncio.create_task(
        _read_stream(p.stderr, lambda l: tee(l, err_text, stderr or sys.stderr))
    )
    # Forward stdout, stderr while capturing them.
    await asyncio.wait([out_task, err_task])

    return SubprocessResult(
        returncode=await p.wait(),
        stdout=out_text,
        stderr=err_text,
        pid=pid,
    )


def subprocess_call(
    cmd: Sequence[str],
    stdin: IO[Any] | None = None,
    stdout: TextIO | None = None,
    stderr: TextIO | None = None,
    quiet: bool = False,
    **kwargs: Any,
) -> SubprocessResult:
    """Similar to subprocess.call(), but records stdout/stderr.

    Use this when interested in stdout/stderr.

    Args:
      cmd: command to execute
      stdin: input stream
      stdout: output stream
      stderr: error stream
      quiet: if True, suppress forwarding to sys.stdout/stderr.
      **kwargs: forwarded subprocess.Popen arguments.

    Returns:
      returncode, stdout (text), stderr (text)
    """
    loop = asyncio.get_event_loop()
    result = loop.run_until_complete(
        _stream_subprocess(
            cmd=cmd,
            stdin=stdin,
            stdout=stdout,
            stderr=stderr,
            quiet=quiet,
            **kwargs,
        )
    )
    return result


# end of subprocess_call section
#####################################################################


def subprocess_communicate(
    cmd: Sequence[str], input_text: str, **kwargs: Any
) -> SubprocessResult:
    """Pipes text through an external program.

    Unlike subprocess_call, this does not come with `tee` capability.

    Args:
      cmd: command to run (operates as a pipe).
      input_text: text to feed to command's stdin.
      kwargs: additional Popen() kwargs.

    Returns:
      SubprocessResult containing output and exit code.
    """
    p = subprocess.Popen(
        cmd,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        **kwargs,
    )
    out, err = p.communicate(input=input_text)
    return SubprocessResult(
        returncode=p.returncode,
        stdout=out.splitlines(False),
        stderr=err.splitlines(False),
        pid=p.pid,
    )
