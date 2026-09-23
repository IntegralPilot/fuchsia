# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Process the debug_symbols.json build API module in various ways."""

import json
import os
import shutil
import subprocess
import sys
import time
import typing as T
from pathlib import Path

DebugSymbolEntryType = dict[str, T.Any]

# Binaries whose file name appears in this set are ignored.
# See https://fxbug.dev/445670194 for context. This can be
# removed once https://fxbug.dev/445786243 is fixed.
_DEBUG_BINARY_DENYLIST = {
    "static-pie-noop-test",
    "static-pie-custom-startup-test",
}


def extract_gnu_build_id(elf_file: str | Path) -> str:
    """Extracts the GNU build ID from an ELF64 file.

    Args:
        elf_file: Path to input file.
    Returns:
        The build-id value has an hexadecimal string, or
        an empty string on failure (e.g. not an ELF file,
        or no .note.gnu.build-id section in it).
    """
    try:
        # See https://en.wikipedia.org/wiki/Executable_and_Linkable_Format
        with open(elf_file, "rb") as f:
            # Read ELF header
            e_ident = f.read(16)
            if e_ident[:4] != b"\x7fELF":
                return ""  # Not an ELF file

            def read(count: int) -> int:
                return int.from_bytes(f.read(count), "little")

            f.seek(40)  # e_shoff location.
            e_shoff = read(8)
            if e_shoff == 0:
                return ""  # no section headers.

            f.seek(58)  # read e_shentsize, e_shnum and e_shstrndx
            e_shentsize, e_shnum, e_shstrndx = read(2), read(2), read(2)

            f.seek(
                e_shoff + e_shentsize * e_shstrndx + 24
            )  # sh_offset in string table Elf64_shdr entry
            shstrtab_offset = read(8)

            f.seek(e_shoff)
            for i in range(e_shnum):
                f.seek(e_shoff + i * e_shentsize)
                sh_name = read(4)
                f.read(4 + 8 + 8)  # skip sh_flags and sh_addr
                sh_offset = read(8)
                sh_size = read(8)

                f.seek(shstrtab_offset + sh_name)
                name = b""
                while True:
                    byte = f.read(1)
                    if byte == b"\x00":
                        break
                    name += byte

                if name == b".note.gnu.build-id":
                    f.seek(sh_offset + 16)  # note description offset.
                    return f.read(sh_size - 16).hex()
    except Exception:
        # Ignore missing file, unreadable file, or truncated file errors.
        pass
    return ""


class DebugSymbolsManifestParser(object):
    """Parse the debug_symbols.json build API module and expand its
    included manifests to generate a flattened list of entries.

    See comments for //:debug_symbols target for schema description.

    Usage is:
        1) Create instance.

        2) Call parse_manifest_file(). This will raise ValueError
           if the input is malformed, if an include cycle is
           detected.

        3) Use the |entries| property to get the flatenned list of entries,
           with duplicates removed.

        4) Use the |extra_input_files| property to get the files read by the
           parser due to manifest includes or when reading the build-ID values.
    """

    def __init__(self, build_dir: None | Path | str = None) -> None:
        """Create instance.

        Args:
           build_dir: Optional build directory Path. For entries that have a
              "manifest" key, the value will be a path relative to this value.
              Default value is the current directory.
        """
        self._build_dir = Path(build_dir) if build_dir else Path.cwd()
        self._visited_paths: set[Path] = set()
        self._visited_stack: list[Path] = []
        self._entries: list[DebugSymbolEntryType] = []
        self._resolve_build_id = False
        self._get_build_id: T.Callable[[str | Path], str] = extract_gnu_build_id
        self._elfinfo: T.Any = None

    def enable_build_id_resolution(self) -> None:
        """Ensure the build-id value is computed if not present in an entry."""
        self._resolve_build_id = True

    def set_build_id_callback_for_test(
        self, get_build_id: T.Callable[[str | Path], str]
    ) -> None:
        """Override the function used to compute the BuildID, only used for tests."""
        self._get_build_id = get_build_id

    @property
    def entries(self) -> list[DebugSymbolEntryType]:
        """The flattened list of parsed entries, each one describing a single ELF file."""
        return self._entries

    @property
    def extra_input_files(self) -> set[Path]:
        """The set of extra input files read by the parsing functions.

        These maybe be used to write implicit inputs in a Ninja depfile
        for a script invoked through a GN action, to ensure proper
        incremental builds.
        """
        return self._visited_paths

    def parse_manifest_file(self, manifest_path: Path) -> None:
        """Parse a given debug_symbols.json manifest.

        Args:
            manifest_path: Path to input manifest.
        Raises:
            ValueError if the manifest, or one of its includes is
            malformed, or if there is a cycle in the include chain.
        """
        if not manifest_path.exists():
            raise ValueError(f"Missing manifest file at {manifest_path}")

        with manifest_path.open("rt") as f:
            manifest_json = json.load(f)

        self.parse_manifest_json(manifest_json, manifest_path)

    def _resolve_entry_build_id(self, entry: T.Any) -> str:
        """Return the build-id value of a specific manifest entry.

        Args:
            entry: A DebugSymbolEntryType value with a "debug" key.
        Returns:
            A build-id hexadecimal string on success, or an empty string
            on failure, which can happen for non-ELF platforms, and a
            certain number of entries like Go host binaries, or special
            Zircon binaries.
        """
        assert "debug" in entry, f"Invalid debug manifest entry {entry}"

        # First, look at the debug value if it looks like ../.build-id/xx/yyyyyyyy.debug
        debug = entry["debug"]
        pos = debug.find(".build-id/")
        if pos >= 0:
            pos += len(".build-id/")
            rest = debug[pos:]
            if len(rest) > 3 and rest[2] == "/" and rest.endswith(".debug"):
                build_id = rest[:2] + rest[3:-6]
                return build_id

        # Second, look for an elf_build_id_file path.
        build_id_file = entry.get("elf_build_id_file")
        if build_id_file:
            build_id_file_path = self._build_dir / build_id_file
            self._visited_paths.add(build_id_file_path)
            # Some files list an elf_build_id_file but do not generate
            # anything at that location. E.g.: XXXX
            if build_id_file_path.exists():
                build_id = build_id_file_path.read_text().strip()
                # It is possible for certain files to have an empty
                # build_id file, for example, see
                # /zircon/kernel/lib/libc/string/arch/x86:_hermetic_code_blob.memcpy_movsb.executable(//zircon/kernel:kernel_x64)
                if build_id:
                    return build_id

        # As a fallback, try to extract the build id directly from the file.
        debug_path = self._build_dir / debug
        self._visited_paths.add(debug_path)
        build_id = self._get_build_id(debug_path)

        # It is possible for unstripped files to not have a GNU .build-id
        # value, for example Go host binaries. So this could be an empty string.
        return build_id

    def _parse_entry(
        self, entry: T.Any, manifest_path: Path | str | None
    ) -> None:
        """Parse a given debug manifest entry, and update state accordingly.

        Args:
            entry:  The entry to parse
            manifest_path:  The path to use in error messages
        """
        debug = entry.get("debug")
        if debug:
            if self._resolve_build_id and "elf_build_id" not in entry:
                # Resolve build-id value, and skip entries for which this is
                # not possible, e.g. Go host tools, and some special Zircon
                # binaries.
                build_id = self._resolve_entry_build_id(entry)
                if not build_id:
                    print(
                        f"MISSING build-id FOR {entry}",
                        file=sys.stderr,
                    )
                    return
                entry["elf_build_id"] = build_id

            self._entries.append(entry)
            return

        sub_manifest = entry.get("manifest")
        if not sub_manifest:
            raise ValueError(f"Malformed entry in {manifest_path}: {entry}")

        sub_manifest_path = (self._build_dir / sub_manifest).resolve()
        if not sub_manifest_path.exists():
            raise ValueError(
                f"Missing include path in {manifest_path}: {sub_manifest}"
            )

        # Check for cycles in manifest include chain. Raise exception if one
        # is detected.
        for n, visited in enumerate(self._visited_stack):
            if visited == sub_manifest_path:
                error_msg = "Recursive manifest includes:\n"
                for visited in self._visited_stack[n:]:
                    error_msg += f"  {visited} -->\n"
                error_msg += f"  {sub_manifest_path}"
                raise ValueError(error_msg)

        self._visited_paths.add(sub_manifest_path)
        self._visited_stack.append(sub_manifest_path)

        self.parse_manifest_file(sub_manifest_path)

        self._visited_stack = self._visited_stack[:-1]

    def parse_manifest_json(
        self, manifest_json: T.Any, manifest_path: Path | str | None = None
    ) -> None:
        """Parse a given debug_symbols manifest JSON value.

        Args:
            manifest_json: The input manifest as a JSON value
            (i.e. a list of dictionaries).

            manifest_path: The path of the manifest whose content is
            being parsed (only used for error messages).

        Raises:
            ValueError if the manifest, or one of its includes is
            malformed, or if there is a cycle in the include chain.
        """
        # As a special case, the prebuilt_binaries.json file can be 'None'
        # instead of an empty list in a clean Fuchsia checkout that didn't
        # use `jiri init -fetch-optional=debug-symbols`. Convert it here
        # to avoid crashing the debug symbol extraction process on infra.
        if manifest_json is None:
            manifest_json = []

        if not isinstance(manifest_json, list):
            raise ValueError(
                f"Malformed manifest at {manifest_path}: expected list, got {type(manifest_json)}"
            )

        for entry in manifest_json:
            self._parse_entry(entry, manifest_path)

    def deduplicate_entries(self) -> None:
        """Deduplicate entries after parsing.

        Duplicate entries are possible when dealing with prebuilt symbols, for
        example:

        - The {BUILD}/gen/build/prebuilt/prebuilt_symbols.json file
          is created by //build/prebuilt:prebuilt which parses the content
          of //prebuilt/.build-id/ and generates breakpad symbol files
          directly into {BUILD}/.build-id for them. The entries in this
          manifest do not have a "label" key though. E.g.:

            {
              "breakpad": ".build-id/63/cd5f4d7cfa5a487cd50446b34023eb9a68fad6.sym",
              "cpu": "arm64",
              "debug": "../../prebuilt/.build-id/63/cd5f4d7cfa5a487cd50446b34023eb9a68fad6.debug",
              "elf_build_id": "63cd5f4d7cfa5a487cd50446b34023eb9a68fad6",
              "os": "fuchsia"
            },

        - The {BUILD}/obj/src/connectivity/bluetooth/core/bt-host/bt-host.debug_symbols.json
          manifest contains labelled entries for some of the same prebuilt binaries,
          but without any "breakpad" key.

            {
              "cpu": "arm64",
              "debug": "../../prebuilt/.build-id/63/cd5f4d7cfa5a487cd50446b34023eb9a68fad6.debug",
              "dest_path": "bin/bt_host",
              "elf_build_id": "63cd5f4d7cfa5a487cd50446b34023eb9a68fad6",
              "label": "//src/connectivity/bluetooth/core/bt-host:bt-host.package(//build/toolchain/fuchsia:arm64)",
              "os": "fuchsia"
            },

        Deduplication verifies that there are no conflicts between entries pointing
        to the same "debug" path, and merges the keys to keep all information.

        This updates self._entries and should be called after a call to
        parse_manifest_json() or parse_manifest_file().

        Raises:
            ValueError when duplicates are found. The exception string contains
            a newline-separated list of error messages.
        """
        debug_to_entries: dict[str, T.Any] = {}
        errors: list[str] = []
        new_entries = []
        for entry in self._entries:
            debug = entry["debug"]
            cur_entry = debug_to_entries.get(debug, None)
            if cur_entry is None:
                debug_to_entries[debug] = entry
                new_entries.append(entry)
                continue

            for key in (
                "elf_build_id",
                "elf_build_id_file",
                "breakpad",
                "gsym",
                "label",
                "cpu",
                "os",
                "dest_path",
            ):
                cur_value = cur_entry.get(key, "")
                new_value = entry.get(key, "")
                if cur_value == new_value:
                    continue
                if cur_value == "":
                    cur_entry[key] = new_value
                elif new_value != "":
                    errors.append(
                        f"Incompatible '{key}' value between {cur_entry} and {entry}"
                    )

            # If the merge resolved the BuildID value, drop the elf_build_id_file key.
            if cur_entry.get("elf_build_id") and cur_entry.get(
                "elf_build_id_file"
            ):
                cur_entry.drop("elf_build_id_file", None)

        if errors:
            raise ValueError("\n".join(errors))

        self._entries = new_entries


# Type of a callable object used to start a new command. First parameter
# is a unique command id, second parameter is the cmd_data passed to
# CommandPool.add_command(). Must return a new subprocess.Popen instance.
CommandRunnerType = T.Callable[[int, T.Any], subprocess.Popen[str]]

# Type of a callable object used to compute the result of a given
# command invocation. First parameter is a unique command id, second
# parameter is a cmd_data value, and third parameter is a completed
# subprocess.Popen value.
CommandResultProcessorType = T.Callable[
    [int, T.Any, subprocess.Popen[str]], T.Any
]


class CommandPool(object):
    """Generic parallel command runner class.

    Usage is:
    1) Create instance, passing the max parallel command count (depth).

    2) Call add_command() as many time as necessary to schedule a
       command to run. This takes a callable that must return
       a subprocess.Popen instance to start the command.

    3) Call run() to launch all commands in parallel. This generates
       a sequence of (cmd_id, cmd_result) values. Where cmd_id is
       a unique command index, and cmd_result is the result of calling
       result_processor() on completed subprocess.Popen instances.
    """

    def __init__(self, depth: int) -> None:
        """Create instance.

        Args:
            depth: Maximum number of parallel commands to run.
        """
        self._depth = depth
        self._commands: list[tuple[int, T.Any, CommandRunnerType]] = []

    def add_command(
        self, cmd_data: T.Any, cmd_runner: CommandRunnerType
    ) -> None:
        """Add a new command invocation to the scheduled plan.

        Args:
            runner: A callable that returns a new subprocess.Popen
               instance obtained by calling subprocess.run() for
               the command. It takes a unique command id as its
               first parameter.
        """
        cmd_id = len(self._commands)
        self._commands.append((cmd_id, cmd_data, cmd_runner))

    def run(
        self, result_processor: CommandResultProcessorType
    ) -> T.Iterator[T.Any]:
        """Run all recorded commands in parallel.

        This ensures that this will not launch more than self._depth
        commands in parallel.

        Args:
          result_processor: A callable used to convert the result of a completed
            command's Popen value into an result value.

        Yields:
           A sequence of cmd_result values, each one being computed by calling
           result_processor() on command completion.
        """
        # Map command ids to running subprocess.Popen instances.
        running: dict[int, subprocess.Popen[str]] = {}

        def poll_run_queue() -> T.Iterator[T.Any]:
            # Find all completed commands.
            results: list[T.Any] = []
            completed_ids: list[int] = []

            for cmd_id, proc in running.items():
                returncode = proc.poll()
                if returncode is not None:
                    cmd_data = self._commands[cmd_id][1]
                    completed_ids.append(cmd_id)
                    results.append(result_processor(cmd_id, cmd_data, proc))

            # Release file descriptors as early as possible.
            del proc
            for cmd_id in completed_ids:
                del running[cmd_id]

            yield from results

            if not completed_ids:
                time.sleep(0.01)  # 10ms

        for cmd_id, cmd_data, cmd_runner in self._commands:
            while len(running) == self._depth:
                yield from poll_run_queue()
            running[cmd_id] = cmd_runner(cmd_id, cmd_data)

        while running:
            yield from poll_run_queue()


class DebugSymbolExporter(object):
    """A class used to export debug symbols and generate breakpad symbols if needed.

    Usage is:
        1) Create instance. Optionally passing the path to the Breakpad `dump_syms` tool.

        2) Call parse_debug_symbols(), passing a flat list of debug symbol
           manifest entries whose "elf_build_id" key has been set properly
           for ELF platforms.

        3) Call export_debug_symbols() to populate a directory with symlinks
           to debug symbol files (following the standard
           $EXPORT_DIR/.build-id/xx/yyyyyyyyyyy.debug layout), as well as
           $EXPORT_DIR/build-ids.json and $EXPORT_DIR/build-ids.txt.

           If a `dump_syms` path was passed to the constructor, this also creates
           Breakpad symbol files from the ELF debug one under the name
           $EXPORT_DIR/.build-id/xx/yyyyyyyyyyyy.sym).

           The directory's content is re-created on each call, and thus does not
           accumulate files from previous runs.
    """

    def __init__(
        self,
        build_dir: Path | str,
        dump_syms_tool: None | Path | str = None,
        gsymutil_tool: None | Path | str = None,
        log: None | T.Callable[[str], None] = None,
        log_error: None | T.Callable[[str], None] = None,
    ) -> None:
        """Create instance.

        Args:
            build_dir: The Ninja build directory path.

            dump_syms_tool: Optional path the Breakpad dump_syms host tool.
                If provided, will be used to generate breakpad symbol files
                from ELF debug binaries.

            gsymutil_tool: Optional path to the LLVM gsymutil tool host
                tool. If provided, will be used to generate GSYM symbol files
                from ELF debug binaries.
        """
        self._build_dir = Path(build_dir)

        # Map destination path, relative to self._output_dir, to target path.
        # Used for debug symbol symlinks, as well as symlinks to existing breakpad symbol files.
        self._symlink_map: dict[Path, Path] = {}

        # Map breakpad symbol destination path, relative to self._output_dir, to
        # the manifest entry.
        self._breakpad_map: dict[Path, DebugSymbolEntryType] = {}

        # A similar map for GSYM symbol extraction.
        self._gsym_map: dict[Path, DebugSymbolEntryType] = {}

        self._dump_syms_tool = dump_syms_tool
        self._gsymutil_tool = gsymutil_tool
        self._log = log
        self._log_error = log_error

        # Map GNU BuildId hex value to the corresponding GN or Bazel label.
        # (Bazel labels always starts with @).
        self._build_ids_to_labels: dict[str, str] = {}

        # The set of BuildID values for Fuchsia binaries only.
        self._fuchsia_build_ids: set[str] = set()

        # Filtered list of debug symbol entries (only for ELF binaries containing
        # a proper GNU BuildID note). "debug" paths will be relative to the output
        # directory.
        self._debug_symbols: list[DebugSymbolEntryType] = []

    def parse_debug_symbols(
        self, debug_entries: list[DebugSymbolEntryType]
    ) -> None:
        """Parse list of debug symbol entries.

        Args:
            debug_entries: A list of DebugSymbolEntryType values. Entries for non-ELF
               systems will be ignored, otherwise their "elf_build_id" key must be
               set properly.
        """
        # Find all the debug symbol entries that do not have a breakpad symbol path yet.
        for entry in debug_entries:
            # Skip non-ELF platforms.
            if entry["os"] not in ("fuchsia", "linux"):
                continue

            if os.path.basename(entry["debug"]) in _DEBUG_BINARY_DENYLIST:
                continue

            build_id = entry.get("elf_build_id", "")
            if not build_id:
                if self._log:
                    self._log(f"Missing elf_build_id in entry: {entry}")
                continue

            self._build_ids_to_labels[build_id] = entry.get("label", "")
            if entry["os"] == "fuchsia":
                self._fuchsia_build_ids.add(build_id)

            # Record a .debug symlink in the symlink map.
            debug_dst_path = (
                Path(".build-id") / build_id[:2] / f"{build_id[2:]}.debug"
            )
            debug_src_path = self._build_dir / entry["debug"]
            self._symlink_map[debug_dst_path] = debug_src_path

            # If a stripped file is provided, add a symlink to it as well.
            if entry.get("stripped", "") != "":
                stripped_src_path = self._build_dir / entry["stripped"]
                stripped_dst_path = (
                    Path(".build-id") / build_id[:2] / f"{build_id[2:]}"
                )
                self._symlink_map[stripped_dst_path] = stripped_src_path

            # If a breakpad file path is provided by the entry, record a .sym symlink in the
            # map. Otherwise, add an entry in breakpad_map to ensure a breakpad symbol
            # file can be created later. *Unless* this is not a Fuchsia binary, see
            # https://fxbug.dev/445119122 for context.
            breakpad_dst_path = Path(
                str(debug_dst_path).replace(".debug", ".sym")
            )
            breakpad_file = entry.get("breakpad")
            if breakpad_file:
                self._symlink_map[breakpad_dst_path] = (
                    self._build_dir / breakpad_file
                )
            elif entry["os"] == "fuchsia":
                self._breakpad_map[breakpad_dst_path] = entry

            # Same for GSYM symbols.
            gsym_dst_path = Path(str(debug_dst_path).replace(".debug", ".gsym"))
            gsym_file = entry.get("gsym")
            if gsym_file:
                self._symlink_map[gsym_dst_path] = self._build_dir / gsym_file
            elif entry["os"] == "fuchsia":
                self._gsym_map[gsym_dst_path] = entry

            debug_symbol = entry.copy()
            debug_symbol["debug"] = str(debug_dst_path)
            # The following lines remove "stripped" if it is an empty string or
            # otherwise rewrite it.
            debug_symbol.pop("stripped", None)
            if entry.get("stripped", "") != "":
                debug_symbol["stripped"] = ".build-id/{}/{}".format(
                    build_id[:2], build_id[2:]
                )
            debug_symbol["breakpad"] = str(breakpad_dst_path)
            debug_symbol.pop("elf_build_id_file", None)
            self._debug_symbols.append(debug_symbol)

    def get_debug_symbols_to_build_id_copies(
        self, build_id_dir: Path | str
    ) -> list[tuple[str, str]]:
        """Get the list of files to copy to an external .build-id/ directory.

        Collects the list of (src, dst) path pairs to copy debug binaries to their
        location in an output .build-id directory. Useful if the caller wants to
        perform the copy using parallel threads / tasks.

        Consider using copy_debug_symbols_to_build_id() instead to do the copy.

        Args:
            build_id_dir: Path to the output .build-id directory. Its name must
                be ".build-id".
        Raises
            AssertionError if build_id_dir.name is not ".build-id".
        Returns:
            A list of (src_path, dst_path) pairs describing which files to copy.
        """
        build_id_dir = Path(build_id_dir)
        assert (
            build_id_dir.name == ".build-id"
        ), f"Invalid output directory name (.../.build-id expected): {build_id_dir}"

        # dst_path values in the loop below already begin with .build-id/
        build_id_root = build_id_dir.parent

        result: list[tuple[str, str]] = []
        for dst_path, target_path in self._symlink_map.items():
            if not dst_path.name.endswith(".debug"):
                continue  # Ignore breakpad, GSYM or stripped files.
            output_path = build_id_root / dst_path
            if output_path.exists():
                # The {BUILD_DIR}/.build-id/ directory accumulates files over time,
                # unless the next `fx clean`.
                #
                # It is assumed that BuildID values are unique and content-dependent.
                # Thus if a file already exists in the destination directory, assume
                # this is a duplicate and ignore it.
                continue

            result.append((str(target_path), str(output_path)))

        return result

    def copy_debug_symbols_to_build_id(self, build_id_dir: Path | str) -> None:
        """Copy debug symbols to an external .build-id/ directory.

        Unlike export_debug_symbols(), this does not create a new output directory,
        and copies the file instead of creating symlinks.

        This also doesn't create build-ids.txt, build-ids.json or breakpad files.
        This should be used only to copy debug symbols to the Ninja build directory,
        for local debugging / symbolification.

        Args:
           build_id_dir: Path to the output .build-id directory. Its name must be ".build-id".
        """
        copies = self.get_debug_symbols_to_build_id_copies(build_id_dir)
        log = self._log

        for src, dst in copies:
            src_path = Path(src)
            dst_path = Path(dst)

            if log:
                log(f"Copying {dst_path} FROM {src_path}")

            dst_path.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(src_path, dst_path)

    def export_debug_symbols(
        self,
        output_dir: Path | str,
        depth: int = 0,
    ) -> bool:
        """Create a new output directory with symlinks to debug symbol files.

        This populates output_dir to provide symlinks to debug symbols using
        the standard .build-id/xx/yyyyyyyyyyy.debug layout for them.

        If the input debug symbol entries contained paths to breakpad symbol files,
        these will appear in the output directory as symlink named
        .build-id/xx/yyyyyyyyyyy.sym

        Finally, if dump_syms_tool was provided in the constructor, it will be
        invoked to generate breakpad symbol files in the output directory, by
        parsing the content of .build-id/xx/yyyyyyyyyy.debug to generate the
        corresponding .build-id/xx/yyyyyyyyyyyy.sym file.

        Args:
            output_dir: Path to output directory. Its current content will be removed.

            depth: Optional. Maximum number of concurrent dump_syms invocations to use.
                Default is to use the number of available CPU cores.

        Returns:
            True on success, False on error.

            On success, the output directory will contain:

            - .build-id/xx/yyyyyyyyyyy.debug symlinks to the corresponding debug
               symbol file.

            - .build-id/xx/yyyyyyyyyyyy.sym breakpad symbol files. These are either
              symlinks (when the breakpad symbol was provided by the Fuchsia checkout,
              e.g. for Clang and Rust runtime libraries), or a file generated by this
              function by invoking the Breakpad `dump_syms` tool, if provided.

            - A build-ids.json file mapping hexadecimal build-id values to labels
              of the generating target (either from GN or Bazel, Bazel labels always
              start with @). This is used by one coverage-related infra recipe

            - A build-ids.txt which lists all build-id values in the export directory,
              one hexadecimal string per line. Used by artifactory to upload
              the debug symbols to cloud storage.

            - A debug_symbols.json manifest following the //:debug_symbols schema
              detailing each debug binary, its build-id value, its optional Fuchsia
              installation path, and its optional breakpad file.
        """
        log = self._log
        self._log_error

        output_dir = Path(output_dir)

        # Create clean new output directory.
        if output_dir.exists():
            shutil.rmtree(output_dir)
        output_dir.mkdir(parents=True)

        # Write the build-ids.json file for artifactory.
        build_ids_json = output_dir / "build-ids.json"
        if log:
            log(f"Creating {build_ids_json}")

        with build_ids_json.open("wt") as f:
            json.dump(self._build_ids_to_labels, f, sort_keys=True, indent=2)

        # Write the build-ids.txt file for artifactory.
        # Artifactory expects that this only contains values for Fuchsia binaries.
        build_ids_txt = output_dir / "build-ids.txt"
        if log:
            log(f"Creating {build_ids_txt}")

        with build_ids_txt.open("wt") as f:
            for build_id in sorted(self._fuchsia_build_ids):
                f.write(f"{build_id}\n")

        if log:
            log(
                "Creating %d symlinks in %s"
                % (len(self._symlink_map), output_dir)
            )

        # Populate all symlinks under output_dir
        for dst_path, target_path in self._symlink_map.items():
            link_path = output_dir / dst_path
            link_path.parent.mkdir(parents=True, exist_ok=True)
            link_path.symlink_to(target_path.resolve())

        # Write the debug_symbols.json manifest.
        with (output_dir / "debug_symbols.json").open("wt") as f:
            json.dump(self._debug_symbols, f, indent=2, sort_keys=True)

        if depth <= 0:
            depth = len(os.sched_getaffinity(0))

        if self._dump_syms_tool and not self._generate_breakpad_symbols(
            output_dir, depth
        ):
            return False

        if self._gsymutil_tool and not self._generate_gsym_symbols(
            output_dir, depth
        ):
            return False

        if log:
            log("Done!")

        return True

    def _generate_breakpad_symbols(self, output_dir: Path, depth: int) -> bool:
        """Generate breakpad symbols from debug binaries.

        Runs the dumpsym tool in multiple processes concurrently.

        Args:
            output_dir: Output directory path.
            depth: Number of max concurrent processes allowed.
        Returns:
            True in case of success.
        """
        log = self._log
        log_error = self._log_error

        if len(self._breakpad_map) == 0:
            if log:
                log("Skipping breakpad symbol generation (no inputs).")
            return True

        if log:
            log(
                "Generating %d breakpad symbols in %s"
                % (len(self._breakpad_map), output_dir)
            )

        command_pool = CommandPool(depth)

        cmd_ids_to_files: dict[int, T.TextIO] = {}

        def cmd_dumpsym_runner(
            cmd_id: int, entry: DebugSymbolEntryType
        ) -> subprocess.Popen[str]:
            """Start a command to generate the breakpad symbol file for a given entry.

            Args:
                entry: The input debug symbol manifest entry.
            Returns:
                a new subprocess.Popen instance.
            """
            debug_symbol_path = self._build_dir / entry["debug"]

            build_id = entry["elf_build_id"]
            symbol_path = (
                Path(".build-id") / build_id[:2] / f"{build_id[2:]}.sym"
            )
            if log:
                log(
                    f"  - Creating {symbol_path} FROM {os.path.relpath(debug_symbol_path)}"
                )

            dumpsym_cmd = [
                str(self._dump_syms_tool),
                "-r",  # Do not handle inter-compilation unit references.
            ]
            if entry["os"] == "fuchsia":
                dumpsym_cmd += [
                    "-n",
                    "<_>",  # Use specific object name.
                    "-o",
                    "Fuchsia",  # Use specific operating system name.
                ]

            dumpsym_cmd += [str(debug_symbol_path)]

            symbol_path = output_dir / symbol_path
            symbol_path.parent.mkdir(parents=True, exist_ok=True)
            symbol_file = symbol_path.open("wt")

            # Save for closing in the result processor.
            cmd_ids_to_files[cmd_id] = symbol_file

            return subprocess.Popen(
                dumpsym_cmd,
                stdout=symbol_file,
                stderr=subprocess.PIPE,
                text=True,
            )

        def cmd_dumpsym_result_processor(
            cmd_id: int,
            entry: DebugSymbolEntryType,
            proc: subprocess.Popen[str],
        ) -> bool:
            """Process the result of a completed command."""
            assert proc.returncode is not None
            assert proc.stderr  # make pypy happy.
            stderr = proc.stderr.read()
            proc.stderr.close()

            # Close the output file handle.
            cmd_ids_to_files[cmd_id].close()

            if proc.returncode == 0:
                return True

            if log_error:
                log_error(
                    "Failure to generate breakpad symbol for: {}".format(
                        entry["debug"]
                    )
                )
                log_error(stderr)

            return False

        for entry in self._breakpad_map.values():
            command_pool.add_command(entry, cmd_dumpsym_runner)

        # Run all commands in parallel and change result to False if at
        # least one error is detected. Error messages are sent to log_error
        # by cmd_result_processor() directly.
        success = True
        for cmd_success in command_pool.run(cmd_dumpsym_result_processor):
            if not cmd_success:
                success = False

        return success

    def _generate_gsym_symbols(self, output_dir: Path, depth: int) -> bool:
        """Generate breakpad symbols from debug binaries.

        Runs the gsymutil tool in multiple processes concurrently.


        Args:
            output_dir: Output directory path.
            depth: Number of max concurrent processes allowed.
        Returns:
            True in case of success.
        """
        log = self._log
        log_error = self._log_error
        log = self._log
        log_error = self._log_error

        if len(self._gsym_map) == 0:
            if log:
                log("Skipping GSYM symbol generation (no inputs).")
            return True

        if log:
            log(
                "Generating %d GSYM symbols in %s"
                % (len(self._gsym_map), output_dir)
            )

        command_pool = CommandPool(depth)

        def cmd_gsymutil_runner(
            cmd_id: int, entry: DebugSymbolEntryType
        ) -> subprocess.Popen[str]:
            """Start a command to generate the GSYM symbol file for a given entry.

            Args:
                entry: The input debug symbol manifest entry.
            Returns:
                a new subprocess.Popen instance.
            """
            src_symbol_path = self._build_dir / entry["debug"]
            build_id = entry["elf_build_id"]
            gsymbol_path = ".build-id/{}/{}.gsym".format(
                build_id[:2], build_id[2:]
            )
            if log:
                log(
                    f"  - Creating {gsymbol_path} FROM {os.path.relpath(src_symbol_path)}"
                )

            dst_symbol_path = output_dir / gsymbol_path

            gsymutil_cmd = [
                str(self._gsymutil_tool),
                f"--convert={src_symbol_path}",
                f"--out-file={dst_symbol_path}",
            ]

            dst_symbol_path.parent.mkdir(parents=True, exist_ok=True)

            return subprocess.Popen(
                gsymutil_cmd,
                # Do not capture stdout with subprocess.PIPE here as it is often
                # larger than the pipe's capacity (64 kiB on Linux), blocking the
                # process and preventing it from ever completing.
                # The alternative would be to redirect to an opened file object
                # but there is no need for this output anyway.
                stdout=subprocess.DEVNULL,
                stderr=subprocess.PIPE,
                text=True,
            )

        def cmd_gsymutil_result_processor(
            cmd_id: int,
            entry: DebugSymbolEntryType,
            proc: subprocess.Popen[str],
        ) -> bool:
            """Process the result of a completed command."""
            assert proc.returncode is not None
            assert proc.stderr  # make pypy happy.
            stderr = proc.stderr.read()
            proc.stderr.close()

            if proc.returncode == 0:
                return True

            if log_error:
                log_error(stderr)

            return False

        for entry in self._gsym_map.values():
            command_pool.add_command(entry, cmd_gsymutil_runner)

        # Run all commands in parallel and change result to False if at
        # least one error is detected. Error messages are sent to log_error
        # by cmd_result_processor() directly.
        success = True
        for cmd_success in command_pool.run(cmd_gsymutil_result_processor):
            if not cmd_success:
                success = False

        return success
