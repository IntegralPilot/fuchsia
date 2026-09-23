# HOW BAZEL RUNFILES WORK

## ABSTRACT

A description of Bazel runfiles implementation, and how that relates to
exporting Fuchsia Bazel test artifacts for `fx test` and Fuchsia infra.


## GENERAL OVERVIEW

Sadly, Bazel runfiles are still not properly documented
(https://github.com/bazelbuild/bazel/issues/10022), so the following
is an overview of the current situation as of 2025Q1.

In many cases, Bazel executable artifacts need to access other build artifacts
or source files at runtime (i.e. during `bazel build` or `bazel test` invocations).

This is non-trivial because Bazel places build artifacts in mostly unpredictable
locations which depend on build configurations options (e.g. `-c opt`), as well
as Bazel implementation details (e.g. canonicalized repository paths). Moreover,
the Bazel `output_base` is usually stored in a location that is completely
independent from the location of the source tree.

This is solved by doing the following:

- During the analysis phase, Special `runfiles` values, which describe a set of files
  required at runtime execution, are computed and put into the `DefaultInfo`
  providers or executable targets.

  See https://bazel.build/extending/rules#runfiles for more details.

  The actual implementation is [more complicated than that][bazel-runfiles-guide]
  but that's good enough for this document.

- When building an executable artifact, Bazel may use the corresponding `runfiles`
  value to generate a manifest file, or a directory containing symlinks,
  describing the target's runfiles.

- The source code for the artifact must use a language-specific "runfiles library"
  which implements the logic necessary to lookup runtime-required files, using
  the manifest or directories created by Bazel.

  The lookup takes hard-coded paths values, such as `my_project/data/file` and
  translates that into the actual location of the corresponding artifact or source file.

Several runfile libraries are provided to implement the correct strategy
to locate runfiles. Their most important feature is providing an `rlocation()`
function, which translates an input path into a final target file path.

Bazel 7.x provides a number of libraries directly from its built-in `@bazel_tools`
repository, for example:

- `@bazel_tools//tools/cpp/runfiles`: for C++
- `@bazel_tools//tools/python/runfiles`: for Python
- `@bazel_tools//tools/java/runfiles`: for Java

However, these only support a few languages and are deprecated. Instead libraries
from dedicated rule sets should be used instead, for example:

- `@rules_cc//cc/runfiles`: for C++
- `@rules_python//python/runfiles`: for Python
- `@rules_go//go/runfiles`: for Go.
- `@rules_rust//tools/runfiles`: for Rust.
- `@rules_shell//shell/runfiles`: for Bash.

Unsurprisingly, without a correct specification, most of these libraries are
inconsistent in small details that can become important for exporting tests,
more details below :-/

## A SIMPLE EXAMPLE

Let's consider a trivial C++ program that wants to read a source data
file at runtime to process it. Let's assume for now that Bazel 8.0 is used,
that BzlMod is enabled, with the following project files:

```
## From `/tmp/test_project/MODULE.bazel`:
module(name = "my_project")

## From `/tmp/test_project/data/BUILD.bazel`:
exports_files(["file"])

## From `/tmp/test_project/data/file`:
Some data!

## /tmp/test_project/src/BUILD.bazel
cc_binary(
   name = "prog",
   srcs = [ "main.cc" ],
   data = [ "//data:file" ],
   deps = [ "@bazel_tools//tools/cpp/runfiles" ],
)

## /tmp/test_project/src/main.cc
#include <cstdio>
#include "tools/cpp/runfiles/runfiles.h"
using bazel::tools::cpp::runfiles::Runfiles;

int main(int argc, char* argv[]) {
  std::string argv0 = argv[0];
  std::string error;
  auto runfiles = std::unique_ptr<Runfiles>(
    Runfiles::Create(argv0, &error));
  if (!runfiles) {
    fprintf(stderr, "ERROR: %s\n", error.c_str());
    return 1;
  }
  std::string path = runfiles->Rlocation("my_project/data/file");
  printf("Found data file path: %s\n", path.c_str());
  return 0;
}
```

Then running `bazel build //src:foo` in the `/tmp/test_project` on Posix
will create something like this in the Bazel `output_base`:

```
# NOTE: OUTPUT_BASE would be an absolute path that looks like:
# /home/<username>/.cache/bazel/_bazel_<username>/<some-very-long-hexadecimal-hash>

${OUTPUT_BASE}/execroot/_main/bazel-out/k8-fastbuild/bin/  # bazel-bin symlink destination
    src/
        foo                   # the executable
        foo.repo_mapping
        foo.runfiles_manifest
        foo.runfiles/
            _main/
                src/
                    foo  --> symlink to ${OUTPUT_BASE}/.../bin/src/foo
                data/
                    file --> symlink to /tmp/test_project/data/file
            MANIFEST      --> symlink to ${OUTPUT_BASE}/.../bin/src/foo.runfiles_manifest
            _repo_mapping  --> symlink to ${OUTPUT_BASE}/.../bin/src/foo.repo_mapping
```

While on Windows, where symlinks are not supported by default, it generates something
slightly different:

```
${OUTPUT_BASE}/execroot/bazel-out/k8-fastbuild/bin/  # bazel-bin symlink destination
    src/
        foo                   # the executable
        foo.runfiles_manifest
        foo.repo_mapping
```

There is no runfiles directory, as it is replaced by a simple manifest file which
contains entries mapping input source paths to the corresponding target location.
For example:

```
## From bazel-bin/src/foo.runfiles_manifest
_main/data/file /tmp/test_project/data/file
_main/src/foo /home/username/.cache/bazel/...../bin/src/foo
_repo_mapping /home/username/.cache/bazel/...../bin/src/foo.repo_mapping
```

## REPOSITORY MAPPING FILES

Notice that the C++ code called the `Rlocation()` function with a fixed input
path of `my_project/data/file`, while the manifest (or runfiles directory)
uses `_main/data/file` instead as the source path key.

This is working as intended, as the first path segment of an rlocation input
path (here `my_project`), must be a workspace or repository *apparent name*,
while the manifest or runfiles directory uses *canonical names* instead as
source path keys.

See https://bazel.build/external/overview#apparent-repo-name for details
about apparent vs. canonical repository names.

The `foo.repo_mapping` file is used to perform this translation. It stores
the equivalent of a `{ (source_name, apparent_name) -> canonical_name }`
dictionary, where:

- `source_name` is either an empty string (for the main workspace), or a
  repository canonical name.

- `apparent_name`, is the apparent name of the workspace or of a repository,
  name to be translated, as seen inside of `source_name`.

- `canonical_name` is either the hard-coded `_main` value, for the main
  workspace, or a repository canonical name.

In the example above, the `foo.repo_mapping` only contains a single line:

```
,my_project,_main
```

Which translates to: replace `my_project` with `_main` for code that was
compiled from targets defined in the main workspace.

A more complex example that involves code from external repositories
generated by a custom module extension would look like:

```
,my_project,_main
,my_repo,+custom_repo_ext+the_repo
+custom_repo_ext+the_repo,my_project,_main
+custom_repo_ext+the_repo,the_repo,+custom_repo_ext+the_repo
```

## RUNFILES DIRECTORY LAYOUT AND MANIFEST FORMAT

When building an executable artifact `foo`, on Posix, the following sibling
entries are also created:

- `foo.runfiles_manifest`: A manifest describing the runfiles.
- `foo.repo_mapping`: A repository mapping file.
- `foo.runfiles`: A directory containing entries for the runfiles.

The `foo.runfiles` directory contains:

- A `MANIFEST` file, which is a copy of or a symlink to `foo.runfiles_manifest`.
- A `_repo_mapping` symlink to `foo.repo_mapping`.
- Regular files for a few special cases (e.g. empty Python `__init__.py` files).
- Symlinks pointing to artifacts in the output base or to workspace sources

Most importantly, all symlinks created by Bazel in the runfiles directory use
*absolute target paths*.

This is convenient because it ensures that the code executed at runtime doesn't
need to worry about the current working directory to find the files.

In most cases, the manifest reflects the structure of the runfiles directory,
**HOWEVER** it can use a mix of *absolute* and *execroot-relative* target paths.

These relative path can look like
`bazel-out/<config-specific-dir>/bin/<package-segments>/<name>`
where `<config-specific-dir>` is some unpredictable value.

By default, `bazel run` and `bazel test` change the current directory to the
`execroot` before invoking the executables, so the runtime lookup will still
work in this case. But it makes the compiled binaries fail to run properly
when invoked directly (for example with `bazel-bin/<package-segments>/<name>`
following a `bazel build` command), or when copied to a different location.

On Windows, because symlinks are not supported by default, no `foo.runfiles`
directory is ever created. Instead, only the manifest exists. This also has
the drawback that empty `__init__.py` files are not available at runtime.
This is worked-around by creating Python executable .zip archives on this
platform instead :-(

The manifest file format is relatively simple:

- Each line maps a source path to a target path.

- The source path is relative to the runfiles directory root
  (e.g. `_main/src/foo`), while the target path can be either
  an absolute path, or a path relative to the Bazel `execroot`
  path.

- If there are no spaces, newlines or backslashes in either
  the source and target paths, each line follows the simple
  format of `<source_path> <space> <target_path>`.

- Otherwise, the line begins with the space and contains escaped
  versions of the paths, as in
  `<space> <escaped_source> <space> <escaped_target>`.

  Escaped source paths replace spaces with "\s", newlines with
  "\n", and backslashes with "\b".

## HOW RUNFILES LIBRARIES REALLY WORK

Runfiles libraries need to locate the runfiles directory and/or manifest
when they are initialized. Due to the lack of consistent spec, they all
use slightly different logic to do that, but which amounts to the following:

- If a `RUNFILES_DIR` environment variable is defined, use that to point
  to the runfiles directory.

- If a `RUNFILES_MANIFEST_FILE` environment variable is defined, use that
  to point to the runfiles manifest.

- Otherwise, try to probe the filesystem. E.g. looking for a
  `$0.runfiles_manifest` file, or a `$0.runfiles` directory.

- Runfile libraries are inconsistent in their behavior when both
  `$0.runfiles_manifest` and `$0.runfiles/MANIFEST` exist. Some prefer
  to use one of the other. Due to this, it is important that both files
  be the same content, if they both exist.

- There is additional probing to find the repository mapping file,
  but no environment variable is provided. Libraries tend to search
  for `$RUNFILES_DIR/_repo_mapping`, `$0.repo_mapping` and other
  inconsistent heuristics.

  Note that the `_repo_mapping` file seems to always be listed in the
  manifest, but no runfiles libraries that were inspected actually use
  that to locate the file. This could still happen in a future
  implementation though.

When both a runfiles directory and a manifest are available. Whether
the manifest or the directory is used for lookups, or in which order,
varies with the runfiles library implementation. Hence it is critical
that the manifest and directory content be consistent, or some targets
may fail in surprising ways.

NOTE: Some runfiles libraries are obsolete and do not support repository
mapping, for example `@bazel_tools//tools/python/runfiles` does not,
even in Bazel 8.x sources, and should avoided in all project files,
in favor of `@rules_python//python/runfiles`.

As a consequence, updating Bazel `rules_xxx` repositories to recent
versions is probably critical to avoid unexpected problems when running
tests.

## MIDDLE-MAN SCRIPTS

Bazel rules for Python and shell scripts generate special launcher
scripts for executable targets (e.g. `py_binary()` or `sh_binary()`)
that are called "middle-man" scripts.

What these do is setup the environment to run the real script. In
practice what they do is:

- Verify whether `RUNFILES_DIR` or `RUNFILES_MANIFEST_FILE` are
  defined in the environment, and if not, probe the filesystem
  to find the corresponding locations (if they exist), and define
  the environment variables before calling the real script.

  This ensures more consistent behavior of the runfiles libraries
  when they are called further down.

- For Python, this also modifies the PYTHONPATH to ensure
  `import` statements for `py_library()` dependencies work correctly.
  It also locates the Python interpreter (in the case of a custom
  Python toolchain runtime), handles executable zip archives on
  Windows and other book-keeping.

For example, assume an `py_binary(//src:foo)` target that depends
on a `py_library(//lib:dependency)` library. The runfiles layout
would look like:

```
...../bin/
  src/
    foo              # python middle-man for //src:foo py_binary()
                     # modifies PYTHONPATH to insert .../foo.runfiles/lib
                     # before calling .../foo.runfiles/src/foo.py

    foo.repo_mapping
    foo.runfiles_manifest
    foo.runfiles/
        _main/
            __init__.py
            src/
                __init__.py
                foo.py    # the real python script to run.
            lib/
                __init__.py
                dependency.py
        MANIFEST
        _repo_mapping
```

On Windows, the "middle-man" is actually a compiled Win32 executable
that does the same thing, since no shell script can be assumed to be
available. Due to this, simply building Python code on Windows with
Bazel requires a C++ host toolchain being available too!

## C++ RUNTIME SHARED LIBRARY DEPENDENCIES

On Posix, when compiling a `cc_binary()` target that depends on a
`cc_shared_library()` target defined in a different package, Bazel
hard-codes special relative paths in the executable's  dynamic section.

For example, if `//:bin` target, that is a `cc_binary()` which depends on
a `//:foo_shared` target, which is a `cc_shared_library()`, the output
base will look like the following, where `${BAZEL_BIN}` expands to a
a very long absolute path, that the top-level `bazel-bin` symlink points
to as well:

```
bazel-bin
├── libfoo_shared.so
├── prog
├── prog.repo_mapping
├── prog.runfiles_manifest
├── prog.runfiles
│   ├── _main
│   │   ├── libfoo_shared.so -> ${BAZEL_BIN}/libfoo_shared.so
│   │   ├── prog -> ${BAZEL_BIN}/prog
│   │   └── _solib_k8
│   │       └── _U
│   │           └── libfoo_shared.so -> ${BAZEL_BIN}/libfoo_shared.so
│   ├── MANIFEST -> ${BAZEL_BIN}/prog.runfiles_manifest
│   └── _repo_mapping -> ${BAZEL_BIN}/prog.repo_mapping
└── _solib_k8
    └── _U
        └── libfoo_shared.so -> ${BAZEL_BIN}/libfoo_shared.so
```

While the dynamic section of the `prog` ELF binary will contain a line
specifying a library runtime search path list:

```
 0x0000000000000001 (NEEDED)             Shared library: [libfoo_shared.so]
 0x0000000000000001 (NEEDED)             Shared library: [libc.so.6]
 ...
 0x000000000000001d (RUNPATH)            Library runpath: [$ORIGIN/_solib_k8/_U:$ORIGIN/prog.runfiles/_main/_solib_k8/_U]
```

I.e. the dynamic loader, when trying to load the executable, will look into
the directories `_solib_k8/_U` and `prog.runfiles/_main/_solib_k8/_U` for
dependencies (`libfoo_shared.so` and `libc.so`), and will otherwise fallback
to its standard library search paths.

The first path makes calling `bazel-bin/prog` and `bazel-bin/prog.runfiles/_main/prog` directly
work, as this will pick up the shared library artifact, or the symlink pointing to it.

The second path is necessary for remote execution. In this mode, Bazel will only upload
the executable artifact and its runfiles to the remote sandbox. More precisely:

`SANDBOX/bazel-out/<config_dir>/bin/prog` will be a copy of the executable.
`SANDBOX/bazel-out/<config_dir>/bin/prog.runfiles`: will contain copies of the runfiles,
not symlinks.
`SANDBOX/bazel-out/<config_dir>/bin/prog.runfiles/_solib_k8/_u/libfoo_shared.so`: will be the actual shared library location.

The program will be called from the `SANDBOX` directory, with a path of
`bazel-out/<config_dir>/bin/prog`, to ensure relative Rlocation files work.

## HOW FUCHSIA INFRA RUNS HOST TESTS

In a nutshell:

- Each test (Fuchsia or host) has a corresponding entry in the `tests.json`
  file generated at `fx gen` time. Currently, this only contains entries from
  the GN graph.

  For host tests, each entry uses the `path` key to point to executable,
  relative to the Ninja build directory, and the `runtime_deps` key to point
  to a JSON file that contains a list of runtime requirements (also paths
  relative to the Ninja build directory).

- The `tests.json` file is uploaded by the builder and later sent to the
  [testsharder] tool, which splits it into separate files (each one targeting
  a specific builder shard).

  It also generates a manifest to upload test binaries and their runtime
  dependencies to content-addressed cloud storage using the `cas` tool.

  *Hence symlinks are never uploaded, only the data they point to*.

- On at least one test bot, the `botanist` command uses [testrunner] to run
  host tests in an `nsjail` sandbox.

  The sandbox is populated by downloading files from the content-addressable
  cloud storage, using the target paths from the `testsharder` file output,
  i.e. they will be relative to the current directory when the test is invoked.

## EXPORTING BAZEL TESTS TO THE FUCHSIA BUILD DIRECTORY

First, use `$NINJA_BUILD_DIR/bazel_host_tests/` as the top-level export directory
for all tests copied to the Fuchsia build directory. Do not include the
`bazel-out/<config_dir>/bin` portion of the Bazel artifact and runfile paths
in the output, but keep the package / workspace prefixes, as in the following
diagram, where `TARGET_OF(symlink_path)` points to the same location as
`symlink_path`, removing one layer of indirection.

```
$NINJA_BUILD_DIR/bazel_host_tests/
    src/
        foo.runtime_deps.json  # Describes runtime requirements for `tests.json` / `fx test` / `botanist`.
        foo                    ---> TARGET_OF($BAZEL_BIN/src/foo)
        foo.repo_mapping       ---> $BAZEL_BIN/src/foo.repo_mapping
        foo.runfiles_manifest  # Exported manifest file (see below)
        foo.runfiles/
            _main/
                __init__.py        # Empty file (as in original $BAZEL_BIN/src/foo.runfiles/ directory)
                src/
                    __init__.py    # Empty file.
                    foo.py         ----> TARGET_OF($BAZEL_BIN/src/foo.runfiles/_main/src/foo.py)
                lib/
                    __init__.py
                    dependency.py
        MANIFEST               ----> Copy of../foo.runfiles_manifest (!!)
        _repo_mapping          ----> $BAZEL_BIN/src/foo.repo_mapping
```

The corresponding `tests.json` entry will use the following keys and values:

```
{
  "environments": { ... }
  "test": {
    "label": "@//src:foo",
    "path": "bazel_host_tests/src/foo",
    "name": "bazel_host_tests/src/foo",
    "runtime_deps": "bazel_host_tests/src/foo.runtime_deps.json",
    "os": "linux",
    "cpu": "x64",
  }
}
```

The `foo.runtime_deps.json` will contain paths for all runtime dependencies relative
to the Ninja build directory, e.g.:

```
bazel_host_tests/src/foo.repo_mapping
bazel_host_tests/src/foo.runfiles_manifest
bazel_host_tests/src/foo.runfiles/_main/__init__.py
bazel_host_tests/src/foo.runfiles/_main/src/__init__.py
bazel_host_tests/src/foo.runfiles/_main/src/foo.py
bazel_host_tests/src/foo.runfiles/_main/src/lib/__init__.py
bazel_host_tests/src/foo.runfiles/_main/src/lib/dependency.py
bazel_host_tests/src/foo.runfiles/MANIFEST
bazel_host_tests/src/foo.runfiles/_repo_manifest
```

IMPORTANT: It is assumed that all tests are run from the Ninja build directory
and/or the test sandbox working directory, which acts as one.
The `foo.runfiles_manifest` file must be adjusted to only include target paths
that are relative to them, assuming, for remote execution, that only the `.runfiles`
directory and the executable are available. For example, this could look like
(with the space separator expanded to print a columnar output):

```
_main/__init__.py             bazel_host_tests/src/foo.runfiles/_main/__init__.py
_main/src/__init__.py         bazel_host_tests/src/foo.runfiles/_main/src/__init__.py
_main/src/foo.py              bazel_host_tests/src/foo.runfiles/_main/src/foo.py
_main/src/lib/__init__.py     bazel_host_tests/src/foo.runfiles/_main/src/lib/__init__.py
_main/src/lib/dependency.py   bazel_host_tests/src/foo.runfiles/_main/src/lib/dependency.py
_repo_mapping                 bazel_host_tests/src/foo.runfiles/repo_mapping
```

This is easy to generate from the `foo.runtimes_deps.json`, the only major difference is
that the `foo.runfiles/MANIFEST` file does not need to list itself, or the files not under
`foo.runtiles`, so will contain fewer entries than `foo.runtime_deps.json`.

[bazel-runfiles-guide]: https://docs.google.com/document/d/1wzjTCuqXNKBlEEHCVw17j7kil2qx04sBsMZCTZzOJj0/edit?tab=t.0#heading=h.rpcqrjsxgnpk
[testsharder]: https://cs.opensource.google/fuchsia/fuchsia/+/main:tools/integration/testsharder/
