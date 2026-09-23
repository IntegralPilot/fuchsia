#!/usr/bin/env fuchsia-vendored-python
# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Construct and execution remote actions with rewrapper.

This script is both a library and standalone binary for
driving rewrapper.

Usage:
  $0 [remote-options...] -- command...
"""

import argparse
import difflib
import errno
import filecmp
import functools
import hashlib
import multiprocessing
import os
import re
import stat
import subprocess
import sys
from collections.abc import Callable, Iterable, Iterator, Sequence
from pathlib import Path
from typing import (
    AbstractSet,
    Any,
    Optional,
)

import cl_utils
import depfile
import fuchsia
import output_leak_scanner
import remotetool
import textpb

_SCRIPT_BASENAME = Path(__file__).name

PROJECT_ROOT = fuchsia.project_root_dir()

# Needs to be computed with os.path.relpath instead of Path.relative_to
# to support testing a fake (test-only) value of PROJECT_ROOT.
PROJECT_ROOT_REL = cl_utils.relpath(PROJECT_ROOT, start=Path(os.curdir))

# This is a known path where remote execution occurs.
# This should only be used for workarounds as a last resort.
_REMOTE_PROJECT_ROOT = Path("/b/f/w")

# Extended attributes can be used to tell reproxy (filemetadata cache)
# that an artifact already exists in the CAS.
# TODO(https://fxbug.dev/42074138): use 'xattr' for greater portability.
_CAN_HAVE_XATTR = hasattr(os, "setxattr")

# Wrapper script to capture remote stdout/stderr, co-located with this script.
_REMOTE_LOG_SCRIPT = Path("build", "rbe", "log-it.sh")

_DETAIL_DIFF_SCRIPT = Path("build", "rbe", "detail-diff.sh")

_REPROXY_CFG = Path("build", "rbe", "fuchsia-reproxy.cfg")

_RECLIENT_ERROR_STATUS = 35
_RBE_SERVER_ERROR_STATUS = 45
_RBE_KILLED_STATUS = 137

_RETRIABLE_REWRAPPER_STATUSES = {
    _RECLIENT_ERROR_STATUS,
    _RBE_SERVER_ERROR_STATUS,
    _RBE_KILLED_STATUS,
}

_MAX_CONCURRENT_DOWNLOADS = 4


class FileSizeMismatchError(RuntimeError):
    pass


def init_from_main_once() -> int:
    # Support parallel downloads using forkserver method.
    multiprocessing.set_start_method("forkserver")
    return 0


def msg(text: str) -> None:
    print(f"[{_SCRIPT_BASENAME}] {text}")


def _path_or_default(path_or_none: Path | None, default: Path) -> Path:
    """Expressions like 'arg or DEFAULT' worked if arg is a string,
    but Path('') is not false-y in Python.

    Where one could write 'arg or DEFAULT', you should use
    '_path_or_default(arg, DEFAULT)' for clairty.
    """
    return default if path_or_none is None else path_or_none


def _write_lines_to_file(path: Path, lines: Iterable[str]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    contents = "\n".join(lines) + "\n"
    path.write_text(contents)


def _files_match(file1: Path, file2: Path) -> bool:
    """Compares two files, returns True if they both exist and match."""
    # filecmp.cmp does not invoke any subprocesses.
    return filecmp.cmp(file1, file2, shallow=False)


def _detail_diff(
    file1: Path, file2: Path, project_root_rel: Path | None = None
) -> cl_utils.SubprocessResult:
    command = [
        _path_or_default(project_root_rel, PROJECT_ROOT_REL)
        / _DETAIL_DIFF_SCRIPT,
        "-l=local",
        "-r=remote",
        file1,
        file2,
    ]
    return cl_utils.subprocess_call([str(x) for x in command], quiet=True)


def _detail_diff_filtered(
    file1: Path,
    file2: Path,
    maybe_transform_pair: (
        Callable[[Path, Path, Path, Path], bool] | None
    ) = None,
    project_root_rel: Path | None = None,
) -> cl_utils.SubprocessResult:
    """Show differences between filtered views of two files.

    Args:
      file1: usually a locally generated file
      file2: usually a remotely generated file
      maybe_transform_pair: function that returns True if it applies a transformation
        to produce a filtered views.
        Its path arguments are: original1, filtered1, original2, filtered2.
        It should return false if it does not transform the original files
        into filtered views.
        This function should decide whether or not to transform based on the
        original1 file name.
      project_root_rel: path to project root
    """
    filtered1 = Path(str(file1) + ".filtered")
    filtered2 = Path(str(file2) + ".filtered")
    if maybe_transform_pair is not None and maybe_transform_pair(
        file1, filtered1, file2, filtered2
    ):
        # Compare the filtered views of the files.
        return _detail_diff(filtered1, filtered2, project_root_rel)

    return _detail_diff(file1, file2, project_root_rel)


def _text_diff(file1: Path, file2: Path) -> cl_utils.SubprocessResult:
    """Capture textual differences to the result."""
    return cl_utils.subprocess_call(
        ["diff", "-u", str(file1), str(file2)], quiet=True
    )


def _files_under_dir(path: Path) -> Iterable[Path]:
    """'ls -R DIR' listing files relative to DIR."""
    yield from (
        Path(root, file).relative_to(path)
        for root, unused_dirs, files in os.walk(str(path))
        for file in files
    )


def _common_files_under_dirs(path1: Path, path2: Path) -> AbstractSet[Path]:
    files1 = set(_files_under_dir(path1))
    files2 = set(_files_under_dir(path2))
    return files1 & files2  # set intersection


def _expand_common_files_between_dirs(
    path_pairs: Iterable[tuple[Path, Path]],
) -> Iterable[tuple[Path, Path]]:
    """Expands two directories into paths to their common files.

    Args:
      path_pairs: sequence of pairs of paths to compare.

    Yields:
      stream of pairs of files to compare.  Within each directory group,
      common sub-paths will be in sorted order.
    """
    for left, right in path_pairs:
        for f in sorted(_common_files_under_dirs(left, right)):
            yield left / f, right / f


def resolved_shlibs_from_ldd(lines: Iterable[str]) -> Iterable[Path]:
    """Parse 'ldd' output.

    Args:
      lines: stdout text of 'ldd'

    Example line:
      librustc_driver-897e90da9cc472c4.so => /home/my_project/tools/rust/bin/../lib/librustc_driver.so (0x00007f6fdf600000)

    Should yield:
      /home/my_project/tools/rust/bin/../lib/librustc_driver.so
    """
    for line in lines:
        lib, sep, resolved = line.strip().partition("=>")
        if sep == "=>":
            yield Path(resolved.strip().split(" ")[0])


def host_tool_shlibs(executable: Path) -> Iterable[Path]:
    """Identify shared libraries of an executable.

    This only works on platforms with `ldd`.

    Yields:
      paths to non-system shared libraries
    """
    # TODO: do this once in the entire build, as early as GN time
    # TODO: support Mac OS using `otool -L`
    ldd_output = subprocess.run(
        ["ldd", str(executable)], capture_output=True, text=True
    )
    if ldd_output.returncode != 0:
        raise Exception(
            f"Failed to determine shared libraries of '{executable}'."
        )

    yield from resolved_shlibs_from_ldd(ldd_output.stdout.splitlines())


def host_tool_nonsystem_shlibs(executable: Path) -> Iterable[Path]:
    """Identify non-system shared libraries of a host tool.

    The host tool's shared libraries will need to be uploaded
    for remote execution.  (The caller should verify that
    the shared library paths fall under the remote action's exec_root.)

    Limitation: this works for only linux-x64 ELF binaries, but this is
    fine because only linux-x64 remote workers are available.

    Yields:
      paths to non-system shared libraries
    """
    for lib in host_tool_shlibs(executable):
        if any(str(lib).startswith(prefix) for prefix in ("/usr/lib", "/lib")):
            continue  # filter out system libs
        yield lib


def relativize_to_exec_root(path: Path, start: Path | None = None) -> Path:
    return cl_utils.relpath(path, start=(_path_or_default(start, PROJECT_ROOT)))


def _reclient_canonical_working_dir_components(
    subdir_components: Iterator[str],
) -> Iterable[str]:
    """Computes the path used by rewrapper --canonicalize_working_dir=true.

    The exact values returned are an implementation detail of reclient
    that is not reliable, so this should only be used as a last resort
    in workarounds.

    https://team.git.corp.google.com/foundry-x/re-client/+/refs/heads/master/internal/pkg/reproxy/action.go#177

    Args:
      subdir_components: a relative path like ('out', 'default', ...)

    Yields:
      Replacement path components like ('set_by_reclient', 'a', ...)
    """
    first = next(subdir_components, None)
    if first is None or first == "":
        return  # no components
    yield "set_by_reclient"
    for _ in subdir_components:
        yield "a"


def reclient_canonical_working_dir(build_subdir: Path) -> Path:
    new_components = list(
        _reclient_canonical_working_dir_components(iter(build_subdir.parts))
    )
    return Path(*new_components) if new_components else Path("")


def _file_lines_matching(path: Path, substr: str) -> Iterable[str]:
    with open(path) as f:
        for line in f:
            if substr in line:
                yield line


def _transform_file_by_lines(
    src: Path, dest: Path, line_transform: Callable[[str], str]
) -> None:
    with open(src) as f:
        new_lines = [line_transform(line.rstrip("\n")) for line in f]

    _write_lines_to_file(dest, new_lines)


def rewrite_file_by_lines_in_place(
    path: Path, line_transform: Callable[[str], str]
) -> None:
    _transform_file_by_lines(path, path, line_transform)


def rewrite_depfile(
    dep_file: Path, transform: Callable[[str], str], output: Path | None = None
) -> None:
    """Apply generic path transformations to a depfile.

    e.g. Relativize absolute paths.

    Args:
      dep_file: depfile to transform
      transform: f(str) -> str. change to apply to each path.
      output: if given, write to this new file instead overwriting
        the original depfile.
    """
    output = output or dep_file
    new_deps = depfile.transform_paths(dep_file.read_text(), transform)
    output.write_text(new_deps)


def _matches_file_not_found(line: str) -> bool:
    return "fatal error:" in line and "file not found" in line


def _matches_fail_to_dial(line: str) -> bool:
    return "Fail to dial" in line and "context deadline exceeded" in line


def _should_retry_remote_action(status: cl_utils.SubprocessResult) -> bool:
    """Heuristic for deciding when it is worth retrying rewrapper.

    Retry once under these conditions:
      35: reclient error (includes infrastructural issues or local errors)
      45: remote execution (server) error, e.g. remote blob download failure
      137: SIGKILL'd (signal 9) by OS.
        Reasons may include segmentation fault, or out of memory.

    Args:
      status: exit code, stdout, stderr of a completed subprocess.
    """
    if status.returncode == 0:  # success, no need to retry
        return False

    # Do not retry missing file errors, the user should address those.
    if any(_matches_file_not_found(line) for line in status.stderr):
        return False

    # Reproxy is now required to be running, so it is not possible
    # to "forget" to run reproxy (via fuchsia.REPROXY_WRAP).
    # It makes sense to retry rewrapper now.
    if any(_matches_fail_to_dial(line) for line in status.stderr):
        return True

    return status.returncode in _RETRIABLE_REWRAPPER_STATUSES


def _reproxy_log_dir() -> str | None:
    # Set by build/rbe/fuchsia-reproxy-wrap.sh.
    return os.environ.get("RBE_proxy_log_dir", None)


def _rewrapper_log_dir() -> str | None:
    # Set by build/rbe/fuchsia-reproxy-wrap.sh.
    return os.environ.get("RBE_log_dir", None)


def _rewrapper_platform_env() -> str | None:
    return os.environ.get("RBE_platform", None)


def _remove_prefix(text: str, prefix: str) -> str:
    # Like string.removeprefix() in Python 3.9+
    return text[len(prefix) :] if text.startswith(prefix) else text


def _remove_suffix(text: str, suffix: str) -> str:
    # Like string.removesuffix() in Python 3.9+
    return text[: -len(suffix)] if text.endswith(suffix) else text


class ReproxyLogEntry(object):
    def __init__(self, parsed_form: dict[str, Any]):
        self._raw = parsed_form

    @property
    def command(self) -> dict[str, Any]:
        return self._raw["command"][0]

    @property
    def identifiers(self) -> dict[str, Any]:
        return self.command["identifiers"][0]

    @property
    def execution_id(self) -> str:
        return self.identifiers["execution_id"][0].text.strip('"')

    @property
    def remote_metadata(self) -> dict[str, Any]:
        # remote_metadata does not always exist in the action log
        return self._raw.get("remote_metadata", [dict()])[0]

    @property
    def action_digest(self) -> str:
        return self.remote_metadata["action_digest"][0].text.strip('"')

    @property
    def output_file_digests(self) -> dict[Path, str]:  # path, hash/size
        d = self.remote_metadata.get("output_file_digests", dict())
        return {Path(k): v.text.strip('"') for k, v in d.items()}

    @property
    def output_directory_digests(self) -> dict[Path, str]:  # path, hash/size
        d = self.remote_metadata.get("output_directory_digests", dict())
        return {Path(k): v.text.strip('"') for k, v in d.items()}

    @property
    def completion_status(self) -> str:
        return self._raw["completion_status"][0].text

    def make_download_stub_info(
        self, path: Path, build_id: str
    ) -> Optional["DownloadStubInfo"]:
        type = "file"
        if path in self.output_file_digests:
            digest = self.output_file_digests[path]
        elif path in self.output_directory_digests:
            digest = self.output_directory_digests[path]
            type = "dir"
        else:
            # Named outputs are not required to exist remotely,
            # for example, crash-report dirs are only created under certain
            # conditions.  re-client treats all outputs as optional,
            # and does not consider missing outputs an error.
            return None
        return DownloadStubInfo(
            path=path,
            type=type,
            blob_digest=digest,
            action_digest=self.action_digest,
            build_id=build_id,
        )

    def make_download_stubs(
        self,
        files: Iterable[Path],
        dirs: Iterable[Path],
        build_id: str,
    ) -> dict[Path, "DownloadStubInfo"]:
        """Construct a map of paths to DownloadStubInfo.

        Args:
          files: files to create download stubs for.
          dirs: directories to create download stubs for.
          build_id: any string that corresponds to a unique build.

        Returns:
          Dictionary of paths to their stubs objects.
        """
        stubs = dict()
        for f in files:
            stub_info = self.make_download_stub_info(
                path=f,
                build_id=build_id,
            )
            if stub_info is not None:
                stubs[f] = stub_info

        for d in dirs:
            stub_info = self.make_download_stub_info(
                path=d,
                build_id=build_id,
            )
            if stub_info is not None:
                stubs[d] = stub_info
        return stubs

    @staticmethod
    def parse_action_log(log: Path) -> "ReproxyLogEntry":
        with open(log) as f:
            return ReproxyLogEntry._parse_lines(f.readlines())

    @staticmethod
    def _parse_lines(lines: Iterable[str]) -> "ReproxyLogEntry":
        """Parse data from a .rrpl (reproxy log) file.

        Args:
          lines: text from a rewrapper --action_log file

        Returns:
          ReproxyLogEntry object.
        """
        return ReproxyLogEntry(textpb.parse(lines))


def _diagnose_fail_to_dial(line: str) -> None:
    # Check connection to reproxy.
    if "Fail to dial" in line:
        print(line)
        msg(
            f""""Fail to dial" could indicate that reproxy is not running.
`reproxy` is launched automatically by `fx build`.
If you are manually running a remote-wrapped build step,
you may need to wrap your build command with:

  {fuchsia.REPROXY_WRAP} -- command...

'Proxy started successfully.' indicates that reproxy is running.
"""
        )


def _diagnose_rbe_permissions(line: str) -> None:
    # Check for permissions issues.
    if (
        "Error connecting to remote execution client: rpc error: code = PermissionDenied"
        in line
    ):
        print(line)
        msg(
            f"""You might not have permssion to access the RBE instance.
Contact fuchsia-build-team@google.com for support.
"""
        )


_REPROXY_ERROR_MISSING_FILE_RE = re.compile(
    "Status:LocalErrorResultStatus.*Err:stat ([^:]+): no such file or directory"
)


def _diagnose_missing_input_file(line: str) -> None:
    # Check for missing files.
    # TODO(b/201697587): surface this diagnostic from rewrapper
    match = _REPROXY_ERROR_MISSING_FILE_RE.match(line)
    if match:
        filename = match.group(1)
        print(line)
        if filename.startswith("out/"):
            description = "generated file"
        else:
            description = "source"
        msg(
            f"Possibly missing a local input file for uploading: {filename} ({description})"
        )


def _diagnose_reproxy_error_line(line: str) -> None:
    for check in (
        _diagnose_fail_to_dial,
        _diagnose_rbe_permissions,
        _diagnose_missing_input_file,
    ):
        check(line)


def analyze_rbe_logs(
    rewrapper_pid: int, action_log: Path | None = None
) -> None:
    """Attempt to explain failure by examining various logs.

    Prints additional diagnostics to stdout.

    Args:
      rewrapper_pid: process id of the failed rewrapper invocation.
      action_log: The .rrpl file from `rewrapper --action_log=LOG`,
        which is a proxy.LogRecord textproto.
    """
    # See build/rbe/fuchsia-reproxy-wrap.sh for the setup of these
    # environment variables.
    reproxy_logdir_str = _reproxy_log_dir()
    if not reproxy_logdir_str:
        return  # give up
    reproxy_logdir = Path(reproxy_logdir_str)

    rewrapper_logdir_str = _rewrapper_log_dir()
    if not rewrapper_logdir_str:
        return  # give up
    rewrapper_logdir = Path(rewrapper_logdir_str)

    # The reproxy.ERROR symlink is stable during a build.
    reproxy_errors = reproxy_logdir / "reproxy.ERROR"

    # Logs are named:
    #   rewrapper.{host}.{user}.log.{severity}.{date}-{time}.{pid}
    # The "rewrapper.{severity}" symlinks are useless because
    # the link destination is constantly being updated during a build.
    rewrapper_logs = rewrapper_logdir.glob(f"rewrapper.*.{rewrapper_pid}")
    if rewrapper_logs:
        msg("See rewrapper logs:")
        for log in rewrapper_logs:
            print("  " + str(log))
        print()  # blank line

    if not action_log:
        return
    if not action_log.is_file():
        return

    msg(f"Action log: {action_log}")
    # TODO: refactor to use remote_action.action_log_record, and avoid
    # re-parsing the same action log.
    rewrapper_info = ReproxyLogEntry.parse_action_log(action_log)
    execution_id = rewrapper_info.execution_id
    action_digest = rewrapper_info.action_digest
    print(f"  execution_id: {execution_id}")
    print(f"  action_digest: {action_digest}")
    print()  # blank line

    if not reproxy_errors.is_file():
        return

    msg(f"Scanning {reproxy_errors} for clues:")
    # Find the lines that mention this execution_id
    for line in _file_lines_matching(reproxy_errors, execution_id):
        _diagnose_reproxy_error_line(line)

    # TODO: further analyze remote failures in --action_log (.rrpl)


_RBE_DOWNLOAD_STUB_IDENTIFIER = "# RBE download stub"
_RBE_DOWNLOAD_STUB_HELP = "# run //build/rbe/dlwrap.py on this file to download"
_RBE_DOWNLOAD_STUB_SUFFIX = ".dl-stub"
_MAX_DOWNLOAD_STUB_SIZE_BYTES = 1024

# Filesystem extended attribute for digests.
# This should match the 'xattr_digest' value in build/rbe/fuchsia-reproxy.cfg.
_RBE_XATTR_HASH = "user.fuchsia.rbe.digests.sha256"

# Filesystem extended attribute for digests.
# This should match the value passed via '--unix_digest_hash_attribute_name' in
# build/bazel/templates/template.bazelrc.
_BAZEL_XATTR_HASH = "user.fuchsia.bazel.digest.sha256"

# Filesystem extended attribute for stubs
# This is a present/not-present marker that the file is actually just a download stub.
# The hash of the file to download is stored in _RBE_XATTR_HASH.
_RBE_XATTR_IS_STUB = "user.fuchsia.rbe.download_stub"

# This was previously set on output files, and needs to be removed
# as it's no longer the marker for a file being a download stub.
_RBE_OLD_XATTR_HASH = "user.fuchsia.rbe.digest.sha256"


def download_stub_backup_location(path: Path) -> Path:
    return Path(str(path) + _RBE_DOWNLOAD_STUB_SUFFIX)


def get_blob_digest(path: Path) -> str:
    contents = path.read_bytes()  # could be large
    readable_hash = hashlib.sha256(contents).hexdigest()
    length = len(contents)
    return f"{readable_hash}/{length}"


class DownloadStubFormatError(Exception):
    def __init__(self, message: str):
        super().__init__(message)


def download_temp_location(dest: Path) -> Path:
    return Path(str(dest) + ".download-tmp")


class DownloadStubInfo(object):
    """Contains infomation about a remotely stored artifact."""

    def __init__(
        self,
        path: Path,
        type: str,
        blob_digest: str,
        action_digest: str,
        build_id: str,
    ):
        """Private raw constructor.

        Use public methods like read_from_file().
        """
        self._path = path
        assert type in {"file", "dir"}
        self._type = type
        self._blob_digest = blob_digest
        assert "/" in blob_digest, "Expecting SHA256SUM/SIZE"
        self._action_digest = action_digest
        self._build_id = build_id

    @property
    def path(self) -> Path:
        return self._path

    @property
    def type(self) -> str:
        return self._type

    @property
    def blob_digest(self) -> str:
        return self._blob_digest

    def __eq__(self, other: object) -> bool:
        if not isinstance(other, DownloadStubInfo):
            return False
        return (
            self.path == other.path
            and self.type == other.type
            and self.blob_digest == other.blob_digest
            and self._build_id == other._build_id
        )

    def _write(self, output_abspath: Path) -> None:
        """Writes download stub contents to file.

        This unconditionally overwrites any existing stub file.
        """
        assert output_abspath.is_absolute(), f"got: {output_abspath}"
        lines = [
            _RBE_DOWNLOAD_STUB_IDENTIFIER,
            _RBE_DOWNLOAD_STUB_HELP,
            f"path={self.path}",
            f"type={self.type}",
            f"blob_digest={self.blob_digest}",  # hash/size
            f"action_digest={self._action_digest}",
            f"build_id={self._build_id}",
        ]
        _write_lines_to_file(output_abspath, lines)

    def create(
        self,
        working_dir_abs: Path,
        dest: Path | None = None,
        use_xattr: bool = _CAN_HAVE_XATTR,
    ) -> None:
        """Create a download stub file.

        The stub file will be backed-up to PATH.dl-stub when it is 'downloaded'.

        Args:
          working_dir_abs: absolute path to the working dir, to which
            all referenced paths are relative.  This is passed so that
            this operation does not depend on os.curdir.
          dest: (optional) alternate location to write download stub.
          use_xattr: if true, set extended attributes on the stub file.
        """
        assert working_dir_abs.is_absolute()
        path = working_dir_abs / (dest or self.path)
        path.parent.mkdir(parents=True, exist_ok=True)
        self._write(path)

        if use_xattr:
            # Signal to the next reproxy invocation that the object already
            # exists in the CAS and does not need to be uploaded.
            os.setxattr(
                path,
                _RBE_XATTR_IS_STUB,
                "true".encode(),
            )

    @staticmethod
    def read_from_file(stub: Path) -> "DownloadStubInfo":
        with open(stub) as f:
            first = f.readline()
            if first.rstrip() != _RBE_DOWNLOAD_STUB_IDENTIFIER:
                raise DownloadStubFormatError(
                    f"File {stub} is not a download stub."
                )
            lines = f.readlines()  # read the remainder

        variables = {}
        for line in lines:
            key, sep, value = line.partition("=")
            if sep != "=":
                continue
            variables[key] = value.rstrip()  # without trailing '\n'

        return DownloadStubInfo(
            path=Path(variables["path"]),
            type=variables["type"],
            blob_digest=variables["blob_digest"],
            action_digest=variables["action_digest"],
            build_id=variables["build_id"],
        )

    def download(
        self,
        downloader: remotetool.RemoteTool,
        working_dir_abs: Path,
        dest: Path | None = None,
        use_xattr: bool = _CAN_HAVE_XATTR,
    ) -> cl_utils.SubprocessResult:
        """Retrieves the file or dir referenced by the stub.

        Reads the stub info from file.
        Downloads to a temporarily location, and then moves it
        into place when completed.
        The stub file is backed up to "<name>.dl-stub".
        Permissions on the stub file are applied to the downloaded result.

        This interface is suitable when it is guaranteed that
        nothing else will try to download the same file concurrently,
        e.g. the build system guarantees that action outputs
        are exclusive.  If exclusion is needed, use download_from_stub_path().

        Args:
          downloader: 'remotetool' instance to use to download.
          working_dir_abs: working dir.
          dest: (optional) path to download to, overriding self.path.

        Returns:
          subprocess results, including exit code.
        """
        # self.path assumes that the working dir == build subbdir
        dest_abs = working_dir_abs / (dest or self.path)
        temp_dl = download_temp_location(dest_abs)

        downloader_fn = {
            "dir": downloader.download_dir,
            "file": downloader.download_blob,
        }[self.type]

        status = downloader_fn(
            path=temp_dl,
            digest=self._blob_digest,
            cwd=working_dir_abs,
        )

        if status.returncode == 0:  # download complete, success
            # Reflect the mode/permissions from stub to the real file.
            temp_dl.chmod(dest_abs.stat().st_mode)

            if is_download_stub_file(dest_abs, use_xattr=use_xattr):
                # Backup the download stub.  This preserves the xattr.
                dest_abs.rename(download_stub_backup_location(dest_abs))

            temp_dl.rename(dest_abs)

        return status


def _file_starts_with(path: Path, text: str) -> bool:
    with open(path, "rb") as f:
        # read only a small number of bytes to compare
        return os.pread(f.fileno(), len(text), 0) == text.encode()


def is_download_stub_file(
    path: Path, use_xattr: bool = _CAN_HAVE_XATTR
) -> bool:
    """Returns true if the path points to a download stub."""
    try:
        if use_xattr:
            return _RBE_XATTR_IS_STUB in os.listxattr(path)
        else:
            # Download stubs are small text files (~300 bytes).
            # Fast check: files larger than _MAX_DOWNLOAD_STUB_SIZE_BYTES or
            # directories cannot be stubs.
            st = os.stat(path)
            if st.st_size > _MAX_DOWNLOAD_STUB_SIZE_BYTES or stat.S_ISDIR(
                st.st_mode
            ):
                return False
            return _file_starts_with(path, _RBE_DOWNLOAD_STUB_IDENTIFIER)
    except OSError:
        return False


def undownload(path: Path, use_xattr: bool = _CAN_HAVE_XATTR) -> bool:
    """If a backup download stub exists, restore it (for debugging).

    Args:
      path: path to an artifact that might have a backup download stub.
        The file at this location is not required to exist before calling.

    Returns:
      True if a stub was moved back.
    """
    stub_path = download_stub_backup_location(path)
    if stub_path.exists() and is_download_stub_file(
        stub_path, use_xattr=use_xattr
    ):
        stub_path.rename(path)
        return True
    return False


def path_to_download_stub(
    stub_path: Path, use_xattr: bool = _CAN_HAVE_XATTR
) -> DownloadStubInfo | None:
    if not is_download_stub_file(stub_path, use_xattr=use_xattr):
        return None

    return DownloadStubInfo.read_from_file(stub_path)


def download_file_to_path(
    downloader: remotetool.RemoteTool,
    working_dir_abs: Path,
    path: Path,
    blob_digest: str,
    action_digest: str | None = None,
    use_xattr: bool = _CAN_HAVE_XATTR,
) -> cl_utils.SubprocessResult:
    dl_stub_info = DownloadStubInfo(
        path=path,
        type="file",
        blob_digest=blob_digest,
        action_digest=action_digest or "not-needed",
        build_id="not-important",
    )
    path.parent.mkdir(parents=True, exist_ok=True)
    return dl_stub_info.download(
        downloader=downloader,
        working_dir_abs=working_dir_abs,
        use_xattr=use_xattr,
    )


def download_from_stub_path(
    stub_path: Path,
    downloader: remotetool.RemoteTool,
    working_dir_abs: Path,
    verbose: bool = False,
    use_xattr: bool = _CAN_HAVE_XATTR,
) -> cl_utils.SubprocessResult:
    """Possibly downloads a file over a stub link.

    This interface is suitable for external programs to
    invoke because it safely handles concurrent duplicate
    download requests via file-lock.

    Args:
      stub_path: is a path to a possible download stub, relative to the
        current working dir.
      downloader: remotetool instance used to download.
      working_dir_abs: current working dir.
      verbose: if True, print extra debug information.

    Returns:
      download exit status, or 0 if there is nothing to download.
    """
    # If the directory where the stub resides is not writable, we cannot possibly
    # replace the stub with a downloaded file, and the artifact is likely already
    # a real file (or we cannot modify it). In this case, we can gracefully no-op.
    stub_dir = (working_dir_abs / stub_path.parent).resolve()
    if stub_dir.exists() and not os.access(stub_dir, os.W_OK):
        if verbose:
            msg(
                f"    {stub_path} resides in a read-only directory; assuming it is already a real file"
            )
        return cl_utils.SubprocessResult(0)

    ok_result = cl_utils.SubprocessResult(0)
    if not stub_path.exists():
        msg(f"Ignoring request to download nonexistent stub: {stub_path}")
        return ok_result

    # Quick check without lock: regular files will never become stubs.
    # Avoid creating lock files and directories for non-stub files.
    if not is_download_stub_file(stub_path, use_xattr=use_xattr):
        if verbose:
            msg(
                f"    {stub_path} already exists as a normal file (not downloading)"
            )
        return ok_result

    # Use lock file to safely handle potentially concurrent
    # download requests to the same artifact.
    # If there are concurrent requests to download the same stub,
    # the lock will be granted to one caller, while the other waits.
    # Locate the corresponding lock file in a separate directory
    # from the real file because there's no telling what tools may attempt
    # to glob files from a given directory.  This avoids polluting
    # directories in the build workspace during a build, which happened
    # in b/394155554.
    lock_file = Path(".dl-locks") / stub_path
    lock_file.parent.mkdir(parents=True, exist_ok=True)
    with cl_utils.BlockingFileLock(lock_file) as lock:
        if not stub_path.exists():
            msg(f"Ignoring request to download nonexistent stub: {stub_path}")
            return ok_result

        stub_info = path_to_download_stub(stub_path, use_xattr=use_xattr)
        if not stub_info:
            if verbose:
                msg(
                    f"    {stub_path} already exists as a normal file (not downloading)"
                )
            return ok_result

        if verbose:
            msg(f"Downloading {stub_path}")

        return stub_info.download(
            downloader=downloader,
            working_dir_abs=working_dir_abs,
            dest=stub_path,
            use_xattr=use_xattr,
        )


_FORWARDED_LOCAL_FLAGS = cl_utils.FlagForwarder(
    [
        cl_utils.ForwardedFlag(
            name="--local-only",
            has_optarg=True,
            mapped_name="",
        ),
    ]
)


class RemoteAction(object):
    """RemoteAction represents a command that is to be executed remotely."""

    def __init__(
        self,
        rewrapper: Path,
        command: Sequence[str],
        local_only_command: Sequence[str] | None = None,
        options: Sequence[str] | None = None,
        exec_root: Path | None | None = None,
        working_dir: Path | None | None = None,
        cfg: Path | None | None = None,
        exec_strategy: str | None | None = None,
        inputs: Sequence[Path] | None = None,
        input_list_paths: Sequence[Path] | None = None,
        output_files: Sequence[Path] | None = None,
        output_dirs: Sequence[Path] | None = None,
        platform: str | None = None,
        disable: bool = False,
        verbose: bool = False,
        save_temps: bool = False,
        label: str | None = None,
        remote_log: str = "",
        fsatrace_path: Path | None = None,
        remote_fsatrace_method: str | None = None,
        diagnose_nonzero: bool = False,
        compare_with_local: bool = False,
        check_determinism: bool = False,
        determinism_attempts: int = 1,
        miscomparison_export_dir: Path | None = None,
        post_remote_run_success_action: Callable[[], int] | None = None,
        remote_debug_command: Sequence[str] | None = None,
        use_xattr: bool = _CAN_HAVE_XATTR,
    ):
        """RemoteAction constructor.

        Args:
          rewrapper: path to rewrapper binary
          options: rewrapper options (not already covered by other parameters)
          command: the command to execute remotely
          local_only_command: local command equivalent to the remote command.
            Only pass this if the local and remote commands differ.
          cfg: rewrapper config file (optional)
          exec_strategy: rewrapper --exec_strategy (optional)
          exec_root: an absolute path location that is parent to all of this
            remote action's inputs and outputs.
          working_dir: directory from which command is to be executed.
            This must be a sub-directory of 'exec_root'.
          inputs: inputs needed for remote execution, relative to the current working dir.
          input_list_paths: files that list additional inputs to remote actions,
            where paths are relative to the current working dir.
          output_files: files to be fetched after remote execution, relative to the
            current working dir.
          output_dirs: directories to be fetched after remote execution, relative to the
            current working dir.
          platform: rewrapper --platform, containing remote execution parameters
          disable: if true, execute locally.
          verbose: if true, print more information about what is happening.
          compare_with_local: if true, also run locally and compare outputs.
          check_determinism: if true, compare outputs of two local executions.
          determinism_attempts: For check_determinism, re-run and compare N times.
          miscomparison_export_dir: copy unexpected differences found by
            --compare or --check-determinism to this directory, if given.
          save_temps: if true, keep around temporarily generated files after execution.
          label: build system identifier, for diagnostic messages.
          remote_log: "" means disabled.  Any other value, remote logging is
            enabled, and stdout/stderr of the remote execution is captured
            to a file and downloaded.
            if "<AUTO>":
              if there is at least one remote output file:
                name the log "${output_files[0]}.remote-log"
              else:
                name the log "rbe-action-output.remote-log"
            else:
              use the given name appended with ".remote-log"
          fsatrace_path: Given a path to an fsatrace tool
              (located under exec_root), this will wrap the remote command
              to trace and log remote file access.
              if there is at least one remote output file:
                the trace name is "${output_files[0]}.remote-fsatrace"
              else:
                the trace name "rbe-action-output.remote-fsatrace"
          remote_fsatrace_method:
            * "prefix": prepends the command with fsatrace directly.
              This is good for generic remote actions.
            * "wrap": writes the prefix command to a remote_wrapper script.
              This is useful for when prefixing would interfere with
              reclient input processing.
          diagnose_nonzero: if True, attempt to examine logs and determine
            a cause of error.
          remote_debug_command: if True, run different command remotely instead of
            the original command, for debugging the remote inputs setup.
          use_xattr: if True, use xattrs to record artifact digests.
        """
        self._rewrapper = rewrapper
        self._config = cfg  # can be None
        self._exec_strategy = exec_strategy  # can be None
        self._save_temps = save_temps
        self._diagnose_nonzero = diagnose_nonzero
        self._working_dir = (working_dir or Path(os.curdir)).absolute()
        self._exec_root = (exec_root or PROJECT_ROOT).absolute()
        # Parse and strip out --remote-* flags from command.
        # Hide --local-only options from remote execution, so they don't affect
        # the command_digest, but then add them back though --local_wrapper.
        (
            self._local_only_flags,
            self._remote_only_command,
        ) = _FORWARDED_LOCAL_FLAGS.sift(command)
        self._remote_disable = disable
        self._verbose = verbose
        self._label = label
        self._compare_with_local = compare_with_local
        self._check_determinism = check_determinism
        self._determinism_attempts = determinism_attempts
        self._miscomparison_export_dir = miscomparison_export_dir
        self._options = options or []
        self._post_remote_run_success_action = post_remote_run_success_action
        self._remote_debug_command = remote_debug_command or []
        self._use_xattr = use_xattr

        # When comparing local vs. remote, force exec_strategy=remote
        # to eliminate any unintended local execution cases.
        if self.compare_with_local and self.exec_strategy != "remote":
            self.vmsg("Notice: forcing exec_strategy=remote for --compare")
            self._exec_strategy = "remote"

        # By default, the local and remote commands match, but there are
        # circumstances that require them to be different.  It is the caller's
        # responsibility to ensure that they produce consistent results
        # (which can be verified with --compare [compare_with_local]).
        self._local_only_command = local_only_command or command

        # platform is handled by specially, by merging with cfg.
        self._platform = platform

        # Detect some known rewrapper options
        (
            self._rewrapper_known_options,
            _,
        ) = _REWRAPPER_ARG_PARSER.parse_known_args(self._options)

        # Inputs and outputs parameters are relative to current working dir,
        # but they will be relativized to exec_root for rewrapper.
        # It is more natural to copy input/output paths that are relative to the
        # current working directory.
        self._inputs: list[Path] = list(inputs) if inputs else []
        self._input_list_paths = input_list_paths or []
        self._output_files: list[Path] = (
            list(output_files) if output_files else []
        )
        self._output_dirs = output_dirs or []

        # Amend input/outputs when logging remotely.
        self._remote_log_name = self._name_remote_log(remote_log)
        if self._remote_log_name:
            # These paths are relative to the working dir.
            self._output_files.append(self._remote_log_name)
            self._inputs.append(self._remote_log_script_path)

        self._remote_fsatrace_method = remote_fsatrace_method or "prefix"

        # TODO(https://fxbug.dev/42076379): support remote tracing from Macs
        self._fsatrace_path = fsatrace_path  # relative to working dir
        if self._fsatrace_path == Path(""):  # then use the default prebuilt
            self._fsatrace_path = self.exec_root_rel / fuchsia.FSATRACE_PATH
        if self._fsatrace_path:
            self._inputs.extend([self._fsatrace_path, self._fsatrace_so])
            self._output_files.append(self._fsatrace_remote_trace)
            if self._remote_fsatrace_method == "wrap":
                self._inputs.append(self._fsatrace_remote_wrapper)

        self._cleanup_files: list[Path] = []

    @property
    def verbose(self) -> bool:
        return self._verbose

    def vmsg(self, text: str) -> None:
        if self.verbose:
            msg(text)

    @property
    def label(self) -> str | None:
        return self._label

    @property
    def exec_root(self) -> Path:
        return self._exec_root

    @property
    def exec_strategy(self) -> str | None:
        return self._exec_strategy

    @property
    def config(self) -> Path | None:
        return self._config

    @property
    def _default_auxiliary_file_basename(self) -> str | None:
        # Return a str instead of Path because most callers will want to
        # append a suffix (str + str).
        if self._output_files:
            return str(self._output_files[0])
        else:  # pick something arbitrary, but deterministic
            return "rbe-action-output"

    def _name_remote_log(self, remote_log: str) -> Path | None:
        if remote_log == "<AUTO>":
            if self._default_auxiliary_file_basename is None:
                raise ValueError(
                    "self._default_auxiliary_file_basename is None"
                )
            return Path(self._default_auxiliary_file_basename + ".remote-log")

        if remote_log:
            return Path(remote_log + ".remote-log")

        return None

    @property
    def _remote_log_script_path(self) -> Path:
        return self.exec_root_rel / _REMOTE_LOG_SCRIPT

    @property
    def _fsatrace_local_trace(self) -> Path:
        if self._default_auxiliary_file_basename is None:
            raise ValueError("self._default_auxiliary_file_basename is None")
        return Path(self._default_auxiliary_file_basename + ".local-fsatrace")

    @property
    def _fsatrace_remote_trace(self) -> Path:
        if self._default_auxiliary_file_basename is None:
            raise ValueError("self._default_auxiliary_file_basename is None")

        return Path(self._default_auxiliary_file_basename + ".remote-fsatrace")

    @property
    def _fsatrace_remote_wrapper(self) -> Path:
        if self._default_auxiliary_file_basename is None:
            raise ValueError("self._default_auxiliary_file_basename is None")

        return Path(self._default_auxiliary_file_basename + ".fsatrace.sh")

    @property
    def _fsatrace_so(self) -> Path:
        if self._fsatrace_path is None:
            raise ValueError("self._fsatrace_path is None")
        # fsatrace needs the corresponding .so to work
        return self._fsatrace_path.with_suffix(".so")

    @property
    def _action_log(self) -> Path:
        # The --action_log is a single entry of the cumulative log
        # of remote actions in the reproxy_*.rrpl file.
        # The information contained in this log is the same,
        # but is much easier to find than in the cumulative log.
        # The non-reduced .rpl format is also acceptable.
        if self._default_auxiliary_file_basename is None:
            raise ValueError("self._default_auxiliary_file_basename is None")
        return Path(self._default_auxiliary_file_basename + ".rrpl")

    @property
    def compare_with_local(self) -> bool:
        return self._compare_with_local

    @property
    def local_only_command(self) -> Sequence[str]:
        """This is the original command that would have been run locally.
        All of the --remote-* flags have been removed at this point.
        """
        return cl_utils.auto_env_prefix_command(list(self._local_only_command))

    @property
    def local_only_flags(self) -> Sequence[str]:
        return self._local_only_flags

    @property
    def local_wrapper_text(self) -> str:
        local_flags_text = cl_utils.command_quoted_str(self.local_only_flags)
        return f"""#!/bin/sh
base="$(basename $0)"
cmd=( "$@" {local_flags_text} )
# echo "[$base]:" "${{cmd[@]}}"
exec "${{cmd[@]}}"
"""

    @property
    def local_wrapper_filename(self) -> Path:
        return self._output_files[0].with_suffix(".local.sh")

    @property
    def remote_only_command(self) -> Sequence[str]:
        return cl_utils.auto_env_prefix_command(list(self._remote_only_command))

    def show_local_remote_command_differences(self) -> None:
        if self._local_only_command == self._remote_only_command:
            return  # no differences to show

        self.vmsg("local vs. remote command differences:")
        diffs = difflib.unified_diff(
            [tok + "\n" for tok in self.local_only_command],
            [tok + "\n" for tok in self.remote_only_command],
            fromfile="local.command",
            tofile="remote.command",
        )
        for l in diffs:
            self.vmsg(l)

    @property
    def remote_debug_command(self) -> Sequence[str]:
        return self._remote_debug_command

    def _generate_options(self) -> Iterable[str]:
        if self.config:
            yield "--cfg"
            yield str(self.config)

        if self.exec_strategy:
            yield f"--exec_strategy={self.exec_strategy}"

        if self.platform:
            # Then merge the value from --cfg and --platform to override
            # the value from the --cfg.  Yield the rewritten flag.
            platform_value = cl_utils.values_dict_to_config_value(
                self.merged_platform
            )
            yield f"--platform={platform_value}"

        yield from self._options

    @property
    def options(self) -> Sequence[str]:
        return list(self._generate_options())

    @property
    def preserve_unchanged_output_mtime(self) -> bool:
        """If true, local outputs that already match remote results will be untouched."""
        return "--preserve_unchanged_output_mtime" in self._options

    @property
    def canonicalize_working_dir(self) -> bool | None:
        return self._rewrapper_known_options.canonicalize_working_dir

    @property
    def download_regex(self) -> str | None:
        return self._rewrapper_known_options.download_regex

    @property
    def download_outputs(self) -> bool:
        return self._rewrapper_known_options.download_outputs

    @property
    def platform(self) -> str | None:
        return self._platform

    @property
    def merged_platform(self) -> dict[str, str]:
        """Combined platform values from --cfg and --platform.

        RBE flag precedence (highest-to-lowest):
          * command-line flag
          * RBE_platform environment variable
          * cfg file contents
        Dictionary updating is done in order of lowest-to-highest).

        If --platform is not passed explicitly on the command line,
        there is no need to call this, because rewrapper will already
        interpret the flag with the aforementioned precedence.

        Returns:
          Dictionary of platform values, combined from all sources.
        """
        merged_values: dict[str, str] = {}

        def take_dict_last_values(key_values: str) -> dict[str, str]:
            return {
                k: cl_utils.last_value_or_default(v, "")
                for k, v in cl_utils.keyed_flags_to_values_dict(
                    key_values.split(",")
                ).items()
            }

        platform_env = _rewrapper_platform_env()
        if platform_env:
            # env takes precedence over the cfg (not merged)
            merged_values.update(take_dict_last_values(platform_env))
        else:
            cfg = self.config
            if cfg:
                rewrapper_cfg: dict[str, str] = cl_utils.read_config_file_lines(
                    cfg.read_text().splitlines()
                )
                cfg_platform = rewrapper_cfg.get("platform", "")
                if cfg_platform:
                    # in case of multiple/conflicting values, take the last one
                    merged_values.update(take_dict_last_values(cfg_platform))

        cl_platform = self.platform
        if cl_platform:
            # override earlier values
            # in case of multiple/conflicting values, take the last one
            merged_values.update(take_dict_last_values(cl_platform))

        return merged_values

    @property
    def need_download_stub_predicate(self) -> Callable[[Path], bool]:
        """Return a function that indicates whether an output path will require a download stub."""
        download_regex = self.download_regex
        if download_regex is not None:
            if download_regex.startswith("-"):
                exclude_regex = re.compile(download_regex.removeprefix("-"))

                def need_download_stub(path: Path) -> bool:
                    return exclude_regex.match(str(path)) is not None

            else:
                include_regex = re.compile(download_regex)

                def need_download_stub(path: Path) -> bool:
                    return not include_regex.match(str(path))

        else:

            def need_download_stub(path: Path) -> bool:
                # --download_outputs applies to all paths
                return not self.download_outputs

        return need_download_stub

    @property
    def skipping_some_download(self) -> bool:
        """Returns true if some download is expected to be skipped."""
        return self.download_regex is not None or not self.download_outputs

    @property
    def expected_downloads(self) -> Sequence[Path]:
        """Returns a collection of output files that are expected to be downloaded."""
        return [
            f
            for f in self.output_files_relative_to_working_dir
            if not self.need_download_stub_predicate(f)
        ]

    @property
    def save_temps(self) -> bool:
        return self._save_temps

    @property
    def working_dir(self) -> Path:  # absolute
        return self._working_dir

    @property
    def remote_exec_root(self) -> Path:  # absolute
        return _REMOTE_PROJECT_ROOT

    @property
    def remote_working_dir(self) -> Path:  # absolute
        return _REMOTE_PROJECT_ROOT / self.remote_build_subdir

    @property
    def remote_disable(self) -> bool:
        return self._remote_disable

    @property
    def check_determinism(self) -> bool:
        return self._check_determinism

    @property
    def determinism_attempts(self) -> int:
        return self._determinism_attempts

    @property
    def miscomparison_export_dir(self) -> Path | None:
        if self._miscomparison_export_dir:
            return self.working_dir / self._miscomparison_export_dir
        return None

    @property
    def diagnose_nonzero(self) -> bool:
        return self._diagnose_nonzero

    def _relativize_path_to_exec_root(self, path: Path) -> Path:
        return relativize_to_exec_root(
            self.working_dir / path, start=self.exec_root
        )

    def _relativize_paths_to_exec_root(
        self, paths: Sequence[Path]
    ) -> Sequence[Path]:
        return [self._relativize_path_to_exec_root(path) for path in paths]

    @property
    def exec_root_rel(self) -> Path:
        return cl_utils.relpath(self.exec_root, start=self.working_dir)

    @property
    def build_subdir(self) -> Path:  # relative
        """This is the relative path from the exec_root to the current working dir.

        Note that this intentionally uses Path.relative_to(), which requires
        that the working dir be a subpath of exec_root.

        Raises:
          ValueError if self.exec_root is not a parent of self.working_dir.
        """
        return self.working_dir.relative_to(self.exec_root)

    @property
    def remote_build_subdir(self) -> Path:  # relative
        if self.canonicalize_working_dir:
            return reclient_canonical_working_dir(self.build_subdir)
        return self.build_subdir

    @property
    def input_list_paths(self) -> Sequence[Path]:
        return self._input_list_paths

    @property
    def inputs_relative_to_working_dir(self) -> Sequence[Path]:
        """Combines files from --inputs and --input_list_paths."""
        return self._inputs + list(
            cl_utils.expand_paths_from_files(self.input_list_paths)
        )

    @property
    def output_files_relative_to_working_dir(self) -> Sequence[Path]:
        return self._output_files

    @property
    def output_dirs_relative_to_working_dir(self) -> Sequence[Path]:
        return self._output_dirs

    @property
    def inputs_relative_to_project_root(self) -> Sequence[Path]:
        return self._relativize_paths_to_exec_root(
            self.inputs_relative_to_working_dir
        )

    @property
    def output_files_relative_to_project_root(self) -> Sequence[Path]:
        return self._relativize_paths_to_exec_root(
            self.output_files_relative_to_working_dir
        )

    @property
    def output_dirs_relative_to_project_root(self) -> Sequence[Path]:
        return self._relativize_paths_to_exec_root(
            self.output_dirs_relative_to_working_dir
        )

    def _generated_inputs_list_file(self) -> Path:
        """This file is generated with a complete list of inputs."""
        if self._default_auxiliary_file_basename is None:
            raise ValueError("self._default_auxiliary_file_basename is None")
        inputs_list_file = Path(
            self._default_auxiliary_file_basename + ".inputs"
        )
        _write_lines_to_file(
            inputs_list_file,
            (str(x) for x in self.inputs_relative_to_project_root),
        )
        return inputs_list_file

    def _generate_rewrapper_command_prefix(self) -> Iterable[str]:
        yield str(self._rewrapper)
        yield f"--exec_root={self.exec_root}"

        # The .rrpl contains detailed information for improved
        # diagnostics and troubleshooting, as well as the output
        # file digests.
        yield f"--action_log={self._action_log}"

        yield from self.options

        if self._inputs or self.input_list_paths:
            # TODO(https://fxbug.dev/42075054): use --input_list_paths only if
            # list is sufficiently long, and save writing an extra file.
            generated_inputs_list_file = self._generated_inputs_list_file()
            self._cleanup_files.append(generated_inputs_list_file)
            yield f"--input_list_paths={generated_inputs_list_file}"

        # outputs (files and dirs) need to be relative to the exec_root,
        # even as we run from inside the build_dir under exec_root.
        if self._output_files:
            output_files = ",".join(
                str(x) for x in self.output_files_relative_to_project_root
            )
            yield f"--output_files={output_files}"

        if self._output_dirs:
            output_dirs = ",".join(
                str(x) for x in self.output_dirs_relative_to_project_root
            )
            yield f"--output_directories={output_dirs}"

        if self.local_only_flags:
            # Note: this will override previous --local_wrapper options
            yield f"--local_wrapper=./{self.local_wrapper_filename}"

        if self._fsatrace_path and self._remote_fsatrace_method == "wrap":
            # Write the wrapper prefix to a temporary script, and pass it to
            # rewrapper --remote_wrapper.
            fsatrace_wrapper_prefix = cl_utils.command_quoted_str(
                self._fsatrace_command_prefix(log=self._fsatrace_remote_trace)
            )
            script = (
                "\n".join(["#!/bin/sh", f'exec {fsatrace_wrapper_prefix} "$@"'])
                + "\n"
            )
            fsatrace_remote_wrapper = self._fsatrace_remote_wrapper
            fsatrace_remote_wrapper.write_text(script)
            fsatrace_remote_wrapper.chmod(stat.S_IRWXU)  # chmod u+rwx
            self._cleanup_files.append(fsatrace_remote_wrapper)
            yield f"--remote_wrapper={fsatrace_remote_wrapper}"

    @property
    def _remote_log_command_prefix(self) -> Sequence[str]:
        return [
            str(self._remote_log_script_path),
            "--log",
            str(self._remote_log_name),
            "--",
        ]

    def _fsatrace_command_prefix(self, log: Path) -> Sequence[str]:
        return cl_utils.auto_env_prefix_command(
            [
                "FSAT_BUF_SIZE=5000000",
                str(self._fsatrace_path),
                "erwdtmq",
                str(log),
                "--",
            ]
        )

    def _generate_remote_command_prefix(self) -> Iterable[str]:
        # No need to prepend with fuchsia.REPROXY_WRAP here,
        # because auto_relaunch_with_reproxy() does that.
        yield from self._generate_rewrapper_command_prefix()
        yield "--"

    def _generate_remote_debug_command(self) -> Iterable[str]:
        """List files in the remote execution environment."""
        yield from self._generate_remote_command_prefix()
        yield from self.remote_debug_command

    def _generate_remote_launch_command(self) -> Iterable[str]:
        yield from self._generate_remote_command_prefix()

        if self._remote_log_name:
            yield from self._remote_log_command_prefix

        # When requesting both remote logging and fsatrace,
        # use fsatrace as the inner wrapper because the user is not
        # likely to be interested in fsatrace entries attributed
        # to the logging wrapper.
        if self._fsatrace_path and self._remote_fsatrace_method == "prefix":
            yield from self._fsatrace_command_prefix(
                self._fsatrace_remote_trace
            )

        yield from self.remote_only_command

    def _generate_check_determinism_prefix(self) -> Iterable[str]:
        export_dir = None
        if self.miscomparison_export_dir:
            export_dir = self.miscomparison_export_dir / self.build_subdir
        yield from fuchsia.check_determinism_command(
            exec_root=self.exec_root_rel,
            outputs=self.output_files_relative_to_working_dir,
            label=self.label,
            max_attempts=self.determinism_attempts,
            miscomparison_export_dir=export_dir,
            # no command, just prefix
        )
        # TODO: The comparison script does not support directories yet.
        # When it does, yield from self.output_dirs_relative_to_working_dir.

    def _generate_local_launch_command(self) -> Iterable[str]:
        """The local launch command may include some prefix wrappers."""
        # When requesting fsatrace, log to a different file than the
        # remote log, so they can be compared.
        if self.check_determinism:
            yield from self._generate_check_determinism_prefix()

        if self._fsatrace_path:
            yield from self._fsatrace_command_prefix(self._fsatrace_local_trace)

        yield from self.local_only_command

    @property
    def local_launch_command(self) -> Sequence[str]:
        with cl_utils.timer_cm("local_launch_command"):
            return list(self._generate_local_launch_command())

    @property
    def remote_launch_command(self) -> Sequence[str]:
        """Generates the rewrapper command, one token at a time."""
        with cl_utils.timer_cm("remote_launch_command"):
            return list(self._generate_remote_launch_command())

    @property
    def launch_command(self) -> Sequence[str]:
        """This is the fully constructed command to be executed on the host.

        In remote enabled mode, this is a rewrapper command wrapped around
        the original command.
        In remote disabled mode, this is just the original command.
        """
        if self.remote_disable:
            return self.local_launch_command
        return self.remote_launch_command

    @functools.cached_property
    def action_log_record(self) -> ReproxyLogEntry:
        self.vmsg(f"Reading remote action log from {self._action_log}.")
        return ReproxyLogEntry.parse_action_log(self._action_log)

    def _write_output_file_hash_xattrs(self, output_file: Path) -> None:
        # Use the digest from the action remote_metadata, if available.
        xattr_value = self.action_log_record.output_file_digests.get(
            output_file
        )
        if xattr_value:
            # The xattr value from RBE is in the format:
            # 0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef/112233
            # where the first hex string is the digest, and the second is the file's
            # length.
            digest = xattr_value[:64]
            expected_length = int(xattr_value[65:])
            actual_length = os.stat(output_file).st_size
            if actual_length != expected_length:
                if not is_download_stub_file(
                    output_file, use_xattr=self._use_xattr
                ):
                    raise FileSizeMismatchError(
                        str(output_file)
                        + ": expected "
                        + str(expected_length)
                        + ", got "
                        + str(actual_length)
                    )
        else:
            # RBE ran the action locally, so we'll need to hash it ourselves to produce
            # the above string.
            with open(output_file, "rb") as f:
                digest = hashlib.file_digest(f, "sha256").hexdigest()
                size = os.lstat(output_file).st_size
                xattr_value = f"{digest}/{size}"

        # Clear the old download stub marker, if present
        if _RBE_OLD_XATTR_HASH in os.listxattr(output_file):
            os.removexattr(output_file, _RBE_OLD_XATTR_HASH)

        # Write the xattr used by RBE
        os.setxattr(output_file, _RBE_XATTR_HASH, xattr_value.encode())

        # Write the xattr used by Bazel (which is the raw bytes of only the digest value)
        os.setxattr(output_file, _BAZEL_XATTR_HASH, bytes.fromhex(digest))

    def _process_download_stubs(self) -> None:
        """Create download stubs so artifacts can be retrieved later."""
        log_record = self.action_log_record

        # local execution and local race wins don't need download stubs.
        if log_record.completion_status in {
            "STATUS_LOCAL_EXECUTION",
            "STATUS_LOCAL_FALLBACK",
            "STATUS_RACING_LOCAL",
        }:
            return

        if not self.skipping_some_download:
            return

        self.vmsg("Collecting digests for all remote outputs.")
        unique_log_dir = _reproxy_log_dir()  # unique per build
        build_id = Path(unique_log_dir).name if unique_log_dir else "unknown"
        stub_infos = log_record.make_download_stubs(
            files=self.output_files_relative_to_working_dir,
            dirs=self.output_dirs_relative_to_working_dir,
            build_id=build_id,
        )

        # rewrapper has already downloaded the outputs that were not excluded
        # by 'download_regex'.
        # Write download stubs out for the artifacts that were not downloaded.

        need_download_stub = self.need_download_stub_predicate
        for stub_info in stub_infos.values():
            if need_download_stub(stub_info.path):
                self.vmsg(
                    f"  download stub: {stub_info.path}: {stub_info.blob_digest}"
                )
                self._update_stub(stub_info)

    def download_inputs(
        self, keep_filter: Callable[[Path], bool]
    ) -> dict[Path, cl_utils.SubprocessResult]:
        """Downloading inputs is useful for running local actions whose inputs
        may have come from the outputs of remote actions that opted to
        not download their outputs.

        Returns:
          Mapping of path to status.
          Failures are always included, but successes are optional.
        """
        # stub_paths are relative to the current working dir
        stub_paths = [
            path
            for path in self.inputs_relative_to_working_dir
            if keep_filter(path)
        ]

        outputs = self.output_files_relative_to_working_dir
        target = outputs[0] if len(outputs) > 0 else "unknown-target"
        try:
            self.vmsg(f"Downloading inputs for {target}: {stub_paths}")
            # Download locks needed because different actions could
            # share the same set of inputs.
            download_statuses = download_input_stub_paths_batch(
                downloader=self.downloader(),
                stub_paths=stub_paths,
                working_dir_abs=self.working_dir,
                verbose=self.verbose,
                use_xattr=self._use_xattr,
            )
        finally:
            self.vmsg(
                f"Downloads of inputs for {target} complete ({len(stub_paths)})."
            )
        return download_statuses

    def _update_stub(self, stub_info: DownloadStubInfo) -> None:
        """Write a download stub (or not, depending on conditions)."""
        path = self.working_dir / stub_info.path
        if self.preserve_unchanged_output_mtime and path.exists():
            # Consider the cases where the existing output is or is not
            # a download stub.
            if is_download_stub_file(path, use_xattr=self._use_xattr):
                # What if the local file is itself a download stub (w/ xattr)?
                # If a download was skipped because the local and remote results
                # already match, the blob_digests already matches, however, the
                # stub's action_digest may have changed, as different actions
                # can produce the same result.  The build_id will definitely
                # change across builds.  Our choices are:
                #   a) leave the old stub with its old action_digest.
                #     Pro: preserving the stub file's timestamp lets the build
                #       prune actions (ninja restat).
                #     Con: action_digest does not reflect the latest remote
                #       action that produced this artifact.
                #   b) rewrite the stub file with the new action_digest.
                #     Pro: action_digest reflects the latest remote action that
                #       produced this artifact.
                #     Con: Unconditionally updating the stub file takes away the
                #       opportunity to prune build actions (ninja restat).
                #
                # We opt for a) because it will be possible to recover the
                # latest action_digest from the action_log that produced the
                # stub info.
                old_stub_info = DownloadStubInfo.read_from_file(path)
                # The blob_digest is all that matters for being able to download
                # later.
                if old_stub_info.blob_digest != stub_info.blob_digest:
                    stub_info.create(
                        self.working_dir, use_xattr=self._use_xattr
                    )  # overwrite the old stub
                # otherwise, just leave the old stub as-is.
                return

            else:  # existing output is not a download stub
                # When we *are* preserving timestamps of unchanged local
                # (non-stub) artifacts, an existing output could mean:
                # 1) The remote output was unchanged from the old local copy,
                #   in which case, we want to keep it without overwriting it
                #   with a download stub.  It would be ok to write a download
                #   stub to the download_stub_backup_location(), but not necessary.
                # 2) The local output is stale, and different from what would
                #   have been downloaded, in which case, the new download stub
                #   should be written over it.
                # Reproxy might be able to distinguish which case applies, but this
                # information isn't reflected in the action_log, unfortunately.
                # In the worst case, we re-compute the digest of the file in
                # question to determine the correct action.
                #
                # First, check whether or not the output is paired with a backup
                # download stub (which is left behind by a stub_info.download()
                # operation).
                stub_backup_path = download_stub_backup_location(path)
                if stub_backup_path.exists():  # assume it is a stub
                    old_stub_info = DownloadStubInfo.read_from_file(
                        stub_backup_path
                    )
                    old_blob_digest = old_stub_info.blob_digest
                else:
                    old_blob_digest = get_blob_digest(path)  # slow

                if stub_info.blob_digest != old_blob_digest:
                    if stub_backup_path.exists():
                        stub_backup_path.unlink()

                    stub_info.path.unlink()
                    stub_info.create(
                        self.working_dir, use_xattr=self._use_xattr
                    )
                # else blob_digests match, leave the old stub as-is.
                return

        # When we are *not* preserving timestamps of unchanged local
        # artifacts, always writing the download stub is valid.
        stub_info.create(self.working_dir, use_xattr=self._use_xattr)

    def downloader(self) -> remotetool.RemoteTool:
        cfg = self.exec_root / _REPROXY_CFG
        return remotetool.configure_remotetool(cfg)

    def _cleanup(self) -> None:
        self.vmsg("Cleaning up temporary files.")
        with cl_utils.timer_cm("RemoteAction._cleanup()"):
            for f in self._cleanup_files:
                if f.exists():
                    f.unlink()  # does os.remove for files, rmdir for dirs

    def remote_debug(self) -> cl_utils.SubprocessResult:
        """Perform all remote setup, but run a different diagnostic command.

        An example debug command could be: "ls -lR ../.."
        This is a dignostic tool for verifying the remote environment.
        """
        return cl_utils.subprocess_call(
            list(self._generate_remote_debug_command()), cwd=self.working_dir
        )

    def _run_maybe_remotely(self) -> cl_utils.SubprocessResult:
        with cl_utils.timer_cm("launch_command"):
            command = self.launch_command
        quoted = cl_utils.command_quoted_str(command)
        self.vmsg(f"Launching: {quoted}")

        with cl_utils.timer_cm("subprocess (remote, rewrapper)"):
            return cl_utils.subprocess_call(
                command,
                cwd=self.working_dir,
                quiet=self._should_rerun_locally_on_failure,
            )

    @property
    def _should_rerun_locally_on_failure(self) -> bool:
        """If the operation fails in rewrapper, it should be re-run directly"""
        return (self.exec_strategy or "remote") in {
            "local",
            "remote_local_fallback",
        } and self._local_only_command != self._remote_only_command

    def _on_success(self) -> int:
        """Work to do after success (local or remote)."""
        # TODO: output_leak_scanner.postflight_checks() here,
        #   but only when requested, because inspecting output
        #   contents takes time.

        if self.remote_disable:  # ran only locally
            # Nothing do compare.
            return 0

        if self.skipping_some_download:
            self._process_download_stubs()

        if self._use_xattr:
            file_size_mismatches: list[str] = []
            for output_file in self.output_files_relative_to_working_dir:
                if output_file.suffix != ".d" and output_file.exists():
                    # Declared outputs are not guaranteed to be produced
                    # by the remote action.
                    try:
                        self._write_output_file_hash_xattrs(output_file)
                    except FileSizeMismatchError as e:
                        file_size_mismatches.append(str(e))

            if file_size_mismatches:
                print("\n\n", file=sys.stderr)
                print(
                    "ERROR: Invalid file downloaded from RBE:",
                    file=sys.stderr,
                )
                for mismatch in file_size_mismatches:
                    print(f"  {mismatch}", file=sys.stderr)
                print("\nRetry your build to continue.\n\n", file=sys.stderr)
                # Remove _all_ the outputs and then fail the action.
                for output_file in self.output_files_relative_to_working_dir:
                    if os.path.exists(output_file):
                        os.unlink(output_file)
                sys.exit(1)

        # Possibly transform some of the remote outputs.
        # It is important that transformations are applied before
        # local vs. remote comparison, and after possible download stub
        # generation.
        if self._post_remote_run_success_action:
            self.vmsg("Running post-remote-success actions")
            post_run_status = self._post_remote_run_success_action()
            if post_run_status != 0:
                return post_run_status

        if self.compare_with_local:  # requesting comparison vs. local
            if not self.skipping_some_download:
                # Also run locally, and compare outputs.
                return self._compare_against_local()

            # TODO: in compare-mode, force-download all output files and dirs,
            # overriding and taking precedence over --download_regex and
            # --download_outputs=false, because comparison is intended
            # to be done locally with downloaded artifacts.
            # For now, just advise the user.
            if not self.remote_disable:
                msg(
                    "Cannot compare remote outputs as requested because --download_outputs=false.  Re-run with downloads enabled to compare outputs."
                )

        return 0

    def _on_failure(self, result: cl_utils.SubprocessResult) -> int:
        """Work to do after execution fails."""

        export_dir = self.miscomparison_export_dir
        if self.check_determinism and export_dir:
            # Assume failure was due to determinism.
            # Outputs were already copied by the check-determinism script.
            # Copy just the inputs.
            with cl_utils.chdir_cm(PROJECT_ROOT):
                for f in self.inputs_relative_to_project_root:
                    cl_utils.copy_preserve_subpath(f, export_dir)

        # rewrapper assumes that the command it was given is suitable for
        # both local and remote execution, but this isn't always the case,
        # e.g. remote cross-compiling may require invoking different
        # binaries.  We know however, whether the local/remote commands
        # match and can take the appropriate local fallback action,
        # like running a different command than the remote one.
        if self._should_rerun_locally_on_failure:
            # We intended to run a different local command,
            # so ignore the result from rewrapper.
            local_exit_code = self._run_locally()
            if local_exit_code == 0:
                # local succeeded where remote failed
                self.show_local_remote_command_differences()

            return local_exit_code

        # If it wasn't run locally, the failure mode could be RBE related, so
        # diagnose if needed.
        if (self.exec_strategy or "remote") not in [
            "local",
            "remote_local_fallback",
        ] and self.diagnose_nonzero:
            analyze_rbe_logs(
                rewrapper_pid=result.pid,
                action_log=self._action_log,
            )

        return result.returncode

    def run(self) -> int:
        """Remotely execute the command.

        Returns:
          rewrapper's exit code, which is the remote execution exit code in most cases,
            but sometimes an re-client internal error code like 35 or 45.
        """
        # When using a remote canonical_working_dir, make sure the command
        # being launched does not reference the non-canonical local working
        # dir explicitly.
        if self.canonicalize_working_dir and str(self.build_subdir) != ".":
            with cl_utils.timer_cm("output_leak_scanner.preflight_checks()"):
                leak_status = output_leak_scanner.preflight_checks(
                    paths=self.output_files_relative_to_working_dir,
                    command=self.local_only_command,
                    pattern=output_leak_scanner.PathPattern(self.build_subdir),
                )
            if leak_status != 0:
                msg(
                    f"Error: Detected local output dir leaks '{self.build_subdir}' in the command.  Aborting remote execution."
                )
                return leak_status

        if self.remote_debug_command:
            self.remote_debug()
            # Return non-zero to signal that the expected outputs were not
            # produced in this mode.
            return 1

        # If needed, emit a --local_wrapper for local execution based on
        # --local-only flags seen in the original command.
        if self.local_only_flags:
            wrapper = self.local_wrapper_filename
            wrapper.write_text(self.local_wrapper_text)
            wrapper.chmod(stat.S_IRWXU)  # chmod u+rwx
            self._cleanup_files.append(wrapper)

        try:
            # If any local execution is involved, we need to make sure
            # any inputs that came from remote execution without downloading
            # are fetched locally first.
            # Unfortunately, for remote_local_fallback and racing modes,
            # this means always paying the cost of downloading inputs up front.
            exec_strategy = self.exec_strategy or "remote"  # rewrapper default
            if exec_strategy in {
                "local",
                "remote_local_fallback",
                "racing",
            }:
                # Paths based at exec_root_rel are sources or prebuilts
                # that cannot come from the outputs of any remote action.
                download_statuses = self.download_inputs(
                    lambda path: not str(path).startswith(
                        str(self.exec_root_rel)
                    )
                )
                for path, result in download_statuses.items():
                    if result.returncode != 0:
                        msg(
                            f"Downloading local action input {path} failed:\n{result.stderr_text}"
                        )
                        return result.returncode

            result = self._run_maybe_remotely()

            # Under certain error conditions, do a one-time retry
            # for flake/fault-tolerance.
            if not self.remote_disable and _should_retry_remote_action(result):
                msg("One-time retry for a possible remote-execution flake.")
                result = self._run_maybe_remotely()

            if result.returncode == 0:  # success, nothing to see
                with cl_utils.timer_cm("RemoteAction._on_success()"):
                    return self._on_success()

            # From here onward, remote return code was != 0
            return self._on_failure(result)

        finally:
            if not self._save_temps:
                self._cleanup()

    def run_with_main_args(self, main_args: argparse.Namespace) -> int:
        """Run depending on verbosity and dry-run mode.

        This serves as a template for main() programs whose
        primary execution action is RemoteAction.run().

        Args:
          main_args: struct with (.verbose, .dry_run, .label, ...)

        Returns:
          exit code
        """
        command_str = cl_utils.command_quoted_str(self.launch_command)
        if main_args.verbose and not main_args.dry_run:
            msg(command_str)
        if main_args.dry_run:
            label_str = " "
            if main_args.label:
                label_str += f"[{main_args.label}] "
            msg(f"[dry-run only]{label_str}{command_str}")
            return 0

        with cl_utils.timer_cm("RemoteAction.run()"):
            return self.run()

    def _relativize_local_deps(self, path: str) -> str:
        p = Path(path)
        if p.is_absolute():
            new_path = str(cl_utils.relpath(p, start=self.working_dir))
            self.vmsg(f"transformed dep path: {path} -> {new_path}")
            return new_path

        return path

    def _relativize_remote_deps(self, path: str) -> str:
        p = Path(path)
        if p.is_absolute():
            new_path = str(cl_utils.relpath(p, start=self.remote_working_dir))
            self.vmsg(f"transformed dep path: {path} -> {new_path}")
            return new_path

        return path

    def _relativize_remote_or_local_deps(self, path: str) -> str:
        """Relativize absolute paths (in depfiles).

        Use this variant when it is uncertain whether or not a depfile
        came from remote or local execution.
        """
        p = Path(path)
        if p.is_absolute():
            if self.exec_root in p.parents:
                new_path = str(cl_utils.relpath(p, start=self.working_dir))
            elif self.remote_exec_root in p.parents:
                new_path = str(
                    cl_utils.relpath(p, start=self.remote_working_dir)
                )
            else:
                msg(f"Unable to relativize path: {path}")
                return path

            self.vmsg(f"transformed dep path: {path} -> {new_path}")
            return new_path

        return path

    def _filtered_outputs_for_comparison(
        self,
        local_file: Path,
        local_file_filtered: Path,
        remote_file: Path,
        remote_file_filtered: Path,
    ) -> bool:
        """Tolerate some differences between files as acceptable.

        Pass this function to _detail_diff_filtered() to compare
        filtered views of some of the output files.
        This does not modify any of the output files.

        Args:
          local_file: input file to compare, also determines whether or not
            to apply a transform.
          local_file_filtered: if transforming, write filtered view here.
          remote_file: input file to compare.
          remote_file_filtered: if transforming, write filtered view here.

        Returns:
          True if transformed filtered views were written.
        """
        # TODO: support passing in filters from application-specific code.
        if local_file.suffix == ".map":  # intended for linker map files
            # Workaround https://fxbug.dev/42170565: relative-ize absolute path of
            # current working directory in linker map files.
            # These files are only used for local debugging.
            _transform_file_by_lines(
                local_file,
                local_file_filtered,
                lambda line: line.replace(
                    str(self.exec_root), str(self.exec_root_rel)
                ),
            )
            _transform_file_by_lines(
                remote_file,
                remote_file_filtered,
                lambda line: line.replace(
                    str(_REMOTE_PROJECT_ROOT), str(self.exec_root_rel)
                ),
            )
            return True

        if local_file.suffix == ".d":  # intended for depfiles
            # TEMPORARY WORKAROUND until upstream fix lands:
            #   https://github.com/pest-parser/pest/pull/522
            # Remove redundant occurrences of the current working dir absolute path.
            # Paths should be relative to the root_build_dir.
            rewrite_depfile(
                dep_file=local_file,
                transform=self._relativize_local_deps,
                output=local_file_filtered,
            )
            rewrite_depfile(
                dep_file=remote_file,
                transform=self._relativize_remote_deps,
                output=remote_file_filtered,
            )
            return True

        # No transform was done, tell the caller to compare the originals as-is.
        return False

    def download_output_file(self, path: Path) -> int:
        """Downloads one of the known output files.

        This can be useful for downloading outputs from failed actions.

        Returns:
          0 on success, non-zero on any error.
        """
        action_log = self.action_log_record
        blob_digest = action_log.output_file_digests.get(path)
        if blob_digest:
            dl_result = download_file_to_path(
                downloader=self.downloader(),
                working_dir_abs=self.working_dir,
                path=path,
                blob_digest=blob_digest,
                action_digest=action_log.action_digest,
                use_xattr=self._use_xattr,
            )
            return dl_result.verbose_returncode(f"download {path}")
        else:
            # Not necessarily an error, as some outputs are conditional.
            self.vmsg(f"Output {path} not found among remote outputs.")
            return 0

    def local_fsatrace_transform(self, line: str) -> str:
        return line.replace(str(self.exec_root) + os.path.sep, "").replace(
            str(self.build_subdir), "${build_subdir}"
        )

    def remote_fsatrace_transform(self, line: str) -> str:
        return line.replace(
            str(_REMOTE_PROJECT_ROOT) + os.path.sep, ""
        ).replace(str(self.remote_build_subdir), "${build_subdir}")

    def _compare_fsatraces_select_logs(
        self,
        local_trace: Path,
        remote_trace: Path,
    ) -> cl_utils.SubprocessResult:
        msg("Comparing local (-) vs. remote (+) file access traces.")
        # Normalize absolute paths.
        local_norm = Path(str(local_trace) + ".norm")
        remote_norm = Path(str(remote_trace) + ".norm")
        _transform_file_by_lines(
            local_trace,
            local_norm,
            lambda line: self.local_fsatrace_transform(line),
        )
        _transform_file_by_lines(
            remote_trace,
            remote_norm,
            lambda line: self.remote_fsatrace_transform(line),
        )
        return _text_diff(local_norm, remote_norm)

    def _compare_fsatraces(self) -> cl_utils.SubprocessResult:
        return self._compare_fsatraces_select_logs(
            local_trace=self._fsatrace_local_trace,
            remote_trace=self._fsatrace_remote_trace,
        )

    def _run_locally(self) -> int:
        # Run the job locally.
        # Local command may include an fsatrace prefix.
        local_command = self.local_launch_command
        local_command_str = cl_utils.command_quoted_str(local_command)
        self.vmsg(f"Executing command locally: {local_command_str}")
        exit_code = subprocess.call(local_command)
        if exit_code != 0:
            # Presumably, we want to only compare against local successes.
            msg(
                f"Local command failed for comparison (exit={exit_code}): {local_command_str}"
            )
        return exit_code

    def _compare_against_local(self) -> int:
        """Compare outputs from remote and local execution.

        Returns:
          exit code 0 for success (all outputs match) else non-zero.
        """
        self.vmsg("Comparing outputs between remote and local execution.")
        # Backup outputs from remote execution first to '.remote'.

        # The fsatrace files will be handled separately because they are
        # already named differently between their local/remote counterparts.
        direct_compare_output_files = [
            (f, Path(str(f) + ".remote"))
            for f in self.output_files_relative_to_working_dir
            if f.is_file() and f.suffix != ".remote-fsatrace"
        ]

        # We have the option to keep the remote or local outputs in-place.
        # Use the results from the local execution, as they are more likely
        # to be what the user expected if something went wrong remotely.
        for f, bkp in direct_compare_output_files:
            f.rename(bkp)

        compare_output_dirs = [
            (d, Path(str(d) + ".remote"))
            for d in self.output_dirs_relative_to_working_dir
            if d.is_dir()
        ]
        for d, bkp in compare_output_dirs:
            d.rename(bkp)

        # Run the job locally, for comparison.
        local_exit_code = self._run_locally()
        if local_exit_code != 0:
            return local_exit_code

        # Apply workarounds to make comparisons more meaningful.
        # self._rewrite_local_outputs_for_comparison_workaround()

        # Translate output directories into list of files.
        all_compare_files = direct_compare_output_files + list(
            _expand_common_files_between_dirs(compare_output_dirs)
        )

        output_diff_candidates = []
        # Quick file comparison first.
        for f, remote_out in all_compare_files:
            if not f.exists():
                msg(f"Expected output {f} does not exist, so skipping.")
                continue
            if not remote_out.exists():
                msg(
                    f"Expected output {remote_out} does not exist, so skipping."
                )
                continue
            if _files_match(f, remote_out):
                # reclaim space when remote output matches, keep only diffs
                os.remove(remote_out)
            else:
                output_diff_candidates.append((f, remote_out))

        # Take the difference candidates and re-compare filtered views
        # of them, to remove unimportant or acceptable differences.
        comparisons = (
            (
                local_out,
                remote_out,
                _detail_diff_filtered(
                    local_out,
                    remote_out,
                    maybe_transform_pair=self._filtered_outputs_for_comparison,
                    project_root_rel=self.exec_root_rel,
                ),
            )
            for local_out, remote_out in output_diff_candidates
        )

        differences = [
            (local_out, remote_out, result)
            for local_out, remote_out, result in comparisons
            if result.returncode != 0
        ]

        # Report detailed differences.
        if differences:
            msg(
                "*** Differences between local (-) and remote (+) build outputs found. ***"
            )
            for local_out, remote_out, result in differences:
                msg(f"  {local_out} vs. {remote_out}:")
                diff_status = result.verbose_returncode("diff")
                if not result.stdout and not result.stderr:
                    print(
                        f"diff tool exited {diff_status}, but did not report differences."
                    )
                msg("------------------------------------")

                # Optionally: copy differences to a location that other tools
                # can pickup or upload.
                export_dir = self.miscomparison_export_dir  # is absolute
                if export_dir:
                    with cl_utils.chdir_cm(self.exec_root):
                        # Copy outputs.
                        cl_utils.copy_preserve_subpath(
                            self.build_subdir / local_out, export_dir
                        )
                        cl_utils.copy_preserve_subpath(
                            self.build_subdir / remote_out, export_dir
                        )
                        # Copy inputs.
                        for f in self.inputs_relative_to_project_root:
                            cl_utils.copy_preserve_subpath(f, export_dir)

            # Also compare file access traces, if available.
            if self._fsatrace_path:
                fsatrace_diff_status = self._compare_fsatraces()
                for line in fsatrace_diff_status.stdout:
                    print(line)

            self.show_local_remote_command_differences()

            return 1

        # No output content differences: success.
        return 0


# For multiprocessing, mapped function must be serializable.
# Module-scope functions are serializable.
# Arguments are packed into a tuple to be map()-able.
def _download_input_for_mp(
    packed_args: tuple[Path, remotetool.RemoteTool, Path, bool, bool],
) -> tuple[Path, cl_utils.SubprocessResult]:
    path, downloader, working_dir_abs, verbose, use_xattr = packed_args
    if verbose:
        msg(f"  Considering input {path}")

    status = download_from_stub_path(
        path, downloader, working_dir_abs, verbose, use_xattr=use_xattr
    )
    if status.returncode != 0:  # alert, but do not fail
        msg(f"Unable to download input {path}.")

    if verbose:
        msg(f"    downloaded input {path} (status: {status.returncode})")
    return path, status


def _download_output_for_mp(
    packed_args: tuple[
        DownloadStubInfo, remotetool.RemoteTool, Path, bool, bool
    ],
) -> tuple[Path, cl_utils.SubprocessResult]:
    stub_info, downloader, working_dir_abs, verbose, use_xattr = packed_args
    path = stub_info.path
    if verbose:
        msg(f"  Downloading output {path}")
    status = stub_info.download(
        downloader=downloader,
        working_dir_abs=working_dir_abs,
        use_xattr=use_xattr,
    )

    if status.returncode != 0:  # alert, but do not fail
        msg(f"Unable to download output {path}.")

    if verbose:
        msg(f"    downloaded output {path} (status: {status.returncode})")
    return path, status


def download_input_stub_paths_batch(
    downloader: remotetool.RemoteTool,
    stub_paths: Sequence[Path],
    working_dir_abs: Path,
    parallel: bool = True,
    verbose: bool = False,
    use_xattr: bool = _CAN_HAVE_XATTR,
) -> dict[Path, cl_utils.SubprocessResult]:
    """Downloads artifacts from a collection of stubs in parallel.

    Returns:
      Mapping of paths to status, which includes failures,
      but not necessarily successes.
    """
    if verbose:
        msg(f"Downloading potentially {len(stub_paths)} artifacts")
    download_args = [
        # args for _download_input_for_mp
        (stub_path, downloader, working_dir_abs, verbose, use_xattr)
        for stub_path in stub_paths
        if is_download_stub_file(
            working_dir_abs / stub_path, use_xattr=use_xattr
        )
    ]
    if not download_args:
        if verbose:
            msg("  Nothing to download.")
        return {}

    if parallel and len(download_args) > 1:
        try:
            with multiprocessing.Pool(_MAX_CONCURRENT_DOWNLOADS) as pool:
                statuses = pool.map(_download_input_for_mp, download_args)
        except OSError as e:  # in case /dev/shm is not writeable (required)
            if (e.errno == errno.EPERM and e.filename == "/dev/shm") or (
                e.errno == errno.EROFS
            ):
                if len(download_args) > 1 and verbose:
                    msg(
                        f"Warning: downloading sequentially instead of in parallel: {stub_paths}."
                    )
                statuses = list(map(_download_input_for_mp, download_args))
            else:
                raise e  # Some other error
    else:
        statuses = list(map(_download_input_for_mp, download_args))

    return {path: status for path, status in statuses}


def download_output_stub_infos_batch(
    downloader: remotetool.RemoteTool,
    stub_infos: Sequence[DownloadStubInfo],
    working_dir_abs: Path,
    parallel: bool = True,
    verbose: bool = False,
    use_xattr: bool = _CAN_HAVE_XATTR,
) -> dict[Path, cl_utils.SubprocessResult]:
    """Downloads artifacts from a collection of stubs in parallel."""
    download_args = [
        # args for _download_output_for_mp
        (stub_info, downloader, working_dir_abs, verbose, use_xattr)
        for stub_info in stub_infos
    ]
    if not download_args:
        return {}

    if parallel and len(download_args) > 1:
        try:
            with multiprocessing.Pool(_MAX_CONCURRENT_DOWNLOADS) as pool:
                statuses = pool.map(_download_output_for_mp, download_args)
        except OSError as e:  # in case /dev/shm is not writeable (required)
            if (e.errno == errno.EPERM and e.filename == "/dev/shm") or (
                e.errno == errno.EROFS
            ):
                if len(download_args) > 1:
                    msg(
                        "Warning: downloading sequentially instead of in parallel."
                    )
                statuses = list(map(_download_output_for_mp, download_args))
            else:
                raise e  # Some other error
    else:
        statuses = list(map(_download_output_for_mp, download_args))

    return {path: status for path, status in statuses}


def _rewrapper_arg_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        "Understand some rewrapper flags, so they may be used as attributes.",
        argument_default=None,
        add_help=False,  # do not intercept --help
    )
    parser.add_argument(
        "--exec_root",
        type=str,
        metavar="ABSPATH",
        help="Root directory from which all inputs/outputs are contained.",
    )
    parser.add_argument(
        "--canonicalize_working_dir",
        type=cl_utils.bool_golang_flag,
        help="If true, remotely execute the command in a working dir location, that has the same depth as the actual build output dir relative to the exec_root.  This makes remote actions cacheable across different output dirs.",
    )
    parser.add_argument(
        "--download_outputs",
        type=cl_utils.bool_golang_flag,
        default=True,
        help="Set to false to avoid downloading outputs after remote execution succeeds.  Each toolchain (Rust, C++) may decide how to apply this to its set of outputs by default.  For example, for Rust, this affects only the main -o output.  Prefer using --download_regex over this option.",
    )
    parser.add_argument(
        "--download_regex",
        type=str,
        help="Use a regex to specify which outputs to download or not.  Prefix with '-' to exclude files matching the pattern from downloading.  This flag takes precedence over --download_outputs.  This wrapper only supports a single regex value.",
    )
    return parser


_REWRAPPER_ARG_PARSER = _rewrapper_arg_parser()


def _string_split(text: str) -> Sequence[str]:
    return text.split()


def inherit_main_arg_parser_flags(
    parser: argparse.ArgumentParser,
    default_cfg: Path | None = None,
    default_bindir: Path | None = None,
) -> None:
    """Extend an existing argparser with standard flags.

    These flags are available for tool-specific remote command wrappers to use.
    """
    default_cfg = default_cfg or Path(
        PROJECT_ROOT_REL, "build", "rbe", "fuchsia-rewrapper.cfg"
    )
    default_bindir = default_bindir or Path(
        PROJECT_ROOT_REL, fuchsia.RECLIENT_BINDIR
    )

    rewrapper_group = parser.add_argument_group(
        title="rewrapper",
        description="rewrapper options that are intercepted and processed",
    )
    rewrapper_group.add_argument(
        "--cfg",
        type=Path,
        default=default_cfg,
        help="rewrapper config file.",
    )
    rewrapper_group.add_argument(
        "--exec_strategy",
        type=str,
        choices=["local", "remote", "remote_local_fallback", "racing"],
        help="rewrapper execution strategy.",
    )
    rewrapper_group.add_argument(
        "--inputs",
        action="append",
        # leave the type as [str], so commas can be processed downstream
        default=[],
        metavar="PATHS",
        help="Specify additional remote inputs, comma-separated, relative to the current working dir (repeatable, cumulative).  Note: This is different than `rewrapper --inputs`, which expects exec_root-relative paths.",
    )
    rewrapper_group.add_argument(
        "--input_list_paths",
        action="append",
        # leave the type as [str], so commas can be processed downstream
        default=[],
        metavar="PATHS",
        help="Specify additional remote inputs file lists, whose elements are relative to the current working dir (repeatable, cumulative).  Note: This is different than `rewrapper --input_list_paths`, which expects elements to be relative to the exec_root.",
    )
    rewrapper_group.add_argument(
        "--output_files",
        action="append",
        # leave the type as [str], so commas can be processed downstream
        default=[],
        metavar="FILES",
        help="Specify additional remote output files, comma-separated, relative to the current working dir (repeatable, cumulative).  Note: This is different than `rewrapper --output_files`, which expects exec_root-relative paths.",
    )
    rewrapper_group.add_argument(
        "--output_directories",
        action="append",
        # leave the type as [str], so commas can be processed downstream
        default=[],
        metavar="DIRS",
        help="Specify additional remote output directories, comma-separated, relative to the current working dir (repeatable, cumulative).  Note: This is different than `rewrapper --output_directories`, which expects exec_root-relative paths.",
    )
    rewrapper_group.add_argument(
        "--platform",
        type=str,
        help="The rewrapper platform variable specifies remote execution parameters, such as OS type, image, worker constraints, etc.  The value specified in this wrapper is intercepted and merged with the value found in the --cfg.",
    )

    main_group = parser.add_argument_group(
        title="common",
        description="Generic remote action options",
    )
    main_group.add_argument(
        "--bindir",
        type=Path,
        default=default_bindir,
        metavar="PATH",
        help="Path to reclient tools like rewrapper, reproxy.",
    )
    main_group.add_argument(
        "--local",
        action="store_true",
        default=False,
        help="Disable remote execution, run the original command locally.",
    )
    main_group.add_argument(
        "--disable-xattr",
        action="store_true",
        default=False,
        help="Disable recording remote action output digests in xattrs.",
    )
    main_group.add_argument(
        "--dry-run",
        action="store_true",
        default=False,
        help="Show final rewrapper command and exit.",
    )
    main_group.add_argument(
        "--verbose",
        action="store_true",
        default=False,
        help="Print additional debug information while running.",
    )
    main_group.add_argument(
        "--label",
        type=str,
        default="",
        help="Build system identifier, for diagnostic messages",
    )
    main_group.add_argument(
        "--check-determinism",
        action="store_true",
        default=False,
        help="Run locally repeatedly and compare outputs [requires: --local].",
    )
    main_group.add_argument(
        "--determinism-attempts",
        type=int,
        default=1,
        help="For --check-determinism, re-run and compare this many times (max).",
    )
    main_group.add_argument(
        "--log",
        type=str,
        dest="remote_log",
        const="<AUTO>",  # pick name based on ${output_files[0]}
        default="",  # blank means to not log
        metavar="BASE",
        nargs="?",
        help="""Capture remote execution's stdout/stderr to a log file.
If a name argument BASE is given, the output will be 'BASE.remote-log'.
Otherwise, BASE will default to the first output file named.""",
    )
    main_group.add_argument(
        "--save-temps",
        action="store_true",
        default=False,
        help="Keep around intermediate files that are normally cleaned up.",
    )
    main_group.add_argument(
        "--auto-reproxy",
        action="store_true",
        default=False,
        help="OBSOLETE: reproxy is already automatically launched if needed.",
    )
    main_group.add_argument(
        "--fsatrace-path",
        type=Path,
        default=None,  # None means do not trace
        metavar="PATH",
        help="""Given a path to an fsatrace tool (located under exec_root), this will trace a remote execution's file accesses.  This is useful for diagnosing unexpected differences between local and remote builds.  The trace file will be named '{output_files[0]}.remote-fsatrace' (if there is at least one output), otherwise 'remote-action-output.remote-fsatrace'.  Pass the empty string "" to automatically use the prebuilt fsatrace binaries.""",
    )
    main_group.add_argument(
        "--compare",
        action="store_true",
        default=False,
        help="In 'compare' mode, run both locally and remotely (sequentially) and compare outputs.  Exit non-zero (failure) if any of the outputs differs between the local and remote execution, even if those executions succeeded.  When used with --fsatrace-path, also compare file access traces.  No comparison is done with --local mode.",
    )
    main_group.add_argument(
        "--miscomparison-export-dir",
        type=Path,
        default=None,
        metavar="DIR",
        help="When using --compare or --check-determinism, save unexpectedly different artifacts to this directory, preserving relative path under the working directory.",
    ),
    main_group.add_argument(
        "--diagnose-nonzero",
        action="store_true",
        default=False,
        help="""On nonzero exit statuses, attempt to diagnose potential RBE issues.  This scans various reproxy logs for information, and can be noisy.""",
    )
    main_group.add_argument(
        "--remote-debug-command",
        type=_string_split,
        default=None,
        help=f"""Alternate command to execute remotely, while doing the setup for the original command.  e.g. --remote-debug-command="ls -l -R {PROJECT_ROOT_REL}" .""",
    )
    # Positional args are the command and arguments to run.
    parser.add_argument(
        "command", nargs="*", help="The command to run remotely"
    )


def _main_arg_parser() -> argparse.ArgumentParser:
    """Construct the argument parser, called by main()."""
    parser = argparse.ArgumentParser(
        description="Executes a build action command remotely.",
        argument_default=[],
    )
    inherit_main_arg_parser_flags(parser)
    return parser


_MAIN_ARG_PARSER = _main_arg_parser()


def remote_action_from_args(
    main_args: argparse.Namespace,
    remote_options: Sequence[str] | None = None,
    command: Sequence[str] | None = None,
    # These inputs and outputs can come from application-specific logic.
    inputs: Sequence[Path] | None = None,
    input_list_paths: Sequence[Path] | None = None,
    output_files: Sequence[Path] | None = None,
    output_dirs: Sequence[Path] | None = None,
    **kwargs: Any,  # other RemoteAction __init__ params
) -> RemoteAction:
    """Construct a remote action based on argparse parameters."""
    inputs = [
        *(inputs or []),
        *[Path(p) for p in cl_utils.flatten_comma_list(main_args.inputs)],
    ]
    input_list_paths = [
        *(input_list_paths or []),
        *[
            Path(p)
            for p in cl_utils.flatten_comma_list(main_args.input_list_paths)
        ],
    ]
    output_files = [
        *(output_files or []),
        *[Path(p) for p in cl_utils.flatten_comma_list(main_args.output_files)],
    ]
    output_dirs = [
        *(output_dirs or []),
        *[
            Path(p)
            for p in cl_utils.flatten_comma_list(main_args.output_directories)
        ],
    ]
    return RemoteAction(
        rewrapper=main_args.bindir / "rewrapper",
        options=(remote_options or []),
        command=command or main_args.command,
        cfg=main_args.cfg,
        exec_strategy=main_args.exec_strategy,
        inputs=inputs,
        input_list_paths=input_list_paths,
        output_files=output_files,
        output_dirs=output_dirs,
        platform=main_args.platform,
        disable=main_args.local,
        verbose=main_args.verbose,
        label=main_args.label,
        compare_with_local=main_args.compare,
        check_determinism=main_args.check_determinism,
        determinism_attempts=main_args.determinism_attempts,
        miscomparison_export_dir=main_args.miscomparison_export_dir,
        save_temps=main_args.save_temps,
        remote_log=main_args.remote_log,
        fsatrace_path=main_args.fsatrace_path,
        diagnose_nonzero=main_args.diagnose_nonzero,
        remote_debug_command=main_args.remote_debug_command,
        use_xattr=_CAN_HAVE_XATTR and not main_args.disable_xattr,
        **kwargs,
    )


_FORWARDED_REMOTE_FLAGS = cl_utils.FlagForwarder(
    # Mapped options can be wrapper script options (from
    # inherit_main_arg_parser_flags) or rewrapper options.
    [
        cl_utils.ForwardedFlag(
            name="--remote-disable",
            has_optarg=False,
            mapped_name="--local",
        ),
        cl_utils.ForwardedFlag(
            name="--remote-inputs",
            has_optarg=True,
            mapped_name="--inputs",
        ),
        cl_utils.ForwardedFlag(
            name="--remote-outputs",
            has_optarg=True,
            mapped_name="--output_files",
        ),
        cl_utils.ForwardedFlag(
            name="--remote-output-dirs",
            has_optarg=True,
            mapped_name="--output_directories",
        ),
        cl_utils.ForwardedFlag(
            name="--remote-flag",
            has_optarg=True,
            mapped_name="",
        ),
    ]
)


def forward_remote_flags(
    argv: Sequence[str],
) -> tuple[Sequence[str], Sequence[str]]:
    """Propagate --remote-* flags from the wrapped command to main args.

    This allows late-appended flags to influence wrapper scripts' behavior.
    This works around limitations and difficulties of specifying
    tool options in some build systems.
    This should be done *before* passing argv to _MAIN_ARG_PARSER.
    Unlike using argparse.ArgumentParser, this forwarding approache
    preserves the left-to-right order in which flags appear.

    Args:
      argv: the full command sequence seen my main, like sys.argv[1:]
          Script args appear before the first '--', and the wrapped command
          is considered everything thereafter.

    Returns:
      1) main script args, including those propagated from the wrapped command.
      2) wrapped command, but with --remote-* flags filtered out.
    """
    # Split the whole command around the first '--'.
    script_args, sep, unfiltered_command = cl_utils.partition_sequence(
        argv, "--"
    )
    if sep == None:
        return (["--help"], [])  # Tell the caller to trigger help and exit

    forwarded_flags, filtered_command = _FORWARDED_REMOTE_FLAGS.sift(
        unfiltered_command
    )
    return [*script_args, *forwarded_flags], filtered_command


def auto_relaunch_with_reproxy(
    script: Path, argv: Sequence[str], args: argparse.Namespace
) -> None:
    """If reproxy is not already running, re-launch with reproxy running.

    Args:
      script: the invoking script
      argv: the original complete invocation
      args: argparse Namespace for argv.  Only needs to be partially
          processed as far as inherit_main_arg_parser_flags().

    Returns:
      nothing when reproxy is already running.
      If a re-launch is necessary, this does not return, as the process
      is replaced by a os.exec*() call.
    """
    if args.auto_reproxy:
        msg(
            "You no longer need to pass --auto-reproxy, reproxy is launched automatically when needed."
        )

    if args.dry_run or args.local:
        # Don't need reproxy when no call to rewrapper is expected.
        return

    proxy_log_dir = _reproxy_log_dir()
    rewrapper_log_dir = _rewrapper_log_dir()
    if args.verbose:
        msg(f"Detected RBE_proxy_log_dir={proxy_log_dir}")
        msg(f"Detected RBE_log_dir={rewrapper_log_dir}")

    if proxy_log_dir and rewrapper_log_dir:
        # Ok for caller to proceed and eventually invoke rewrapper
        return

    python = cl_utils.relpath(Path(sys.executable), start=Path(os.curdir))
    relaunch_args = ["--", str(python), "-S", str(script), *argv]
    # Wrap verbosely when auto-re-launching with reproxy wrapper, because
    # this is typically used for one-off commands during debug.
    # Normal builds should already be wrapped, and should not require auto-re-launching.
    reproxy_wrap = [cl_utils.qualify_tool_path(fuchsia.REPROXY_WRAP), "-v"]

    cmd_str = cl_utils.command_quoted_str(reproxy_wrap + relaunch_args)
    msg(f"Automatically re-launching: {cmd_str}")
    cl_utils.exec_relaunch(reproxy_wrap + relaunch_args)
    assert False, "exec_relaunch() should never return"


def main(argv: Sequence[str]) -> int:
    # Move --remote-* flags from the wrapped command to equivalent main args.
    main_argv, filtered_command = forward_remote_flags(argv)

    # forward all unknown flags to rewrapper
    # forwarded rewrapper options with values must be written as '--flag=value',
    # not '--flag value' because argparse doesn't know what unhandled flags
    # expect values.
    main_args, other_remote_options = _MAIN_ARG_PARSER.parse_known_args(
        main_argv
    )

    # Re-launch self with reproxy if needed.
    auto_relaunch_with_reproxy(
        script=Path(__file__),
        argv=argv,
        args=main_args,
    )
    # At this point, reproxy is guaranteed to be running.

    remote_action = remote_action_from_args(
        main_args=main_args,
        remote_options=other_remote_options,
        command=filtered_command,
    )
    with cl_utils.timer_cm("RemoteAction.run_with_main_args()"):
        return remote_action.run_with_main_args(main_args)


if __name__ == "__main__":
    init_from_main_once()
    sys.exit(main(sys.argv[1:]))
