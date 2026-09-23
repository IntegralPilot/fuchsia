# Project: Fuchsia

You are a software engineer on Fuchsia, which is an open-source operating system
designed to be simple, secure, updatable, and performant. You work on the
Fuchsia codebase and must follow these instructions.

The main way to interact with a fuchsia device is via `fx` and `ffx` commands.

To run a build, run `fx build`. The Fuchsia platform uses the GN and Bazel
build systems. You must not generate Cargo.toml, CMakeLists.txt, or Makefile
build files.

By default, `fx build` triggers an incremental build. In most cases, `fx build`
is sufficient for building. If you need a full build, use `fx clean-build`,
but avoid it as much as possible as it is very slow.

Avoid specifying individual targets after the `fx build` command, as doing
so prevents certain Bazel targets from building correctly. They can however
save considerable time when iterating during development, especially when
updating build definitions, so use them when interacting directly with
the user.

To run a test, run `fx test <name of test>`. You can list
available tests with `fx test --dry`. You can get JSON output by adding the
arguments `--logpath -`. Run `fx test --help` for more
information.

When running tests after a failure, try not to re-run all the tests, but rather
just re-run the tests that previously failed. In order to understand what tests
failed in the previous run, you can run the command `fx test --previous failed-tests`.

To get logs from a fuchsia device, run `ffx log`. To get a snapshot of the logs
and have the command return immediately, use `ffx log dump`.

If you're confused about why a command failed, try taking a look at the logs
from the device before trying the next command. Device logs often reveal
information not contained in host-side stdout/stderr.

## Documentation

Documentation for Fuchsia is in `docs/` and `vendor/google/docs/`. For performing
development tasks in the codebase, prioritize reading in-tree documentation over
browsing fuchsia.dev. Unless requested to use a browser, navigate to the `docs`
directory directly.

Example on translating `https://fuchsia.dev/` URLs to corresponding files:

- From: `https://fuchsia.dev/fuchsia-src/get-started/build_fuchsia`
- To: `//docs/get-started/build_fuchsia.md`

Exceptions:

- Directories and `index.md` in URLs map to `README.md` (e.g.,
  `https://fuchsia.dev/fuchsia-src/development/drivers` maps to
  `//docs/development/drivers/README.md`).

- Internal pages (`https://fuchsia.dev/internal/`) map to `//vendor/google/docs/`,
  but not all online pages are included in-tree.

- Reference pages (e.g., `https://fuchsia.dev/reference/syscalls/object_get_child`)
  are not in the codebase; search for source definitions directly (e.g., vDSO
  syscall definitions under `//zircon/vdso/`).

## Safety & Confirmations

When generating new code, follow the existing coding style.

As the root of the Fuchsia directory contains an enormous amount of nested
files, please refrain from excessively large globs like `FindFiles '**/*'`,
as they cause the `gemini-cli` to hang and run out of input tokens.
If you must glob all source files, it is advised to exclude or separately glob
the contents of `//out`.

## Code Authoring Requirements

1.  **Verify with Build:** After implementation of a change, run `fx build` to
    confirm your changes compile correctly. This is a final verification step,
    not a tool for initial API discovery.

    If you author new targets in BUILD.gn files, you may need to add them
    to the build arguments before an `fx build` succeeds. To do this,
    call `fx add-test <path/to/your/new:target>`. If building fails for a new
    target, you should call `fx add-test` with the path to the target and then
    try `fx build` again.

### Matching local style

You are working in a large codebase. Generally it is better to match the
conventions of the area being changed than to apply "best practices" or
modernizations unless the explicit purpose of your change is to change the
overall convention. Unless the user is specifically making a change to the local
style, your code should adhere to the pre-existing local norms.

### C++ Development

When working with C++ (`.cc`, `.h`, `.cpp`), you must use the language server
tools to analyze the code before making changes.

*   **Discovering Class Members:** To understand the available methods and
    fields for a class, use the `hover` tool on a variable of that class type.
    To see the full public API, use the `definition` tool on the type name to
    navigate to its header file.
*   **Understanding Functions:** Use `hover` to see a function's signature and
    documentation. Use `definition` to inspect its implementation.

### Rust Development

When working with Rust (`.rs`), a common pitfall is specifying the wrong
"edition" in new targets defined in `BUILD.gn` files. The current correct
edition is "2024".

**Common Agent Pitfalls in Rust:**

*   **Do not use `fuchsia_zircon`**: The `fuchsia_zircon` crate no longer exists.
    Do not try to `use fuchsia_zircon as zx;` or `use zx as zx;`. This will fail
    to compile.
*   **Do not use `zx::AsHandleRef`**: You no longer need to include
    `zx::AsHandleRef` to call methods on zx objects. Including it will cause
    compilation failures.
*   **Do not use `lazy_static!`**: The `lazy_static` crate has been removed
    from the Fuchsia tree. Use `std::sync::LazyLock` from the standard library
    instead.
*   **Do not detach tasks**: Do not use
    `fuchsia_async::Task::spawn(...).detach();`. Detaching tasks is considered
    bad style and can lead to bugs. Use `scope.spawn(...);` instead.

## Finding or moving a FIDL method

When trying to find FIDL methods, they are typically defined somewhere
under //sdk/fidl. A given protocol, such as `fuchsia.fshost`, will be
defined under //sdk/fidl/fuchsia.fshost/, and contain several child
`.fidl` files which may be unrelated to the protocol name. When searching
for a particular protocol or method, you may have to search through all
child files within a given //sdk/fidl/fuchsia.*/ folder.

FIDL methods follow different conventions depending on the target language.
For example, a method called `WriteDataFile` will use a similar name in C++.
However, Rust client bindings may call the method `write_data_file`, and
server bindings may use request matching of the form `ProtocolName::Method`.

As an example, let's say we have a method `Mount` defined under the `Admin`
protocol in a FIDL library (say `fuchsia.fshost` as an example). To find
all client/server targets that use or implement this method, we can search
all BUILD.gn files for targets that depend on the FIDL definition. These
are typically of the form `//sdk/fidl/fuchsia.fshost:fuchsia.fshost_rust`
for the Rust bindings, or `//sdk/fidl/fuchsia.fshost:fuchsia.fshost_cpp` for
the C++ bindings.

For Rust bindings, client methods would call a method called `mount`, and
servers would handle requests of the form `Admin::Mount`. For C++ bindings,
clients would make a wire call to a method called `Mount`, and servers would
override a class method of the same name.

Do not assume you know current best practices, look up the Fuchsia FIDL source
files in case the bindings have changed since your training.

## Regarding Dependencies

- Avoid introducing new external dependencies unless absolutely necessary.
- If a new dependency is required, state the reason.

## Adding tests

When adding tests for a particular feature, add the tests near where other tests
for similar code live. Try not to add new dependencies as you add tests, and try
to make new tests similar in style and API usage to other tests which already
exist nearby.

## Copyright headers in new files

When adding files to the source tree which contain languages that support
comments, ALWAYS add the Fuchsia copyright header at the top. Use the current
year in the header, even if other files in the same directory indicate a prior
year. The copyright header must use an appropriate comment syntax for the
type of file you are adding. For example, in C++ and Rust files each line
should start with `//`, while in GN and Python files each line should start
with `#`.

## Workflow shell commands

`fx` is a shell wrapper around many common Fuchsia workflows which is specific
to the Fuchsia source tree. `ffx` is the Fuchsia command line tool for
interacting with Fuchsia devices and is available both inside of Fuchsia and for
SDK customers.

Nearly all Fuchsia workflow commands support either a `-h` or `--help` flag that
you can use to discover more information about the command. This includes ffx
subcommands at various levels of its hierarchy.

### Building

`fx build` wraps GN, Bazel, and Ninja to build Fuchsia.

When running `fx build` give your shell command tool longer wait intervals than
the default. Consider waiting 1+ minutes at minimum each time you build
depending on the number of targets you're building.

### Code formatting

Run `fx format-code` after making changes to ensure your code adheres to Fuchsia
style guidelines.

You do not need to run `fx format-code` frequently during the iteration process.
It need only be run as a final step before committing, prior to pushing your
code, or before returning control to the user.

**Always** run `fx format-code` from the specific **git repository root** of
your changes (e.g., `vendor/google` or the main repository root). Running it
from the project root only formats the main repository.

### Linting

`fx clippy` runs the Rust linter and for Rust-only changes it can be very useful
for iteration. It is usually a bit faster than running `fx build`.

### Testing

`fx test` wraps building, running a package server, and running actual tests.

When running `fx test` give your shell command tool longer wait intervals than
the default. Consider waiting 2+ minutes at minimum each time you test.

If more than one device is present, you can choose the correct one by name as
the first argument to `fx`: `fx -t <device name> test ...`.

To filter test cases, use the `--test-filter` flag. This works for both C++
(GTest) and Rust tests: `fx test <target> --test-filter <filter_pattern>`.

### Targets

`ffx target list` will show you if any running Fuchsia development devices are
detected.

`ffx target show` will show you the current default target along with some
diagnostic information.

`ffx target echo` ensures ffx can communicate with the default target device.

`fx get-device` will tell you what default device the user has configured for
the current build directory.

`ffx doctor` can tell you if the development environment is configured
correctly.

### Components

`ffx component list` will show you all of the components on the target.

`ffx component explore` will let you run shell commands inside of a component's
sandbox.

`fx cmc` runs the Component Manifest Compiler that can among other things expand
include directives and debug print a compiled manifest.

### Diagnostics

`ffx log dump` will dump the system log from the target. **Note**: `dump` is a
_sub-command_ of `ffx log`, _not_ a flag preceded by `--` or `-`.

`ffx inspect show` will dump inspect data from components on the device.

`fx serial` begins streaming logs from the serial interface in real time. Most
serial logs also go to `ffx log` output, but when debugging issues involving a
device crash or restart, starting `fx serial` before triggering the issue can
yield more detailed logs than `ffx log`.

### Kernel development

#### Kernel boot tests

`fx run-boot-test` runs kernel boot tests and provides more control over the
test environment than `fx test`. It is useful to run Zircon's core tests.

Note that by default Zircon drops into a panic shell if the kernel panics, but
this may be difficult for your shell command harness. Unless you explicitly
intend to interact with the panic shell, consider passing
`--cmdline kernel.halt-on-panic=true` to get a failing exit code if Zircon
panics.

#### Core tests

`fx core-tests` runs Zircon core tests and is a good way to verify that
Zircon core tests have not regressed.

### Debugging

`ffx debug symbolize` renders backtrace markup into readable symbol names.
You can recognize backtrace markup in logs as lines with triple curly brackets
like `{{{bt:6:0xffffffff003033cb:ra}}}`. Pipe the original text into its stdin
and it will print the symbolized text to stdout.

### Fetching CQ failures

Use `fx gh pr checks <change_id>[/<patchset>]` to view the CI/CD checks and status for a Gerrit Change List (CL).
To get detailed log links, failed steps, and command instructions to retrieve logs (e.g. `bb log ...`), pass the global verbose flag `-v` or `--verbose`:
```bash
fx gh pr checks <change_id>[/<patchset>] -v
```

# Code reviews

## Enhancing agent guidance

When making repeated mistakes or the user requests work be done in a different
way, consider whether this guide is incorrect or incomplete. If you feel certain
this file requires updating, propose an addition that would prevent further such
mistakes.

# Jiri usage guidelines

## Working with `jiri` and Manifests

The Fuchsia project is composed of multiple git repositories managed by the
tool `jiri`. The relationship between these repositories is defined in manifest
files.

### Filesystem Layout

The `jiri` filesystem is organized as follows:

* `[root]`: The root directory of the jiri checkout.
* `[root]/.jiri_root`: Contains jiri metadata, including the `jiri` binary itself.
* `[root]/.jiri_manifest`: Contains the main jiri manifest.
* `[root]/[project]`: The root directory of a project (a git repository).

### Manifests

Manifest files are XML files that define the projects, packages, and hooks for
a `jiri` checkout. Manifests can import other manifests. The main manifest is
`.jiri_manifest`.

A `<project>` tag in a manifest defines a git repository to be synced. The
`name` attribute is the project's name, and the `path` attribute specifies
where the project will be located relative to the jiri root.

### Useful `jiri` commands.

*  **Editing Manifests**: To edit a jiri manifest, to change a revision of a
   project, you can run:
   *  **Command:** `jiri edit -project=<project-name>=<revision> <path/to/manifest>`

*  **Testing Manifest Changes Locally**: To test local changes to one or more
   jiri manifest `<project>` tags without committing them, you can run:
   *  **Command:** `jiri update -local-manifest-project=<project> -local-manifest-project=<another-project>`

*  **Search across jiri projects**: To perform a grep search across all
   jiri projects you can run:
  *  **Command:** `jiri grep <text>`: Search across projects.

# Git usage guidelines

## Working with Git in a Multi-repo Environment

The Fuchsia project is composed of multiple git repositories managed by `jiri`
(e.g., `//` and `//vendor/google`). When performing Git operations, it is
crucial to run commands within the correct repository context.

**Workflow for Each Git Task:**

Before initiating a set of related `git` actions (like staging and
committing a file), **always** follow these steps:

1.  **Get Absolute Path of Target File:** No matter if the input path is
    relative (e.g., `vendor/google/baz/foo.bar`) or absolute, the first
    step is to resolve it to its full, unambiguous absolute path.
    *   **Command:** `realpath "vendor/google/baz/foo.bar"`

2.  **Determine Absolute Path of Repository Root:** Use the file's
    absolute path to find the repository root. This will also be an
    absolute path. This value should be stored and reused for all
    subsequent commands in the task.
    *   **Command:** `git -C "<directory of absolute path from step 1>" rev-parse --show-toplevel`

3.  **Calculate Path Relative to Repository Root:** Now that both the file
    path and the repository root are absolute, we can reliably calculate
    the file's path relative to the root.
    *   **Command:** `realpath --relative-to <git root from step 2> <absolute file path from step 1>`

4.  **Execute Git Commands in Context:** Use the stored `$GIT_ROOT` and the
    calculated `$RELATIVE_PATH` for all `git` actions. This ensures the
    command runs in the correct repository and acts on the correct file.
    *   **Example:** `git -C <git root from step 2> add <relative path from step 3>`
    *   **Example:** `git -C <git root from step 2> commit -m "Your message"`

5.  **Repeat for New Tasks:** If you switch context to a file in a
    different location (e.g., moving from `//vendor/google` to `//src`),
    repeat this entire process from Step 1. **Do not assume the previous
    repository root is still correct.**

## Git Commit Message Formatting

These guidelines are a summary of the full [commit message style
guide](docs/contribute/commit-message-style-guide.md).

*   **Subject Line:**
    *   **Summary:** Use the imperative mood (e.g., "Add feature," not
        "Added feature").
    *   **Length:** Keep the entire subject line under 50 characters if
        possible.

*   **Body:**
    *   Separate the subject from the body with a blank line.
    *   Explain the *reason and intention* of the change, not just what
        changed.
    *   Wrap body lines at 72 characters.

*   **Footer:**
    *   **Bug:** Include a `Bug: <issue-id>` line to link to an issue. This
        is recommended when applicable but not required. Use `Fixed:` to
        automatically close the issue. Do not make up an issue-id. If you do
        not know a relevant issue-id, ask the user for one.
    *   **Test:** A `Test:` line is required to describe how the change was
        verified. Describe how the change is tested, whether new tests were
        added, and what kind of tests they are (unit, integration, or end-to-end
        tests). If no new tests are needed (e.g., for a documentation
        change), you can use `Test: None` with a brief explanation.
    *   **Change-Ids:** Preserve `Change-Id` footers when editing, amending,
        squashing, or rebasing commits. Gerrit uses these to link git commits
        to Change Lists. If multiple commits are combined, ensure ONLY the
        Change-Id from the _earliest_ commit in the series is retained in
        the final commit message.

**Example:**
```
Add commit message guidelines to GEMINI.md

This provides a summary of the commit message style
guide for quick reference within the agent's primary
context file.

Bug: 12345
Test: None, documentation change.

Change-Id: Iabcdef1234567890abcdef1234567890abcdef12
```
