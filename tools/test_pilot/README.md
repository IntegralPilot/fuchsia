# User guide for `test-pilot`

This document provides an explanation of `test-pilot`'s purpose and function as
well as instructions for using it.

## Purpose

`test-pilot` was created as a wrapper for Fuchsia tests to simplify the task of
packaging a test as a simple command that can be invoked by `botanist` or `fx
test`.

### Background

Historically, as the collection of Fuchsia tests expanded and diversified,
the ways that those tests were invoked fragmented. Basic host tests were often
invoked with a simple shell command. Simple target tests were invoked using
`ffx test`, which remotely controls `test_manager` on the target device.
`Lacewing` end-to-end tests required their own procedure as did bringup and
other tests. `botanist` and `fx test` largely absorbed this complexity by
determining the type of test to be invoked and using the correct procedure
to invoke that type of test.

This state of affairs was workable, but not ideal. `botanist` and `fx test`
contained duplicate logic per test type. In addition to incurring the cost of
maintaining two copies of multiple test procedures, this made it difficult to
obtain identical behavior in CI and local test runs.

At the same time, the transition from gn to `bazel` as the Fuchsia build system
motivated conformance with `bazel`'s testing model. Complete conformance could
be achieved for hermetic host tests. Non-hermetic tests (particularly
those involving a Fuchsia target device or emulator) could come close to
conforming using `bazel run` rather than `bazel test`. `bazel`'s hermetic
testing model assumes that a test is encapsulated in a single host executable
that receives various parameters through environment variables. See [the bazel test encyclopedia][bazel-test-encyclopedia].

This motivated an effort to encapsulate each test in such an executable and to
move the type-specific test invocation logic into this encapsulated test.
`test-pilot` was designed to help with this encapsulation.

### Function

An invocation of `test-pilot`, when successful, executes a test and zero or
more post-processing tools, all of which are host executables. It works by
assembling a *test configuration* that is used to guide test execution. The
test configuration is the canonical description of the test run (its invocation,
not its results) and is persisted as a JSON file in the output directory,
allowing the test run to be duplicated later.

The test configuration is a JSON document that contains the following:

- path of the test to be invoked and the arguments to be passed to that test,
- paths of any post-processing tools to be invoked and their arguments,
- test parameters and their values,
- options that control aspects of the test run,
- other information useful to describe or reproduce the test run.

The test configuration is assembled from information obtained from the
`test-pilot` command line, from its environment variables, and from
referenced JSON files. Processing starts with the command line, which
typically includes a JSON file providing a template for the specific test
type.

`test-pilot` was designed with the following principles in mind:

- It is not opinionated about what executables it invokes or what test
  parameters are included in the configuration.
- It *is* opinionated about the format of the test output (a directory). but
  not about what information is expressed there. Post-processing accommodates
  alternative output formats.
- It attempts to offload responsibilities from the test and post-processor
  executables to constrain them as little as possible and make them easy to
  write.
- It captures all information required to diagnose and reproduce failures.
- It preserves the provenence of output data as much as possible. There should
  be no question as which binary supplied what information in the aggregate
  output.

### Role

In normal operation, `test-pilot` is invoked by a shell script that is specific
to the test. Here's a typical example of such a script:

```posix-terminal
#!/bin/bash

${FUCHSIA_HOST_TOOLS}/test-pilot --include=test_configs/foo.test_config.json $@
```

`foo.test_config.json` is a JSON file created by the build specifically for
the test `foo`. It may or may not reference a template JSON file that provides
properties common to tests similar to `foo`.

In the case of a simple target test, the generated configuration specifies that
that an `ffx test run` command should be executed. `ffx` has command line
options that allow it to consume the test configuration directly, simplifying
the command line. In addition to running ffx, the configuration may specify
that post-processors be run, possibly including a converter that produces a
summary in the desired format.

## Core Concepts

*   **Test parameter**: A named value in a test configuration, typically represented as a name/value pair in a JSON object.
*   **Test configuration**: A collection of test parameters that serve as input to a test run, canonically represented as a JSON object.
*   **Test configuration schema**: A JSON schema file in `fuchsia.dev` that defines the names, types, and descriptions of allowed test parameters.
*   **Option**: A `test-pilot` specific setting that is not a test parameter.

`test-pilot` is mostly agnostic to test parameters, acting as a pass-through. Parameter validation is driven by the test configuration schema and `require`/`prohibit` options.

## Command Line and JSON Files

Options and test parameters can be specified on the command line and in JSON files, though in slightly different ways.

### Command Line
The following rules apply to options and test parameters specified on the `test-pilot` command line.

*   Positional parameters are not used.
*   All parameters and options start with `--`.
*   Parameter/option names must be lowercase.
*   Kebab case is preferred for parameter/option names, but underscores may be substituted for hyphens. All parameter names are converted to lower snake case to identify the parameter.
*   Boolean parameters can use `--foo=true`/`--foo=false` or `--foo`/`--no-foo`.
*   Array/list values are comma-separated with no surrounding brackets or parentheses (e.g., `--foo=bar,baz`).
*   Object-typed values are not supported on the command line.
*   Assigning a value to a parameter not in the schema is an error.

### JSON Files
The following rules apply to options and test parameters specified in included JSON files.

*   At top level, the file must contain an object. The properties of that object are the test parameters.
*   Parameter/option names are in lower snake case.
*   Object-typed values are supported.
*   Assigning a value to a parameter not in the schema is an error.
*   If a property is assigned the object-typed value `{ "from_env": "<name>" }`, the value of the environment variable `<name>` is used instead. This fails if the variable is not defined in the environment.
*   If a property is assigned the object-typed value `{ "try_from_env": "<name>" }`, the value of the environment variable `<name>` is used instead. If the variable is not defined in the environment, no assignment occurs.
*   The option `debug` is not allowed in JSON files.

### Reserved Names

Option names (`debug`, `strict`, `include`, `require`, `prohibit`) are reserved and cannot be used as parameter names. The name `test_config_file` is reserved as a pseudo-parameter that `test-pilot` sets to the path of the JSON file to which the test configuration has been written.

### Multiple Assignments

Multiple assignments to the same parameter name are handled as follows:

*   **Array type parameters**: Assignments are additive.
*   **Non-array type parameters**:
    *   **Non-strict (initial) mode**: The last assignment wins.
    *   **Strict mode**: A parameter may only be assigned once. If already assigned in non-strict mode, a subsequent assignment in strict mode has no effect.

## Options and Test Parameters

### `test-pilot` Options

These options are *not* test parameters but control `test-pilot`'s behavior. Except where noted, these options can appear both in the command line and in
JSON configuration files.

*   `strict=true`: Indicates that subsequent test parameter assignments should be strict. In strict mode, a parameter that doesn't have an array type can only be assigned once.
*   `include=<list of paths>`: Reads specified JSON files for test parameters. `test-pilot` processes all included JSON files in the order they are encountered.
*   `require=<list of parameter names>`: Fails the run if the listed parameters are not present in the test configuration.
*   `prohibit=<list of parameter names>`: Fails the run if the listed parameters _are_ present in the test configuration.
*   `--debug`: (Command line only) Prints a log of parameter processing for debugging purposes.

### Test Parameters understood by `test-pilot`

These parameters are used by `test-pilot` to run the test and post-processors and are defined in the test configuration schema:

*   `host_test_binary`: (Required) Path of the host test binary to execute.
*   `host_test_args`: (Optional) Command-line arguments for the test binary. Parameter names in curly braces (e.g., `{test_config_file}`) are replaced with their values. Default args are `["{test_config_file}", "{output_directory}"]`.
*   `output_directory`: (Required) Specifies the directory where test run results will be deposited.
*   `output_processors`: Specifies output processors to run on the test output. The type of this parameter is an array of objects, each of which have the
following properties:
    * `binary`: (Required) Path of the post-processor to execute.
    * `args` (Optional) Command-line arguments for the post-processor. Default args are
    `["{test_config_file}", "{output_directory}"].
    * `use_on_success`: (Optional) Indicates the post-processor should be run when the test
    succeeds.
    * `use_on_failure`: (Optional) Indicates the post-processor should be run when the test
    fails.

## Output Directory Structure

A `test-pilot` run creates an output directory containing the test configuration
and all artifacts. The directory structure is as follows:

```

\<root\>\
output\_summary.json         (`test-pilot` merged summary)
test\_config.json            (`test-pilot` compiled config)
invocation\_log.json         (`test-pilot` invocation log)
test/
stdout.txt              (test stdout)
stderr.txt              (test stderr)
output\_summary.json     (ffx test output summary)
...                     (ffx test artifacts)
\<post-processor name\>/
output\_summary.json     (post-processor output summary)
...                     (post-processor artifacts)

```

*   `<root>/` is the specified `output_directory`. If the output directory is moved or copied to another location, the test configuration is not modified. The output directory in the configuration refers to the location at which the directory was initially created on the host that ran `test-pilot`.
*   `output_summary.json` is a merged summary of all `output_summary.json` files in the subdirectories. `test-pilot` produces this merge after the test run and all post processing is complete.
*   `test_config.json` is the compiled test configuration.
*   `test/` holds the output of the invoked test. The test may not supplement or modify any part of `<root>/` outside this subdirectory.
*   `<post-processor name>/` holds the output of a specific post-processor.
*   `stdout.txt` and `stderr.txt` files are only present if there was output to the respective streams.
*   All `output_summary.json` files are guaranteed to be present and to contain entries for all artifacts in their respective directories. These files are similar to the current `run_summary.json` files. In addition to artifact descriptions, these files contain information about the outcome of the run/processing in question. At a minimum, `test-pilot` will provide outcome information based on the exit status of the (host-side) binary or information about a failure to complete the execution of the host-side binary. All paths in these files are relative to `<root>`. `test-pilot` performs clean-up to meet these constraints, as appropriate, relieving some parties of the burden of satisfying them.

## Host-Side Test Requirements

A host-side test is the test binary invoked by `test-pilot`. This may be the test itself or, in
the case of target tests, ffx test.

A minimal host-side test must:

*   Be a runnable host binary.
*   Exit with a code indicating success (0) or failure (anything else).

Such a test can produce output through stdout and/or stderr, which will be captured by `test-pilot` and included in `<root>/test/`.

Optional capabilities and corresponding requirements:

*   **Consuming Test Parameters**:
    *   To receive specific parameters on the command line: `host_test_args` must be specified to include them, and the test must parse its command line.
    *   To access the entire test configuration as a JSON file: `host_test_args` must include `{test_config_file}` (or use the default args), and the test must read the file.
*   **Adding Files to the Output**: `host_test_args` must include `{output_directory}/test` (or use the default args), and the test must write files to this path.
*   **Producing a `summary.json` File**: The test can produce `<root>/test/output_summary.json`

<!-- Reference links -->

[bazel-test-encyclopedia]: https://bazel.build/reference/test-encyclopedia
