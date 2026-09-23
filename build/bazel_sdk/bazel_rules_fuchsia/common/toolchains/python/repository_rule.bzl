# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Repository rule used to generate a better Python runtime for the Fuchsia build.

The idea is to use a single zip archive to hold all lib/python<version>/ modules
to drastically limit the number of files exposed in Bazel sandboxes. From 4940 to 3!

To do so requires the following:

1) A zip archive that contains all files from <python_prefix>/lib/python<version>/,
   named "lib_python<version>.zip" in this repository.

2) A symlink to the real python interpreter, named "python3-real" in the repository.

3) A wrapper script that sets PYTHONPATH to point to lib_python<version>.zip directly,
   and invokes python3-real with the -S flag (to avoid reading site-specific
   module installs).

Example usage:

  # WORKSPACE.bazel

  workspace(name = "my_project")

  ...

  # Load the repository rule. Note that this requires @rules_python to already be loaded.
  load("//path/to:this/repository_rule.bzl", "compact_python_runtime_repository")

  # Create the repository for the compact python runtime.
  compact_python_runtime_repository(
    name = "compact_python"
  )

  # Register the python runtime, it is always named `py_toolchain`.
  register_toolchains("@compact_python//:py_toolchain")

"""

# Set to True to enable log messages.
_LOG = False

def log(message):
    if _LOG:
        # buildifier: disable=print
        print(message)

# Name of the environment variable used to check for a content-based hash
# reflecting the content of the Python source toolchain, see documentation
# for the compact_python_runtime_repository() repository rule below for
# details.
_VERSION_FILE_VARNAME = "LOCAL_PREBUILT_PYTHON_VERSION_FILE"

def _make_path_from_str(repo_ctx, path_str):
    if not path_str.startswith("/"):
        path_str = "%s/%s" % (repo_ctx.workspace_root, path_str)
    return repo_ctx.path(path_str)

# Ensure this repository rule is re-run everytime the content
# of a given path changes (if relative to the workspace root).
# Does not do anything if path is empty or absolute.
def _record_path_dependency(repo_ctx, path_str):
    if path_str and not path_str.startswith("/"):
        repo_ctx.path("%s/%s" % (repo_ctx.workspace_root, path_str))
        log("### Recording %s as path dependency for repository %s ###" % (path_str, repo_ctx.name))
    else:
        log("### IGNORING %s AS PATH DEPENDENCY FOR REPOSITORY %s ####" % (path_str, repo_ctx.name))

def _compact_python_runtime_impl(repo_ctx):
    # If content_hash_file is provided, make sure this repository rule
    # is re-run whenever its content changes.
    if repo_ctx.attr.content_hash_file:
        _record_path_dependency(repo_ctx, repo_ctx.attr.content_hash_file)
    elif repo_ctx.os.environ.get(_VERSION_FILE_VARNAME):
        _record_path_dependency(repo_ctx, repo_ctx.os.environ[_VERSION_FILE_VARNAME])
    elif repo_ctx.attr.interpreter_path:
        _record_path_dependency(repo_ctx, repo_ctx.attr.interpreter_path)

    # Find the python/bin/ path.
    python_interpreter_path = repo_ctx.attr.interpreter_path
    if not python_interpreter_path:
        python_interpreter = repo_ctx.which("python3")
        if not python_interpreter:
            fail("There is no python3 interpreter in your PATH! Set python_interpreter_path " +
                 "when calling this repository rule to point to an existing one.")
        _record_path_dependency(repo_ctx, str(python_interpreter))
    else:
        python_interpreter = _make_path_from_str(repo_ctx, python_interpreter_path)
        if not python_interpreter.exists:
            fail("Python3 interpreter does not exist: %s" % python_interpreter)

    python_binpath = python_interpreter.dirname

    # Detect which Python version is supported by this installation.
    python_version = None
    for file in python_binpath.readdir():
        filename = file.basename

        # Use the versioned python3 file name, but takes care of ignoring
        # suffixes such as the one in `python3.XX-config`.
        if filename.startswith("python3."):
            python_version, _, __ = filename[6:].partition("-")
            break

    if not python_version:
        fail("Could not find Python version from: %s" % python_binpath)

    # Create symlink to include directory.
    repo_ctx.symlink(python_binpath.dirname.get_child("include"), "include")

    lib_dir = python_binpath.dirname.get_child("lib")
    if not lib_dir.exists:
        fail("Missing python library directory: %s" % lib_dir)

    # Create a symlink to the real interpreter.
    python3_real = "python%s-real" % python_version
    repo_ctx.symlink(python_interpreter, python3_real)

    lib_python_zip = "lib_python%s.zip" % python_version

    # Either symlink or create a zip archive that contains the content of
    # <python_install_dir>/lib/python<version>/
    if repo_ctx.attr.lib_python_zip:
        if repo_ctx.attr.lib_python_path:
            fail("Only one of lib_python_zip or lib_python_path can be defined!")
        lib_python_zip_path = _make_path_from_str(repo_ctx, repo_ctx.attr.lib_python_zip)
        repo_ctx.symlink(lib_python_zip_path, lib_python_zip)
    else:
        if repo_ctx.attr.lib_python_path:
            lib_python_path = _make_path_from_str(repo_ctx, repo_ctx.attr.lib_python_path)
        else:
            lib_python_path = lib_dir.get_child("python%s" % python_version)
        if not lib_python_path.exists:
            fail("Missing python library path: %s" % lib_python_path)

        # Create the zip archive using a custom Python script, since this is
        # more portable than relying on a host `zip` tool being available.
        # On Linux, this is slightly slower than using the host zip command
        # (i.e. 0.77s vs 0.483s).
        zip_directory_script = repo_ctx.path(Label("//common:scripts/zip-directory.py"))
        ret = repo_ctx.execute(
            [
                str(python_interpreter),
                str(zip_directory_script),
                str(lib_python_zip),
                str(lib_python_path),
            ],
            quiet = False,  # False for debugging!
        )
        if ret.return_code != 0:
            fail("Could not create python library zip archive!: %s" % ret.stderr)

        real_lib_dynload = lib_python_path.get_child("lib-dynload")
        if real_lib_dynload.exists:
            repo_ctx.symlink(real_lib_dynload, "lib/python%s/lib-dynload" % python_version)

    # Create a launcher shell script named 'python3' that invokes 'python3-real'
    #
    # - PYTHONHOME is set to _SCRIPT_DIR to ensure sys.path only contains
    #   paths relative to it. Otherwise, some paths hard-coded in the interpreter
    #   binary will be used (e.g. `/work/out/python3`), which could lead to
    #   bad surprises.
    #
    # - PYTHONPATH is extended to point to the zip archive, and allows the
    #   interpreter to find all system libraries from it.
    #
    # - The `-S` flag disables site-specific module lookups.
    #
    # - The `-s` flag disables user-specific module lookups.
    #
    # Note that `python3` also supports the `-I` flag to run in `isolated` mode,
    # where PYTHONPATH and PYTHONHOME are ignored, but this forces sys.path to
    # strictly hard-coded values that are unusable here.
    #
    python3_launcher = "python3"
    repo_ctx.file(
        python3_launcher,
        content = '''\
#!/bin/bash
# AUTO-GENERATED - DO NOT EDIT

# Find real location of Fuchsia's hermetic Python toolchain, in case this script
# is run via a symlink, e.g. when symlinked into .venv by newer versions of
# rules_python.
readonly _REAL_SCRIPT="$(realpath "${{BASH_SOURCE[0]}}")"
readonly _SCRIPT_DIR="$(dirname "${{_REAL_SCRIPT}}")"
PYTHONHOME="${{_SCRIPT_DIR}}" \\
PYTHONPATH="${{_SCRIPT_DIR}}/{lib_python_zip}:${{PYTHONPATH}}" \\
exec "${{_SCRIPT_DIR}}/{python3_real}" -S -s "$@"
'''.format(python3_real = python3_real, lib_python_zip = lib_python_zip),
        executable = True,
    )

    python_runtime_files = [python3_launcher, python3_real, lib_python_zip]

    _CPU_MAP = {
        "amd64": "x86_64",
    }

    host_os = repo_ctx.os.name.lower()
    if host_os.startswith("windows"):
        host_os = "windows"

    host_cpu = repo_ctx.os.arch
    host_cpu = _CPU_MAP.get(host_cpu, host_cpu)

    repo_ctx.template(
        "BUILD.bazel",
        str(repo_ctx.path(Label("//common:toolchains/python/template.BUILD.bazel"))),
        substitutions = {
            "{python_launcher}": python3_launcher,
            "{python_runtime_files}": str(python_runtime_files),
            "{python_version}": python_version,
            "{repository_dir}": repo_ctx.attr.name,
            "{host_platform_os_constraint}": "@platforms//os:" + host_os,
            "{host_platform_cpu_constraint}": "@platforms//cpu:" + host_cpu,
        },
    )

compact_python_runtime_repository = repository_rule(
    implementation = _compact_python_runtime_impl,
    doc = """\
Generate a repository directory that contains a very compact Python
toolchain installation. This considerably speeds up invocation of
any py_binary() script.

A regular toolchain requires adding 5000+ files to each sandbox every
time a py_binary() is invoked. The compact toolchain avoids that by
creating a zip archive containing all standard modules and ensuring
the interpreter uses it at runtime. This reduces the number of files
to add to the sandbox to only 3.

To ensure that the repository rule is re-run properly when the content of
the source prebuilt python directory changes, one of the following techniques
can be used:

1) Define content_hash_file to point to a file whose content will be a unique
   hash computed from the source directory's content. Whenever this value
   changes, the repository rule will be run to regenerate the right content.
   This technique is used by the Fuchsia platform build.

2) Define the %s environment variable to
   point to content hash file. This is used by the Bazel SDK test suite's
   bazel_test.py script.

3) As a fallback, if interpreter_path is given, and is a path relative to
   the workspace, its hash will be used as the source of truth. Note that
   if this path is absolute, it will be ignored entirely.
""" % _VERSION_FILE_VARNAME,
    environ = [_VERSION_FILE_VARNAME],
    attrs = {
        "interpreter_path": attr.string(
            doc = """\
Path to the Python interpreter program, either absolute, or relative
to the project root. If not provided, the python3 in PATH will be
used instead.""",
        ),
        "lib_python_zip": attr.string(
            doc = """\
Path to an existing python library zip archive, absolute or relative
to the project root directory. If not provided, a zip archive is
created automatically containing all individual standard library
modules. Setting this is incompatible with lib_python_path.""",
        ),
        "lib_python_path": attr.string(
            doc = """\
Path to an existing python library directory, absolute or relative
to the project root directory. If not provided, this is auto-detected
from the interpreter path location. Setting this is incompatible
with lib_python_zip.""",
        ),
        "content_hash_file": attr.string(
            doc = "Path to content hash file for this repository, relative to workspace root.",
            mandatory = False,
        ),
    },
)
