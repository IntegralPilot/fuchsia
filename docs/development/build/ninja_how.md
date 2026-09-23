# How Ninja works

This page provides an overview of how [Ninja][ninja]{:.external} works in
Fuchsia.

> **Note:**
> Most developers should interact with the build system through [`fx` workflows](fx.md)
> (such as `fx build` and `fx set`) rather than running GN, Ninja, or Bazel directly
> (even via wrappers like `fx gn` or `fx ninja`).

## General overview

The Fuchsia build system uses Ninja to launch build commands in parallel.
The following steps describe Ninja's behavior:

1. Load the [Ninja build plan](#build-plan), from a top-level `build.ninja`
   file which can itself include several other `.ninja` files.

   In the Fuchsia build, these are created by the [GN][GN]{:.external} build
   tool. This operation builds a [dependency graph](#build-graph) in memory.

2. Load the Ninja build log and deps log if they exist.

   This operation adds dependency edges that were discovered during the previous
   successful Ninja build invocation. This allows fast incremental builds, but at
   [some cost to correctness](#depfile-correctness-issues).

3. Determine which build outputs (also known as "targets") need to be generated.

   Starting from the targets named on the command-line, recursively walk their
   dependencies to determine which final and intermediates outputs are stale
   relative to their inputs, and thus need to be rebuilt. Commands that need to
   be re-run are correctly ordered in a directed acyclic graph.

4. Launch required build commands in parallel based on the number of CPUs
   on the host system (or an explicit `-j<count>` parameter).

   Another way to control parallelism is to use `-l<max_load>` to limit the max
   load value on the system. A command is ready to run when the inputs that
   `ninja` knows about have been updated.

Note: For Ninja, a "target" is simply a file path, relative to the build
directory, that appears in the dependency graph. In particular, source files
and command outputs are all Ninja targets. This differs significantly from
other build systems like GN or Bazel where a "target" models something else[^1].

[^1]: More specifically, the Ninja dependency graph is very similar to the [Bazel action graph][bazel-action-graph]{:.external}, and a Ninja target corresponds to a Bazel File object.

## Status display

During a build, Ninja launches multiple commands in parallel, and by default,
it will buffer their output (stdout and stderr) until their completion.

Ninja also prints a status line (for instance, when `fx build` is used) that
describes the following:

- The number of commands that have completed.
- The total number of commands that must be run to complete the build[^2]
- The number of currently running commands.
- A description of the *last-completed* command. Which typically includes
  a small mnemonic (for example, `ACTION` or `CXX`) followed by a list of
  output targets.

[^2]: This number can decrease during the build if Ninja determines that [the outputs of some commands are considered up-to-date](#restat).

```none  {:.devsite-disable-click-to-copy}
[102/345](36) ACTION path/to/some/build/artifact
```

The example above means that 102 commands have been completed so far, out of
345, and that there are currently 36 parallel commands launched by Ninja, while
`path/to/some/build/artifact` is the latest build artifact to be generated.

It is possible to customize the content of the status line by setting the
`NINJA_STATUS` [environment variable][ninja-environment-variables]{:.external}.

If any command generates some output, or if it fails, Ninja will update the
status line with that command's description, then print its output or an
error message. It will then resume printing the status line after it, for
example:

```none  {:.devsite-disable-click-to-copy}
[102/345](24) ACTION path/to/some/build/artifact
<output of the command which generated 'path/to/some/build/artifact'>
[101/345](23) ACTION path/to/another/build/artifact
```

In practice, this is how most compiler warnings are printed.

Note: By default, when a long command stalls the build, Ninja does *not*
*print* *anything* and does not update its status line (which still shows a
description of the _previously_ completed command). This is a common source
of confusion for developers. The stalling command is never printed until it
is completed, and most of the time its description is immediately overwritten
by that of the next command after it.

As a special exception, if a command is in the special
[`console` pool][ninja-console-pool]{:.external} it will be able to print
directly to the terminal. This is useful for long-running commands that need
to print their own status updates.

Ninja ensures that only one console command can be launched at the same time,
and also suspends its own status line updates until its completion. However,
note that other non-console commands are still run in parallel in the
background, and their outputs buffered. The Fuchsia build uses this feature for
all commands that invoke Bazel, since these tend to be long, and Bazel provides
its own status updates to the terminal.

### Fuchsia-specific status display

As a special Fuchsia-specific improvement, Ninja will display a table of
the oldest long-running commands, with their run time, to better understand
what's going on during a build. This is only enabled in smart terminals.

Set `NINJA_STATUS_MAX_COMMANDS=<count>` in your environment to change the
number of commands being displayed. `fx build `sets its default to 4, which
looks like;

```none  {:.devsite-disable-click-to-copy}
[0/28477](260) STAMP host_x64/obj/tools/configc/configc_sdk_meta_generated_file.stamp
  0.4s | STAMP obj/sdk/zircon_sysroot_meta_verify.stamp
  0.4s | CXX obj/BUILD_DIR/fidling/gen/sdk/fidl/fuchsia.me...chsia.media/cpp/fuchsia.media_cpp_common.common_types.cc.o
  0.4s | CXX obj/BUILD_DIR/fidling/gen/sdk/fidl/fuchsia.me...fuchsia.media/cpp/fuchsia.media_cpp.natural_messaging.cc.o
  0.4s | CXX obj/BUILD_DIR/fidling/gen/sdk/fidl/fuchsia.me...dia/cpp/fuchsia.media_cpp_natural_types.natural_types.cc.o
```

For more details, see
[Fuchsia feature: status of pending commands][fuchsia-feature-pending-commands].

## Ninja build dependency graph {#build-graph}

The graph that Ninja constructs from the build plan contains only two types
of nodes[^3]:

- **Target** nodes: A Target node simply corresponds to a file path known
  to Ninja. That path is always relative to the build directory.

- **Action** nodes: An Action node models a single command to run to
  generate output files, from a given set of input files.

Note the following information:

- A Target node that is not the output of any Action node is called a source
  file.

- A Target node that is not the input of any Action node must be the output
  of a given Action node, and is called a final output.

- A Target node that is both the output of an Action as well as the input
  of another one is called an intermediate target, or intermediate output.

- Each Action can point to zero or more input Target nodes in the graph.

- Each Action can have one or more output Target nodes in the graph.
  An action cannot have zero outputs, otherwise Ninja wouldn't know when
  to run its command.

- Action nodes have no names, so there is no way to reference them directly
  when invoking Ninja. Only file paths, that is, targets.

[^3]: For historical reasons, the Ninja source code uses a C++ class named `Node` to model targets, and a C++ class named `Edge` to model actions. However since this is frequently a source of _great_ _confusion_ when reading the code, this document will _not_ follow this misleading convention.

### Ninja build plan {#build-plan}

The Ninja build plan is defined by a `build.ninja` file at the top of the
build directory, which can include a other `*.ninja` file with `include` or
`subninja` statements. The following is a summary of its most important
features (for full details, see the [Ninja manual][ninja-manual]{:.external}).

Inside `.ninja` files, Action nodes are defined through a `build` statement:

```ninja
build <outputs>: <rule_name> <inputs>
```

`<outputs>` is a list of output paths, `<inputs>` is a list of input paths,
and `<rule_name>` is the name of a Ninja _rule_, which acts as a _recipe_
to craft the final command to run. Rules are defined by a special `rule`
statement:

```ninja
rule <rule_name>
   command = <command expression>
```

`<command expression>` can contain the special `$in` and `$out` keywords
that will be expanded into the list of inputs and outputs of the corresponding
build rule.

```ninja
rule copy_file
  command = cp -f $in $out

build output.txt: copy_file input.txt
```

The example above is a trivial build plan that tells Ninja that to build
`output.txt`, the command `cp -f input.txt output.txt` must be run.

### Implicit outputs

It is possible for a command to have additional outputs that must not appear
in the `$out` expansion. These can be separated from explicit outputs with
the `|` separator.

```ninja
rule copy_file
  command = cp -f $in $out && touch $out.stamp

build output.txt | output.txt.stamp: copy_file input.txt
```

The example above tells Ninja that the command to build `output.txt` will
copy `input.txt` into it and create an `output.txt.stamp` file too.

### Implicit inputs

Similarly, it is possible to tell Ninja that some inputs should not be
expanded from the `$in` expression, by using the `|` on the right side of
the build statement.

```ninja
rule cxx_compile
  command = c++ -c $in -o $out

build foo.o: cxx_compile foo.cc | foo.h
```

The example above tells Ninja that compiling `foo.cc` will use `foo.h` as an
input, even though this file does not appear in the compiler command explicitly.

### Order-only inputs

It is possible to tell Ninja that some file paths are runtime dependencies of
some outputs, and thus should be built "with" them. This uses the `||`
separator on the right side of the `build` statement, and must always appear
after any potential `|` separator, if there is one.

```ninja
rule cxx_binary
  command = c++ -o $out $in -ldl

rule cxx_shared_library
  command = c++ -shared -o $out $in

build foo.so: cxx_shared_library:

build program: cxx_binary main.cc || libfoo.so
```

The example above tells Ninja that whenever `program` needs to be built, then
`foo.so` will need to be built as well, but that order is not important. In other
words, it is ok to run the command that generates `program` before the one that
generates `foo.so`. In this example, this works if the binary only loads the library
through `dlopen()` at runtime.

## Reducing rebuilds with the restat optimization {#restat}

Some commands may not change their output file's timestamp if their content
did not change. Ninja can use this to reduce the total number of commands to
run during a build invocation.

To support this, a rule definition must set the special `restat` variable to
a non-empty value. This causes Ninja to re-stat the command's outputs after
executing it. Each output whose modification time didn't change will be treated
as though it had never needed to be built, and Ninja will remove any commands
that use it as input from the pending commands list.

```ninja {:.devsite-disable-click-to-copy}
# A rule to invoke the create_manifest.py script that processes some input
# and generates a manifest as output. `restat` is set to indicate that the
# script will not update $out's timestamp if the file exists and its content
# is already correct.

rule create_manifest
  command = ../../create_manifest.py --input $in --output $out
  restat = 1

build package_manifest.json: create_manifest package_list.txt

build package_archive.zip: create_archive package_manifest.json
```

In the example above, if the developer changes `package_list.txt` in a way that
does not change the `package_manifest.json` output file, then the final
`package_archive.zip` will not need to be re-generated. To support this, every
time Ninja runs a command, it records for each output file a summary that
includes a hash of the command and the timestamp of the most recent input, into
a special file at `$BUILD_DIR/.ninja_build`, called the _Ninja build log_.

On the next Ninja invocation, the build log timestamp is used instead of the
filesystem one (if it is newer), to determine whether the file needs to be
regenerated. Hence, in the example above, the newer timestamp for
`package_list.txt` would be associated with `package_manifest.json` even though
its filesystem timestamp would be older. Without this feature, Ninja would try
to rebuild the manifest file on every build invocation.

## Discovering implicit inputs at build time with depfiles

A command launched by Ninja can generate a special dependency file (abbreviated
as  `depfile`) that lists _extra_ _implicit_ _inputs_, that is, inputs to the
command that do not appear in the build plan. This information is read by Ninja,
which records it in the binary file named `$BUILD_DIR/.ninja_deps`, as known as
the "Ninja deps log". On the next Ninja invocation, the deps log is loaded
automatically and all recorded implicit inputs are added to the dependency graph.

For example, this is useful for C++ compilation commands to list all included
headers, even those that are not listed explicitly in the corresponding `.ninja`
file. If such a header is modified by the developer, the next Ninja invocation
will see the change and cause the corresponding C++ sources, and any of its
dependencies, to be recompiled.

This works by adding a `depfile` variable declaration to a rule definition as
in:

```ninja
rule cc
    depfile = $out.d
    command = gcc -MD -MF $out.d [other gcc flags here]
```

Note that by default, `depfile` are removed by Ninja after they have been
ingested into the binary deps log. To inspect which `depfile` dependencies were
recorded, take one of the following actions:

- Run `ninja -C <build_dir> -t deps <target>`, where `<build_dir> ` is the
  build directory, and `<target>` is the path to an output file, relative
  to `<build_dir>`. The `-t deps` option invokes a Ninja tool that prints
  the content of the deps log for this output file.

  Note however that the deps log is a binary append-only file, so will
  accumulate `depfile` dependencies over several Ninja build invocations, so
  this may list more implicit dependencies than those generated by the last
  command.

- Remove the build artifact, then invoke `ninja` with the `-d keepdepfile`
  option, which forces Ninja to leave all dependency files in the build
  directory (after copying their content to the binary deps log). This
  allows manual inspection of its content, for example:

  ```sh {:.devsite-disable-click-to-copy}
  $ rm $BUILD_DIR/foo.o
  $ ninja -C $BUILD_DIR -d keepdepfile foo.o
  $ cat $BUILD_DIR/foo.o.d
  ```

  Note that the exact `depfile` path depends on the rule definition. As a
  convention most command just append a `.d` suffix to the first output path,
  but this is not enforced by Ninja.

### Correctness issues with depfiles {#depfile-correctness-issues}

The deps log works extremely well when there are no changes to the build plan,
because Ninja will detect on the next incremental build which `depfile`-listed
implicit inputs where changed, and rebuild anything that depends on them.

However, when the build plan changed, entries in the Ninja deps log can
become _stale_, and will add _incorrect_ edges in the dependency graph of the
next Ninja invocation. And sometimes, these will *break* the next build
invocation. This happens in particular when dependencies are removed from the
build plan, but still recorded in the deps log.

In practice, this results in random incremental build failures that can happen
locally, or on our infra builders (both in CQ or CI). And there is unfortunately
now way to solve the problem at the moment, as the deps log is part of Ninja's
design, and it is impossible to detect "stale" entries (since the exact details
of the old build plans are long gone when they are used).

The usual workaround is to perform a clean build. Fuchsia even implements
["clean build fences"][clean-build-fences]{:.external} to work around the most
problematic cases.

<!-- Reference links -->

[ninja]: https://ninja-build.org
[ninja-manual]: https://ninja-build.org/manual.html
[ninja-environment-variables]: https://ninja-build.org/manual.html#_environment_variables
[ninja-console-pool]: https://ninja-build.org/manual.html#_the_literal_console_literal_pool
[GN]: https://gn.googlesource.com/gn/
[bazel-action-graph]: https://bazel.build/reference/glossary#action-graph
[clean-build-fences]: https://cs.opensource.google/fuchsia/fuchsia/+/main:build/force_clean/README.md
[fuchsia-feature-pending-commands]: /docs/development/build/ninja_fuchsia_improvements.md#feature-stastus-of-pending-commands
