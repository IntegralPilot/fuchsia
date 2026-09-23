# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Providers for debug symbols."""

FuchsiaDebugSymbolInfo = provider(
    doc = "Contains information that can be used to register debug symbols.",
    fields = {
        "build_id_dirs_mapping": """A { source_search_root -> depset[build_id_dir] } dictionary,
            where 'build_id_dir' is a File value pointing to a .build-id/ directory, and
            'source_search_root' is either a string or a File value, used to locate source files
            when using the debugger.

            The source paths embedded in debug symbol files are usually relative. Historically, these
            were relative to the Ninja build directory (e.g. "../../src/foo/foo.cc"), which is why
            this is key is named 'build_dir' in files like symbol-index.json. In the context of
            Bazel, these source paths are relative to the Bazel exec_root instead, which is
            different from the Ninja build directory.

            If 'source_search_root' is a string, it is interpreted as an environment variable
            name, which must be defined by Bazel when the action that registers debug symbols
            is run, such as BUILD_WORKSPACE_DIRECTORY (see Bazel user manual).

            If 'source_search_root' is a File pointing to a directory, the latter is used
            directly as a possible source search directory.

            If 'source_search_root' is a File pointing to a file, its parent directory is used
            instead as a possible source search directory.
            """,
    },
)

FuchsiaCollectedDebugSymbolsInfo = provider(
    doc = "Contains a collection of debug symbols that were collected through an aspect.",
    fields = {
        "collected_symbols": "A depset containing the direct and transitive symbols",
    },
)

def _fuchsia_unstripped_binary_info_init(*, unstripped_file, dest, stripped_file = None, source_search_root = None):
    if not dest or type(dest) != "string":
        fail("Required 'dest' argument must be a string, got: %s" % repr(dest))
    if not unstripped_file or type(unstripped_file) != "File":
        fail("Required 'unstripped_file' argument must be a File, got: %s" % repr(unstripped_file))
    if stripped_file and type(stripped_file) != "File":
        fail("Optional 'stripped_file' argument must be a File, got: %s" % repr(stripped_file))
    if source_search_root != None and type(source_search_root) != "File":
        fail("Optional 'source_search_root' argument must be a None or a File, got: %s type=%s" % (repr(source_search_root), type(source_search_root)))
    return {
        "dest": dest,
        "unstripped_file": unstripped_file,
        "stripped_file": stripped_file,
        "source_search_root": "BUILD_WORKSPACE_DIRECTORY" if source_search_root == None else source_search_root,
        "never_forward": True,
    }

FuchsiaUnstrippedBinaryInfo, make_fuchsia_unstripped_binary_info = provider(
    doc = "Contains information about one unstripped Fuchsia binary and its install location for the corresponding stripped file",
    fields = {
        "unstripped_file": "A required File value for the source unstripped ELF binary file.",
        "stripped_file": "Either None, or a File value for the corresponding stripped ELF binary file, if available as a prebuilt.",
        "dest": "A Fuchsia package install path string for the stripped file.",
        "source_search_root": """Either None, or a File value pointing to a file or directory,
            see FuchsiaDebugSymbolInfo for documentation about this value. If None, the root workspace
            directory is used as the source search root directory.""",
        "never_forward": """A boolean whose value must be True. Its presence ensures that these values are
            never forwarded to dependents. See documentation for can_forward_provider() function.""",
    },
    init = _fuchsia_unstripped_binary_info_init,
)

FuchsiaUnstrippedBinariesInfo = provider(
    doc = "Contains information about a list of unstripped Fuchsia binaries.",
    fields = {
        "binaries": "A list of FuchsiaUnstrippedBinaryInfo instances.",
    },
)

FuchsiaCollectedUnstrippedBinariesInfo = provider(
    doc = "Contains information about a set of unstripped ELF binaries.",
    fields = {
        "source_search_root_to_unstripped_binaries": """
            A { source_search_root -> depset[struct(dest, unstripped_file, stripped_file)] } dictionary,
            Where 'unstripped_file' is a source File value for the unstripped file,
            where 'stripped_file' is either None, or a source File value for the corresponding
            stripped file if available as a prebuilt, and 'dest' is a install path string within
            a Fuchsia package for the corresponding stripped file.

            Where 'source_search_root' is either a string or a File value describing the source
            search directory used by the zxdb to locate sources at debug time. See FuchsiaDebugSymbolInfo
            for more details about this value.
            """,
    },
)
