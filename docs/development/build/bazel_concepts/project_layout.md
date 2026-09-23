# Bazel project layout and organization

## Bazel projects

A Bazel project is a collection of source and build files that describe:

- How to *build* artifacts, like binaries or data files, and their dependencies.
- How to *run* specific commands, i.e. scripts or executables.
- How to *test* said build artifacts.

A Bazel project is materialized by _a top-level directory_, whose
content follows a _specific layout_ and _conventions_.

Note: Bazel does not store build artifacts in the project's directory.
      For more details, see [this page][bazel-build-outputs].

## Bazel workspaces

A Bazel workspace is a directory tree that contains a top-level
`WORKSPACE.bazel`[^1] file. This defines the root of a collection of sources
and related build files. A Bazel project can use several workspaces:

- The project's root directory is called the _root_ _workspace_, and thus must
  contain a `WORKSPACE.bazel` file.

- A project can also reference _other workspaces_, called _external_
  _repositories_, which correspond to third-party project dependencies.

For example, in:

```
/home/user/project/
    WORKSPACE.bazel
    src/
        BUILD.bazel
        extra/
            extra.cc
        lib/
            BUILD.bazel
            foo.cc
            foo.h
        main.cc
```

The directory `/home/user/project` is a root Bazel workspace, which contains
all the files inside it.

The `WORKSPACE.bazel` file can be empty, but can also contain directives
that reference other workspaces, as explained later.

[^1]: For legacy reasons, this file can also be simply called `WORKSPACE`

## Bazel packages

Within a workspace, a directory that contains a `BUILD.bazel`[^2] file defines
a _package_, which is a boundary around a collection of source files and items
that Bazel knows about.

For example, this file layout:

```
/home/user/project/
  WORKSPACE.bazel
  BUILD.bazel
  main.cc
```

Defines a root workspace, located at `/home/user/project` with a single
top-level package, which contains the files `BUILD.bazel` and `main.cc`.

The `BUILD.bazel` file can also contain directives to define named _items_,
such as targets, config conditions and others, that also technically belong
to the package.

Several packages can exist in a single workspace, and _each file can only
belong to one package_. For example, with the following file layout:

```
/home/user/project/
  WORKSPACE.bazel
  BUILD.bazel
  main.cc
  lib/
    BUILD.bazel
    foo.cc
```

The root workspace contains two different packages:

- The top-level package, which still contains the files `BUILD.bazel`
  and `main.cc` (relative to the root workspace directory).

- A second package, which contains the files `lib/BUILD.bazel` and
  `lib/foo.cc`.

Note that the file at `/home/user/project/lib/foo.cc` only belongs
to the second package, not to the first one. This is because
_package_ _boundaries_ _never_ _overlap_.

[^2]: Also for legacy reasons, the file can also be simply called `BUILD`.

## `BUILD.bazel` versus `BUILD`

Bazel originates from Google's Blaze, which only works on Linux with
case-sensitive filesystems. Blaze only used the file name `BUILD` to store
build directives.

However, Bazel also needs to run on Windows which has
_case-insensitive_ filesystems, and many Google, or non-Google projects
already use a directory named "`build`", which then collides with a file
named "`BUILD`" on such systems.

To solve the issue, Bazel uses `BUILD.bazel` and `WORKSPACE.bazel` as the
default file names, while still supporting `BUILD` and `WORKSPACE` as fallbacks.

Note: For portability, prefer `BUILD.bazel` over `BUILD`, and `WORKSPACE.bazel`
      over `WORKSPACE` in your own Bazel projects.

## Workspace directives

The root `WORKSPACE.bazel` can be empty, or it can contain directives which
reference other Bazel workspaces, which are called _external repositories_.
These directives always give a name to the repository. For example:

```py
local_repository(
  name = "my_ssl",
  path = "/home/user/src/openssl-bazel",
)
```

Associates the name `my_ssl` with the workspace located at
`/home/user/src/openssl-bazel` on the build machine. This directory must also
contain a `WORKSPACE.bazel` (or `WORKSPACE`) file.

A repository is just an external Bazel workspace with a name. This name is
local to your project, and can later be used to reference items from the
external workspace (see below).

Bazel also supports other directives to download repositories from the
network, or even generate their content programmatically.

## Bazel labels

Bazel labels are _string references_ to source files and items defined in
`BUILD.bazel` files. Their general format is:

```
@<repository_name>//<package_name>:<target_name>
```

Where:

- `@<repository_name>//` designates the directory of a named Bazel workspace.

  As a convenience, this can be abbreviated as simply **`//`** for the current
  workspace (the one that contains the current `BUILD.bazel` file). Note also
  that **`@//`** is used to designate the project's root workspace, even when
  used in external repositories.

- `<package_name>` is the package's directory path, relative to the
  workspace directory. For example, in the labels `//src:main.cc` or
  `//src/lib:foo`, the package names are `src` and `src/lib` respectively.

  This can be empty, e.g. `//:BUILD.bazel` points to the build file in the
  current workspace's top-level directory.

- For source files, `<target_name>` is the file path relative to its parent
  package's directory, and may include a sub-directory part. For example for
  `//src:main.cc`or `//src:extra/extra.cc`, the target name is `main.cc` and
  `extra/extra.cc` respectively.

  Note: Bazel _target names_ can include sub-directories, which are always
  relative to the package they belong to.

- For other items, `<target_name>` corresponds to an item (build artifact,
  build setting, configuration condition, etc) defined in a `BUILD.bazel`
  file.

  By convention, its `name` attribute  _should not include a directory
  separator_, except in very rare cases, to avoid confusion with sources.

  Note: The Bazel documentation uses the term _target_ to refer to _any_
  item that can be reached with a label. Many of these _do not correspond to_
  _buildable artifacts_ (source files, build variable definitions,
  configuration conditions, platform constraints, and many more).

  This can be confusing to developers coming from other build systems which
  differentiate the type of items in their build graph (e.g. `GN` uses
  "Targets", "Configs", "Toolchains" and "Pools" to designate different
  things).

Shortened expressions for labels are also supported:

- If the label begins with a repository name  and does not include a colon,
  it is a package path, and points to an item with the same name.
  For example `//src/foo` is equivalent to `//src/foo:foo`.

- If the label begins with a colon, it is a name relative to the current
  package. For example "`:bar`" and "`:extra/bar.cc`" that appear in
  `src/foo/BUILD.bazel` are equivalent to `//src/foo:bar` and
  `//src/foo:extra/bar.cc` respectively.

- If the label has no repository name and no colon, it is always a name
  relative to the current package, even if it includes a directory separator.
  E.g. "`bar/bar.cc`" in `src/foo/BUILD.bazel` always refer to
  `//src/foo:bar/bar.cc`.

  Note that this is not the same as `//src/foo/bar:bar.cc`

### Relative labels and package ownership

Since each source file can only belong to a single package, relative labels can
be invalid. For example, in a project that looks like the following:

```
/home/user/project/
    WORKSPACE.bazel
    src/
        BUILD.bazel
        main.cc
        extra/
            extra.cc
        lib/
            BUILD.bazel
            foo.cc
            foo.h

```

The `foo.cc` file belongs to the package `src/lib`, so its label _must be_
`//src/lib:foo.cc`.

Using a label like `src:lib/foo.cc` in `src/BUILD.bazel` is an error:

```py
# From src/BUILD.bazel
cc_binary(
  name = "program",
  srcs = [
    "extra/extra.cc",
    "lib/foo.cc",       # Error: Label '//src:lib/foo.cc' is invalid because 'src/lib' is a subpackage
    "lib/foo.h"         # Error: Label '//src:lib/foo.h' is invalid because 'src/lib' is a subpackage
    "main.cc",
  ],
)
```

### Source file access from other packages

By default, the source files of a given package cannot be accessed from
other packages, and _relative package labels are invalid_, as in:

```py
# From src/BUILD.bazel
cc_binary(
  name = "program",
  srcs = [
    "extra/extra.cc",
    "lib:foo.cc",    # Error: invalid label 'lib:foo.cc': absolute label must begin with '@' or '//'
    "lib:foo.h"      # Error: invalid label 'lib:foo.h': absolute label must begin with '@' or '//'
    "main.cc",
  ],
)
```

And even when using the right absolute label, and error happens:

```py
# From src/BUILD.bazel
cc_binary(
  name = "program",
  srcs = [
    "extra/extra.cc",
    "//src/lib:foo.cc",    # Error: no such target '//src/lib:foo.cc': target 'foo.h' not declared in package 'src/lib'
    "//src/lib:foo.h"      # Error: no such target '//src/lib:foo.h': target 'foo.h' not declared in package 'src/lib'
    "main.cc",
  ],
)
```

Direct access to files across package boundaries can be granted by `export_files()`:

```py
# From src/lib/BUILD.bazel
export_files([
  "foo.cc" ,
  "foo.h" ,
])

# From src/BUILD.bazel
cc_binary(
  name = "program",
  srcs = [
    "extra/extra.cc",
    "//src/lib:foo.cc",    # OK
    "//src/lib:foo.h"      # OK
    "main.cc",
  ],
)
```

### Target access from other packages

Labelled items defined in a `BUILD.bazel`  file that are not source files
need no export, but their `visibility` attribute must allow their use
outside of their own package:

```py
# From src/lib/BUILD.bazel
cc_library(
  name = " lib" ,
  srcs = [ " foo.cc"  ],
  hdrs = [ " foo.h"  ],
  visibility = [ " //visibility:public" ],  # Anyone can reference this directly!
)

# From src/BUILD.bazel
cc_binary(
  name = "program",
  srcs = [
    "extra/extra.cc",
    "main.cc",
  ],
  deps = [ "lib" ],   # OK!
)
```

By default, items are only visible to other items in the same package.
This can be changed by using a [`package()`][bazel-package]{:.external}
directive to change the default visibility of all items defined in a package:

```py
# From src/lib/BUILD.bazel

# Ensure that all items defined in this file are visible to anyone
package(default_visibility = ["//visibility:public"])

cc_library(
  name = " lib" ,
  srcs = [ "foo.cc" ],
  hdrs = [ "foo.h" ],
)

# From src/BUILD.bazel
cc_binary(
  name = "program",
  srcs = [
    "extra/extra.cc",
    "main.cc",
  ],
  deps = [ "lib" ],   # OK!
)
```

### A warning about virtual packages

Avoid creating top-level directories in a project with the following names:

- `conditions`
- `command_line_option`
- `external`
- `visibility`

Because Bazel uses a number of _hard-coded "virtual packages"_ in labels within
`BUILD.bazel` files. For example:

```py
  //visibility:public
  //conditions:default
  //command_line_option:copt
```

The case of `external` is a bit different: it does not appear in
`BUILD.bazel` files, but used internally to manage external repositories.
[This confuses Bazel when used as a project directory][bazel-external-bug]{:.external}

### Canonical repository names

Since Bazel 6.0, repository names in labels can also begin with **`@@`**.

When the optional [BzlMod][bzlmod] feature is enabled, these labels
are used as alternative but _unique_ label names for external repositories,
which becomes important when complex transitive dependency trees are used in a
project.

For example, `@@com_acme_anvil.1.0.3` could be a canonical name for the
workspace directory identified by `@anvil` in the project's own `BUILD.bazel`
files, and by `@acme_anvil` when it appears in an external repository
(e.g. inside `@foo//:BUILD.bazel`). All three labels would refer to the content
of the same directory.

Canonical repository names do not appear in `BUILD.bazel` files, however, they
will appear during the analysis phase (when executing Starlark functions that
look at label values), or when looking at the result of Bazel queries.

## Bazel extension (`.bzl`) files

Extension files contain extra definitions that can be imported into several
other files:

- Their name always ends with the `.bzl` file extension.

- They must belong to a Bazel package, and hence identified by a label.
  For example `//bazel_utils:defs.bzl`.

- They are written in the [Starlark language][starlark-language]{:.external},
  and should follow [specific guidelines][bzl-style-guide]{:.external}.

- They are the _only_ place where Starlark functions can be defined!
  In other words, one cannot define a function in a `BUILD.bazel` file!

- They are always _evaluated once_, even if they are imported multiple times,
  and the variables and functions they define are recorded as constants.

- They can be imported from other files using the
  [`load()`][bazel-load]{:.external} statement.

For example:

- From `$PROJECT/my_definitions.bzl`:

  ```py
  # The official release number
  release_version = "1.0.0"
  ```

- From `$PROJECT/BUILD.bazel`:

  ```py
  # Import the value of `release_version` from my_definitions.bzl
  load("//:my_definitions.bzl", "release_version")

  # Compile C++ executable, hard-coding its version number with a macro.
  cc_binary(
    name = "my_program",
    defines = [ "RELEASE_VERSION=" + release_version ],
    sources = [ … ],
  )
  ```

The [`load()`][bazel-load]{:.external} statement has special semantics:

- Its first argument must be a label string to a `.bzl` file (e.g.
  `"//src:definitions.bzl"`).

- Other arguments name imported constants or functions:

    - If the argument is a string, it must be the name of an imported symbol
      defined by the `.bzl` file. e.g.:

      ```py
      load("//src:defs.bzl", "my_var", "my_func")
      ```

    - If the argument is a variable assignment, it defines a local alias for an
      imported symbol. E.g.:

      ```py
      load("//src:defs.bzl", "my_var", func = "my_func")
      ```

    - _There are no wildcards_: all imported constants and functions must be
      named explicitly.

    - Imported symbols are never recorded when the `load()` appears within a
      `.bzl` file.

    - Similarly, symbols whose name begins with and underscore (e.g. `_foo`) are
      never recorded, and cannot be imported. I.e. they are private to the `.bzl`
      file that defines them.

Sometimes a `.bzl` file wants to import a symbol from another one, and
re-export it with the same name. This requires an alias as in:

```py
# From //src:utils.bzl

# Import "my_vars" from defs.bzl as '_my_var'.
load("//src:defs.bzl", _my_var = "my_var")

# Define my_var in the current scope as a copy of _my_var
# This symbol and its value will be recorded, and available for import
# to any other file that loads //src:utils.bzl.
my_var = _my_var
```

[bazel-build-outputs]: /docs/development/build/bazel_concepts/build_outputs.md
[bazel-external-repositories]: /docs/development/build/bazel_concepts/external_repositories.md
[bazel-external-bug]: https://github.com/bazelbuild/bazel/issues/16220
[starlark-language]: https://bazel.build/rules/language
[bzl-style-guide]: https://bazel.build/rules/bzl-style
[bazel-load]: https://bazel.build/concepts/build-files#load
[bzlmod]: https://bazel.build/external/overview#bzlmod
[bazel-package]: https://bazel.build/reference/be/functions#package
