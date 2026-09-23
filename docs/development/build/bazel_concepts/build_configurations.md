# Bazel build configurations

A _build configuration_ is a set of settings, like `cpu = "arm64"`, where
each setting assigns a _configuration value_ to a known
_configuration variable_ that Bazel knows about. These values affect how Bazel
builds things.

Build configurations can be seen as _immutable dictionaries_ mapping variable
names (string keys) to configuration values (booleans, integers, strings, list
of strings, label or list of labels).

They are only meaningful during the Bazel analysis phase, and are identified
internally by a _unique hash_ of their content. _They do not have a label_, and
cannot appear directly in Bazel build files or Starlark code.

Each build configuration instance has its own output directory under the Bazel
`output_base`, i.e.:

```
${output_base}/execroot/<workspace_name>/bazel-out/<config_dir>/bin/
```

The build artifacts of targets evaluated in the context of the build
configuration are stored there. The `<config_dir>` value depends on the content
of the build configuration. Its computation is a Bazel implementation detail,
that can actually vary between Bazel releases.

In the simplest cases, `<config_dir>` would look like
`${cpu}-${compilation_mode}`, for example `k8-fastbuild`[^1] or `aarch64-dbg`,
but can become much more complex very easily.

[^1]: [`k8`][amd-k8] is an old naming convention for what is now called the `x86_64` CPU architecture.

Some example `config_dir` values for the build configurations of a small
but non-trivial [example Bazel project][bazel-example-project]{:.external}:

- `k8-fastbuild`
- `k8-fastbuild-ST-6f89e1bee3ea`
- `k8-fastbuild-ST-bd2abcc18995`
- `k8-fastbuild-ST-ec22d3eeb595`
- `k8-opt`
- `k8-opt-exec-2B5CBBC6`
- `k8-opt-exec-2B5CBBC6-ST-5934bb4653e4`

Note that the hexadecimal values above are _not_ related to the configuration's
ID (i.e. its unique hash).

Note: Consider the `<config_dir>` value to be **generally unpredictable**, or
      you will have a bad time!

# Common build configurations

By default, Bazel manages two configurations:

- The _default configuration_, also known ambiguously as the
  _target configuration_, because it corresponds to build artifacts that must
  run on the final "target" system.

  Its settings match the host system by default, but can be changed with
  command-line options (see below).

- The _host configuration_, which corresponds to code that must run on the
  machine where Bazel runs. This contains the same settings as the default
  configuration, except when cross-compiling (e.g. with `--cpu=<name>`).

There is also:

- The _exec configuration_, which corresponds to code that runs during build
  actions. In practice, this contains the same settings as the host one,
  except when using remote builders.

A Bazel project can also define _extra configurations_ to accommodate certain
use cases, for example to cross-compile for more than one target architecture
in a single build invocation.

# Native configuration variables

The `bazel build` command supports a large number of command-line options that
change _native configuration variables_ (a.k.a. _native_ _settings_), for
example:

- `--cpu=<name>` and `--host_cpu=<name>`:

  Change the CPU architecture of the code generated for the default or host
  configuration, respectively. For example:

  ```posix-terminal
  bazel build --cpu=arm64 --host_cpu=x86_64 ...
  ```

- `-c <mode>`, `--compilation_mode=<mode>` or `--host_compilation_mode=<mode>`:

  Where `<mode>` is one of "`fastbuild`", "`dbg`" or "`opt`", which describe a
  set of compiler linker flags. For example:

  ```posix-terminal
  bazel build -c opt --host_compilation_mode=dbg ...
  ```

- `--crosstool=<file_path>` or `--host_crosstool=<file_path>`:

  Point to a `CROSSTOOL` file used to describe a custom C++ toolchain to Bazel,
  for compiling and linking either target or host binaries.

Note: The `--host_xxx` options affect both the `host` and `exec` configurations.

There is a _very large set_ of native configuration variables (and related
command-line flags). For example Bazel 5.2.0 supports more than 340 ones
(including 83 experimental ones).

# Custom configuration flags (deprecated)

It is possible to define custom configuration variables (confusingly named
_custom configuration flags_ in the Bazel documentation), that will be set on
the command-line with "`--define <name>=<value>`", where `<name>` is an
arbitrary build configuration variable name, and `<value>` is recorded as an
arbitrary string (the value is never interpreted).

These names can be tested in `BUILD.bazel` files using special constructs,
for example:

```py
# This represents a named configuration condition which will be True whenever
# `--define foo=bar` is used in the `bazel build` command-line.
config_setting(
  name = "is_foo_bar",
  define_flags = {
    "foo": "bar",
  }
)
```

Note: Using "`--define <name>=<value>`" on the command line affects all build
configurations.

Warning: These custom flags are now **DEPRECATED** because `--define` names
share a global namespace, which can lead to conflicts when several workspaces
use the same name value. Other issues include hard-to-detect typographic errors,
and that the value is always recorded and compared as a simple string.

# User-defined build settings

To overcome the limitations of custom `--define` flags, Bazel projects can
now define build configuration variables that:

- Are named with a _label_ (which provides workspace and package scoping,
  plus availability checks).

- Can have a specific type, one of: boolean, integer, string, string list,
  label and label list.

- Always provide a default value.

These can be set on the command line simply with "`--<label>=<value>`" as in:

```posix-terminal
# Set the build setting named enable_logs defined in
# $WORKSPACE/my_settings/BUILD.bazel to True.
#
# Note that the double-slash must follow the double caret directly
# without spaces between them.

bazel build --//my_settings:enable_logs=True …
```

Which matches a definition that must appear in the file
`$PROJECT/my_settings/BUILD.bazel`, e.g.:

```py
# From my_settings/BUILD.bazel
load("@bazel_skylib//rules/common_settings.bzl", "bool_flag")

bool_flag(
  name = "enable_logs",
  default_value = False
)
```

The build setting can also be defined in an external repository, as in:

```posix-terminal
# Set the build setting named enable_logs defined in the config/BUILD.bazel
# file of the @project_settings external repository.

bazel build --@project_settings//config:enable_logs=True …
```

# Storing user configuration values

Instead of passing them directly on the command-line, arguments that set
configuration values can be stored in the `.bazelrc` file, located at the
top of the project's workspace directory, as in:

```py
# from $PROJECT/.bazelrc
build --host_copt=-O3 -s
build --copt=-O1 -g -Wall
build --//my_settings:enable_logs=True
```

When calling `bazel build <target>`, the options provided by the `.bazelrc`
are automatically included, resulting in a command that would be equivalent
to:

```sh
bazel build --host_copy="-O3 -s" --copt="-O1 -g -Wall" --//my_settings:enable_logs=True <target>
```

It is also possible to group several definitions with a custom _name_ as in:

```py
# from $PROJECT/.bazelrc
build:conf_x64 --cpu=x86_64
build:conf_x64 --copt=-mavx2

build:conf_arm64 --cpu=aarch64
build:conf_arm64 --copt=-marmv8.1-a+simd
```

Then use `--config=<name>` to select all options from one group, as in:

```sh
bazel build --config=conf_x64 <target>
```

Which will be equivalent to:

```sh
bazel build --cpu=x86_64 --copt=-mavx2 <target>
```

It is possible to combine `--config` with other options as well, as in:

```sh
bazel build --config=conf_arm64 --copy="-DENABLE_ASSERTS=1" ...
```

===
IMPORTANT: Despite its name, the `--config` option does not create a new
build configuration. This is just a convenient way to group command-line
options for the user. Each `<name>` is simply recursively expanded to its
canonical constituents, then completely ignored by Bazel.
===

# Configuration transitions (quick overview)

Besides the standard target, host and exec configurations, Bazel supports the
creation of extra build configurations, using the notion of _transitions_.

Transitions are an advanced feature that won't be detailed here, but it is
important to understand that:

- Build configurations are conceptually just dictionaries which map
  configuration variable names to configuration values. E.g.:

  ```py
  {
     "copt": "-O2 -g",
     "cpu": "x86_64",
     "compiler": "gcc",
     "//my_settings:enable_logs": True,
     …
  }
  ```

- A transition provides a function which takes the _current build configuration_
  as input, and returns a _new build configuration_ as output.

- A transition function can only _modify the values_ in the dictionary, i.e.
  it cannot add or remove keys from the input.

- Transition functions are called during the analysis phase (explained later),
  under specific conditions, and cannot be triggered directly by the user
  on the command-line or the `.bazelrc` file.

# Inspecting build configurations

After a Bazel build command, it is possible to look at all the configurations
that were needed. Use the `bazel config` command, to print a list of all
configurations, and their `config_dir` value, e.g.:

```sh
$ bazel clean
$ bazel build //src:program
$ bazel config
Available configurations:
20222e2616387f00ae3814d79f82a650829d1aea962d558edda5cf54c459f4a2 k8-fastbuild
30f5a27e1bd054836d97b800bdfe61c6d1cffccdf6b82c9963e94020e3852026 k8-fastbuild-ST-bd2abcc18995
34e198ac81dafe34b82cf62466cc84f5b1af6ef8dce44b9283b3152a6635f64a k8-fastbuild-ST-6f89e1bee3ea
aaa26904110821b8e0f87aabfffcfb8e5003a5620aee74de3d7ddea9a654c0f6 k8-opt (host)
d6b1a93e6619d845f29ee752e338ec2636e7fbf4c88e50168602cc21f5ae6f13 k8-opt-exec-2B5CBBC6-ST-5934bb4653e4 (exec)
```

NOTE: configuration information accumulates until the next bazel clean.

Use `bazel config <id>` to dump the configuration to stdout to inspect its
values. WARNING: The output is very long.

```sh
$ bazel config 20222e2616387f00ae3814d79f82a650829d1aea962d558edda5cf54c459f4a2 | wc -l
Loading:
INFO: Displaying config with id 20222e2616387f00ae3814d79f82a650829d1aea962d558edda5cf54c459f4a2
436
```

Link to the real output here.

Note that in practice, each configuration is _a set of dictionaries_, each one
covering a specific topic. These are called _fragments_, and
[only a few of them][bazel-fragments] can be accessed directly from Starlark:

```sh
$ bazel config 20222e2616387f00ae3814d79f82a650829d1aea962d558edda5cf54c459f4a2 | grep FragmentOptions
FragmentOptions com.google.devtools.build.lib.analysis.PlatformOptions {
FragmentOptions com.google.devtools.build.lib.analysis.ShellConfiguration$Options {
FragmentOptions com.google.devtools.build.lib.analysis.config.CoreOptions {
FragmentOptions com.google.devtools.build.lib.analysis.test.CoverageConfiguration$CoverageOptions {
FragmentOptions com.google.devtools.build.lib.analysis.test.TestConfiguration$TestOptions {
FragmentOptions com.google.devtools.build.lib.bazel.rules.BazelRuleClassProvider$StrictActionEnvOptions {
FragmentOptions com.google.devtools.build.lib.bazel.rules.python.BazelPythonConfiguration$Options {
FragmentOptions com.google.devtools.build.lib.rules.android.AndroidConfiguration$Options {
```

[bazel-example-project]: https://github.com/digit-google/bazel-experiments/tree/main/pic-vs-pie/transitions-5
[bazel-fragments]: https://bazel.build/rules/lib/starlark-configuration-fragment
[amd-k8]: https://en.wikipedia.org/wiki/AMD_K8
