# The Fuchsia Test Runner Framework

The Fuchsia [Component Framework][cf] allows developers to create components in
a variety of languages and runtimes. Fuchsia's own code uses a diverse mix of
programming languages for components, including C/C++, Rust, Dart, and Go.

The Test Runner Framework uses Component Framework [runners][runners] as an
integration layer between various testing runtimes and a common Fuchsia protocol
for launching tests and receiving their results. This makes for an inclusive
design that on one hand allows developers to bring their language and testing
framework of choice, and on the other hand allows building and testing Fuchsia
on a variety of systems and targeting different hardware.

## The Test Manager

The `test_manager` component is responsible for running tests on a Fuchsia
device. Test manager exposes the
[`fuchsia.test.manager.RunBuilder`][fidl-test-manager] protocol, which allows
launching test suites.

Each test suite is launched as a child of test manager. Test suites are offered
capabilities by test manager that enable them to do their work while
maintaining isolation between the test and the rest of the system. For instance
hermetic tests are given the capability to log messages, but are not given the
capability to interact with real system resources outside of their sandbox.
Test manager uses only one capability from the test realm, a controller protocol
that test suites expose. This is done to ensure hermeticity (test results aren't
affected by anything outside of their intended sandbox) and isolation (tests
don't affect each other or the rest of the system).

The test manager controller itself is offered to other components in the system
in order to integrate test execution with various developer tools. Tests can
then be launched with such tools as [`fx test`][fx-test] and [`ffx`][ffx].

## The test suite protocol {#test-suite-protocol}

The test suite protocol, [`fuchsia.test.Suite`][fidl-test-suite], is used by the
test manager to control tests, such as to invoke test cases and to collect their
results.

Test authors typically don't need to implement this protocol. Instead, they rely
on a [test runner](#test-runners) to do this for them. For instance, you might
write a test in C++ using the GoogleTest framework, and then use
[`gtest_runner`](#gtest-runner) in your [component manifest][component-manifest]
to integrate with the Test Runner Framework.

## Test runners {#test-runners}

### A language and runtime-inclusive framework

Test runners are reusable adapters between the Test Runner Framework and common
languages & frameworks used by developers to write tests. They implement the
[`fuchsia.test.Suite`][fidl-test-suite] protocol on behalf of the test author,
allowing developers to write idiomatic tests for their language and framework of
choice.

Component manifests for simple unit tests can be [generated][unit-tests]
by the build rules. Generated component manifests for v2 tests will include the
appropriate test runner based on their build definition. For instance a test
executable that depends on the GoogleTest library will include the
[GoogleTest runner](#gtest-runner) in its generated manifest.

### Inventory of test runners

The following test runners are currently available for general use:

#### GoogleTest runner {#gtest-runner}

A runner for tests written in C/C++ using the GoogleTest framework.
Use this for all tests written using GoogleTest.

Common GoogleTest features are supported, such as disabling tests, running
only specified tests, running the same test multiple times, etc'.
Standard output, standard error, and logs are captured from the test.

In order to use this runner, add the following to your component manifest:

```json5
{
    include: [ "//src/sys/test_runners/gtest/default.shard.cml" ]
}
```

By default GoogleTest test cases run serially (one test case at a time).

#### GoogleTest (Gunit) runner {#gunit-runner}

A runner for tests written in C/C++ using the GUnit framework.
Use this for all tests written using the gUnit flavor of GoogleTest.

Note: Gtest and Gunit testing framework differ in flag names, so we have a
separate runner for gunit.

Common GoogleTest features are supported, such as disabling tests, running
only specified tests, running the same test multiple times, etc'.
Standard output, standard error, and logs are captured from the test.

In order to use this runner, add the following to your component manifest:

```json5
{
    include: [ "sys/testing/gunit_runner.shard.cml" ]
}
```

By default test cases run serially (one test case at a time).

#### Rust runner {#rust-runner}

A runner for tests written in the Rust programming language and following Rust
testing idioms.
Use this for all idiomatic Rust tests (i.e. tests with modules that set the
attribute `[cfg(test)]`).

Common Rust testing features are supported, such as disabling tests, running
only specified tests, running the same test multiple times, etc'.
Standard output, standard error, and logs are captured from the test.

In order to use this runner, add the following to your component manifest:

```json5
{
    include: [ "//src/sys/test_runners/rust/default.shard.cml" ]
}
```

By default Rust test cases run in parallel, at most 10 cases at a time.

#### Go test runner {#gotest-runner}

A runner for tests written in the Go programming language and following Go
testing idioms.
Use this for all tests written in Go using `import "testing"`.

Common Go testing features are supported, such as disabling tests, running
only specified tests, running the same test multiple times, etc'.
Standard output, standard error, and logs are captured from the test.

In order to use this runner, add the following to your component manifest:

```json5
{
    include: [ "//src/sys/test_runners/gotests/default.shard.cml" ]
}
```

By default Go test cases run in parallel, at most 10 cases at a time.

#### ELF test runner {#elf-test-runner}

The simplest test runner - it waits for your program to terminate, then reports
that the test passed if the program returned zero or that it failed for any
non-zero return value.

Use this test runner if your test is implemented as an ELF program (for instance
an executable written in C/C++) but it does not use a common testing framework
that's supported by existing runners and you'd rather not implement a bespoke
test runner.

In order to use this runner, add the following to your component manifest:

```json5
{
    include: [ "sys/testing/elf_test_runner.shard.cml" ]
}
```

If you are [using in-tree unit test GN templates][component-unit-tests],
and you are not already using a test framework with a dedicated test runner,
add the following to your build deps:

```
fuchsia_unittest_package("my-test-packkage") {
    // ...
    deps = [
        // ...
        "//src/sys/testing/elftest",
    ]
}
```

Note: If you see the error message "Component has a \`program\` block defined,
but doesn't specify a \`runner\`" for your test, this indicates you are not using a
test framework with a dedicated test runner, and you should add the above dependency.

### Controlling parallel execution of test cases

When using `fx test` to launch tests, they may run each test case in sequence or
run multiple test cases in parallel up to a given limit. The default
parallelism behavior is determined by the test runner. To manually control the
number of test cases to run in parallel use test spec:

```gn
fuchsia_test_package("my-test-pkg") {
  test_components = [ ":my_test_component" ]
  test_specs = {
    # control the parallelism
    parallel = 10
  }
}
```

### Running test multiple times

To run a test multiple times use:

```posix-terminal
 fx test --count=<n> <test_url>
```

If an iteration times out, no further iteration will be executed.

### Passing arguments

Custom arguments to the tests can be passed using `fx test`:

```posix-terminal
fx test <test_url> -- <custom_args>
```

Individual test runners have restrictions on these custom flags:

#### GoogleTest runner {#gtest-runner-custom-arg}

Note the following known behavior changes:

**--gtest_break_on_failure** - Instead use:

```posix-terminal
fx test --break-on-failure <test_url>
```

The following flags are restricted and the test fails if any are passed as
fuchsia.test.Suite provides equivalent functionality that replaces them.

- **--gtest_filter** - Instead use:

```posix-terminal
 fx test --test-filter=<glob_pattern> <test_url>
```

`--test-filter` may be specified multiple times. Tests that match any of the
given glob patterns will be executed.

- **--gtest_also_run_disabled_tests** - Instead use:

```posix-terminal
 fx test --also-run-disabled-tests <test_url>
```

- **--gtest_repeat** - See [Running test multiple times](#running_test_multiple_times).
- **--gtest_output** - Emitting gtest json output is not supported.
- **--gtest_list_tests** - Listing test cases is not supported.

#### GoogleTest (Gunit) runner {#gunit-runner-custom-arg}

Note the following known behavior changes:

**--gunit_break_on_failure** - Instead use:

```posix-terminal
fx test --break-on-failure <test_url>
```

The following flags are restricted and the test fails if any are passed as
fuchsia.test.Suite provides equivalent functionality that replaces them.

- **--gunit_filter** - Instead use:

```posix-terminal
 fx test --test-filter=<glob_pattern> <test_url>
```

`--test-filter` may be specified multiple times. Tests that match any of the
given glob patterns will be executed.

- **--gunit_also_run_disabled_tests** - Instead use:

```posix-terminal
 fx test --also-run-disabled-tests <test_url>
```

- **--gunit_repeat** - See [Running test multiple times](#running_test_multiple_times).
- **--gunit_output** - Emitting gtest json/xml output is not supported.
- **--gunit_list_tests** - Listing test cases is not supported.

#### Rust runner {#rust-runner-custom-arg}

The following flags are restricted and the test fails if any are passed as
fuchsia.test.Suite provides equivalent functionality that replaces them.

- **\<test_name_matcher\>** - Instead use:

```posix-terminal
 fx test --test-filter=<glob_pattern> <test_url>
```

`--test-filter` may be specified multiple times. Tests that match any of the
given glob patterns will be executed.

- **--nocapture** - Output is printed by default.
- **--list** - Listing test cases is not supported.

#### Go test runner {#gotest-runner-custom-arg}

Note the following known behavior change:

**-test.failfast**: As each test case is executed in a different process, this
flag will only influence sub-tests.

The following flags are restricted and the test fails if any are passed as
fuchsia.test.Suite provides equivalent functionality that replaces them

- **-test.run** - Instead use:

```posix-terminal
 fx test --test-filter=<glob_pattern> <test_url>
```

`--test-filter` may be specified multiple times. Tests that match any of the
given glob patterns will be executed.

- **-test.count** - See [Running test multiple times](#running_test_multiple_times).
- **-test.v** - Output is printed by default.
- **-test.parallel** - See [Controlling parallel execution of test cases](#controlling_parallel_execution_of_test_cases).

### A runtime-agnostic, runtime-inclusive testing framework {#inclusive}

Fuchsia aims to be inclusive, for instance in the sense that
developers can create components (and their tests) in their language and runtime
of choice. The Test Runner Framework itself is language-agnostic by design, with
individual test runners specializing in particular programming languages or test
runtimes and therefore being language-inclusive. Anyone can create and use new
test runners.

Creating new test runners is relatively easy, with the possibility of sharing
code between different runners. For instance, the GoogleTest runner and the Rust
runner share code related to launching an ELF binary, but differ in code for
passing command line arguments to the test and parsing the test's results.

## Temporary storage

To use temporary storage in your test, add the following to your component manifest:

```json5
{
    include: [ "//src/sys/test_runners/tmp_storage.shard.cml" ]
}
```

At runtime, your test will have read/write access to `/tmp`.
The contents of this directory will be empty when the test starts, and will be
deleted after the test finishes.

[Tests that don't specify a custom manifest][component-unit-tests] and instead
rely on the build system to generate their component manifest can add the
following dependency:

```gn
fuchsia_unittest_package("foo-tests") {
  deps = [
    ":foo_test",
    "//src/sys/test_runners:tmp_storage",
  ]
}
```

## Exporting custom files {#custom-artifacts}

To export custom files from your test, use the *custom_artifacts* storage
capability. The contents of *custom_artifacts* are copied out at the conclusion
of a test.

To use *custom_artifacts* in your test, add the following to your component
manifest:

```json5
{
    use: [
        {
            storage: "custom_artifacts",
            rights: [ "rw*" ],
            path: "/custom_artifacts",
        },
    ],
}
```

At runtime, your test will have read/write access to `/custom_artifacts`.
The contents of this directory will be empty when the test starts, and will be
deleted after the test finishes.

See the [custom artifact test example][custom-artifact-example]. To run it, add
`//examples/tests/rust:tests` to your build, then run:

```posix-terminal
fx test --ffx-output-directory <output-dir> custom_artifact_user
```

After the test concludes, `<output-dir>` will contain the `artifact.txt` file
produced by the test.

## Hermeticity

In the context of software testing, Hermeticity refers to the isolation of a
test or test suite from external factors and dependencies, ensuring that it
produces consistent and reliable results regardless of changes in the
surrounding environment. A hermetic test is self-contained and doesn't rely on
external systems or data that might change unexpectedly, leading to flaky or
non-deterministic test outcomes.

*Hermeticity does not mean protection from in-stable platform or routed
capabilities/APIs. If an API surface is in-stable for certain components in the
system, then they will be instable for tests and dependent components and will
help catch regressions due to any direct or in direct changes to system's API
surface.*

Ability to write fully, provably hermetic tests is Fuchsia's **Testing
Superpower**. There are two type of test hermeticity:

- Capability: The Test does not [use][manifests-use] or [offer][manifests-offer]
  any capabilities from the [test root's](#test-roles) parent. These tests don't
  have access to any system capabilities which can affect larger system. Due to
  this property of hermetic tests they can be run in parallel and will not flake
  due to cross-talk or shared state, it improves stability and performance of
  tests.

- Package: The Test does not [resolve][resolvers] any components outside of the
  test package. A hermetically packaged test does not have any implicit contract
  with platform packages. This provides a way to update them without affecting
  system packages and avoids dependence on incompatible packaged dependencies.

Hermeticity does not apply to platform APIs/capabilities which are available to
all components in the system. For eg

- clock
- Kernel provided identifiers, such as koids.
- Framework capabilities provided to all components which allow clients to
  mutate component manager state, and while this is not strictly hermetic it is
  component manager's responsibility to ensure isolation

The tests should be careful when using these APIs/capabilities.

The tests are by default hermetic unless explicitly stated otherwise.

### Hermetic capabilities for tests

There are some capabilities which all tests can use which do not violate test
hermeticity:

| Protocol | Description |
| -----------| ------------|
| `fuchsia.boot.WriteOnlyLog` | Write to kernel log |
| `fuchsia.logger.LogSink` | Write to syslog |
| `fuchsia.process.Launcher` | Launch a child process from the test package |
| `fuchsia.diagnostics.ArchiveAccessor` | Read diagnostics output by components in the test |

The hermeticity is retained because these capabilities are carefully curated
to not allow tests to affect the behavior of system components outside the test
realm or of other tests.

To use these capabilities, there should be a use declaration added to test's
manifest file:

```json5
// my_test.cml
{
    use: [
        ...
        {
            protocol: [
              "{{ '<var label="protocol">fuchsia.logger.LogSink</var>' }}"
            ],
        },
    ],
}
```

Tests are also provided with some default storage capabilities which are
destroyed after the test finishes execution.

| Storage Capability | Description | Path |
| ------------------ | ----------- | ---- |
|  `data` | Isolated data storage directory | `/data` |
|  `cache` | Isolated cache storage directory | `/cache` |
|  `tmp` | Isolated in-memory [temporary storage directory](#temporary_storage) | `/tmp` |

Add a use declaration in test's manifest file to use these capabilities.

```json5
// my_test.cml
{
    use: [
        ...
        {
            storage: "{{ '<var label="storage">data</var>' }}",
            path: "{{ '<var label="storage path">/data</var>' }}",
        },
    ],
}
```

The framework also provides some [capabilities][framework-capabilities] to all
the components and can be used by test components if required.

### Hermetic component resolution {#hermetic-resolver}

Hermetic test components are launched in a realm that utilizes the hermetic
component resolver. This resolver disallows resolving URLs outside of the
test's package. This is necessary for enforcing hermeticity, as we don't
want the availability of an arbitrary component on the system or in an
associated package server to affect the outcome of a test.

Attempts to resolve a component not in the test's package will be met with a
`PackageNotFound` error and the following message in the syslog:

```
failed to resolve component fuchsia-pkg://fuchsia.com/[package_name]#meta/[component_name]: package [package_name] is not in the set of allowed packages...
```

You can avoid this error by including any components your test relies on
to the test package - see [this CL](https://fxrev.dev/608222) for an example of
how to do this, or by using [subpackages]:

```gn
# BUILD.gn
import("//build/components.gni")


fuchsia_test_package("simple_test") {
  test_components = [ ":simple_test_component" ]
  subpackages = [ "//path/to/subpackage:subpackage" ]
}
```

```json5
 // test.cml
 {
...
    children: [
        {
            name: "child",
            url: "subpackage#meta/subpackaged_component.cm",
        },
    ],
...
}
```

See [this CL](https://fxrev.dev/784304) as an example of using subpackages.

Note: Subpackages are not available in the sdk yet. If you are not able to
include a dependent component in your test package you can add below option
to your test manifest file to selectively allow resolution of some packages:

```json5
// my_component_test.cml

{
...

    facets: {
        "fuchsia.test": {
            "deprecated-allowed-packages": [ "non_hermetic_package" ],
        },
    },
...
}

```

### Non-hermetic tests

These tests can access some pre-defined capabilities outside of the test realm.
A capability accessed by non-hermetic test from outside its test realm is called
a *system capability*.

To use a system capability, a test must explicitly mark itself to run in
non-hermetic realm as shown below.

```gn
# BUILD.gn (in-tree build rule)

fuchsia_test_component("my_test_component") {
  component_name = "my_test"
  manifest = "meta/my_test.cml"
  deps = [ ":my_test_bin" ]

  # This runs the test in "system-tests" non-hermetic realm.
  test_type = "system"
}
```

After integrating with the build rule, the test can be executed as

```
fx test <my_test>
```

Or for out-of-tree developers

```
ffx test run --realm <realm_moniker> <test_url>
```

where `realm_moniker` should be replaced with `/core/testing/system-tests` for
above example.

Possible values of `test_type`:

| Value | Description |
| ----- | ----------- |
| `chromium` | [Chromium test realm] |
| `ctf` | [CTF test realm] |
| `device` | [Device tests] |
| `drm` | [DRM tests] |
| `starnix` | [Starnix tests] |
| `system_validation` | [System Validation Tests] |
| `system` | [Legacy non hermetic realm][system-test-realm] with access to some system capabilities. |
| `test_arch` | [Test Architecture Tests] |
| `vfs-compliance` | [VFS compliance tests] |
| `vulkan` | [Vulkan tests] |

[Learn][create-test-realm] how to create your own test realm.

### Non-hermetic legacy test realms

These are legacy test realms created before we had
[Test Manager as a Service][test-manager-as-a-service]. We are in process of
porting these realms. If your tests depend on one of these realms, it should
explicitly mark itself to run in the legacy realm as shown below.

```json5
// my_component_test.cml

{
    include: [
        // Select the appropriate test runner shard here:
        // rust, gtest, go, etc.
        "//src/sys/test_runners/rust/default.shard.cml",

        // This includes the facet which marks the test type as 'starnix'.
        {{ '<strong>' }}"//src/devices/testing/starnix_test.shard.cml",{{ '</strong>' }}
    ],
    program: {
        binary: "bin/my_component_test",
    },
    {{ '<strong>' }}
    use: [
        {
            protocol: [ "fuchsia.vulkan.loader.Loader" ],
        },
    ],{{ '</strong>' }}
}
```

The shard includes following facet in the manifest file:

```json5
{% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="src/starnix/tests/starnix_test.shard.cml" %}
```

Possible values of `fuchsia.test.type`:

| Value | Description |
| ----- | ----------- |
| `hermetic` | Hermetic realm |
| `chromium-system` | Chromium system test realm |
| `google` | Google test realm |

## Restricted logs

By default, a test will fail if it logs a message with a severity of
`ERROR` or higher. See this [guide][restricted-logs] for more information.

## Performance

When writing a test runner that launches processes, the runner needs to
provide a [library loader][loader-service] implementation.

Test runners typically launch individual test cases in separate processes to
achieve a greater degree of isolation between test cases. However this can come
at a significant performance cost. To mitigate this, the test runners listed
above use a [caching loader service][caching-loader-service] which reduces the
extra overhead per process launched.

## Test roles {#test-roles}

Components in the test realm may play various roles in the test, as follows:

- Test root: The component at the top of a test's component tree. The URL for
  the test identifies this component, and the test manager will invoke the
  [`fuchsia.test.Suite`][test-suite-protocol] exposed by this component to
  drive the test.
- Test driver: The component that actually runs the test, and implements
  (either directly or through a [test runner](#test-runners) the
  [`fuchsia.test.Suite`][test-suite-protocol] protocol. Note that the test
  driver and test root may be, but are not necessarily, the same component:
  the test driver could be a subcomponent of the test root which re-exposes its
  `fuchsia.test.Suite`, for example.
- Capability provider: A component that provides a capability that the test
    will exercise somehow. The component may either provide a "fake"
    implementation of the capability for test, or a "real" implementation that
    is equivalent to what production uses.
- Component under test: A component that exercises some behavior to be tested.
    This may be identical to a component from production, or a component written
    specifically for the test intended to model production behavior.

## Troubleshooting {#troubleshooting}

This section contains common issues you may encounter while developing test components
with the Test Runner Framework. If one of your test components fails to run, you may see
an error like the following from `fx test`:

```none {:.devsite-disable-click-to-copy}
Test suite encountered error trying to run tests: getting test cases
Caused by:
    The test protocol was closed. This may mean `fuchsia.test.Suite` was not configured correctly.
```

To address the issue, explore the following options:

- [The test is using wrong test runner](#troubleshoot-wrong-test-runner)
- [The test failed to expose `fuchsia.test.Suite` to test manager](#troubleshoot-test-root)
- [The test driver failed to expose `fuchsia.test.Suite` to the root](#troubleshoot-test-routing)
- [The test driver does not use a test runner](#troubleshoot-test-runner)

### The test is using wrong test runner {#troubleshoot-wrong-test-runner}

If you encountered this error during test enumeration then probably you are
using the wrong test runner.

For example: your Rust test file might be running the
test without using Rust test framework (i.e it is a simple Rust binary with its
own main function). In this case change your test manifest file to use
[elf_test_runner](#elf-test-runner).

Read more about in-built [test runners](#test-runners).

### The test failed to expose `fuchsia.test.Suite` to test manager {#troubleshoot-test-root}

This happens when the test root fails to expose `fuchsia.test.Suite` from the
[test root](#test-roles). The simple fix is to add an `expose` declaration:

```json5
// test_root.cml
expose: [
    ...
    {
        protocol: "fuchsia.test.Suite",
        from: "self",  // If a child component is the test driver, put `from: "#driver"`
    },
],
```

### The test driver failed to expose `fuchsia.test.Suite` to the root {#troubleshoot-test-routing}

Your test may fail with an error similar to the following if the `fuchsia.test.Suite`
protocol is not properly exposed:

```none {:.devsite-disable-click-to-copy}
ERROR: Failed to route protocol `/svc/fuchsia.test.Suite` from component
`/test_manager/...`: An `expose from #driver` declaration was found at `/test_manager/...`
for `/svc/fuchsia.test.Suite`, but no matching `expose` declaration was found in the child
```

If the [test driver and test root](#test-roles) are different components, the test driver
must also expose `fuchsia.test.Suite` to its parent, the test root.

To address this issue, ensure the [test driver](#test-roles) component manifest includes
the following `expose` declaration:

```json5
// test_driver.cml
expose: [
    ...
    {
        protocol: "fuchsia.test.Suite",
        from: "self",
    },
],
```

### The test driver does not use a test runner {#troubleshoot-test-runner}

The [test driver](#test-roles) must use the appropriate [test runner](#test-runners)
corresponding to the language and test framework the test is written with.
For example, the driver of a Rust test needs the following declaration:

```json5
// test_driver.cml
include: [ "//src/sys/test_runners/rust/default.shard.cml" ]
```

Also, if the test driver is a child of the [test root](#test-roles), you need
to offer it to the driver:

```json5
// test_root.cml
offer: [
    {
        runner: "rust_test_runner",
        to: [ "#driver" ],
    },
],
```

## Further reading

- [Complex topologies and integration testing][integration-testing]: testing
  interactions between multiple components in isolation from the rest of the
  system.

[caching-loader-service]: /src/sys/test_runners/src/elf/elf_component.rs
[cf]: /docs/concepts/components/v2/
[Chromium test realm]: /src/sys/testing/meta/chromium_test_realm.shard.cml
[component-manifest]: /docs/concepts/components/v2/component_manifests.md
[component-unit-tests]: /docs/development/components/build.md#unit-tests
[create-test-realm]: /docs/development/testing/components/create_test_realm.md
[CTF test realm]: /docs/development/testing/ctf/test_collection.md
[custom-artifact-example]: /examples/tests/rust/custom_artifact_test.rs
[Device tests]: /src/devices/testing/devices_test_realm.shard.cml
[DRM tests]: /src/media/testing/drm_test_realm.shard.cml
[ffx]: /docs/development/tools/ffx/overview.md
[fidl-test-manager]: /sdk/fidl/fuchsia.test.manager/test_manager.fidl
[fidl-test-suite]: /sdk/fidl/fuchsia.test/suite.fidl
[framework-capabilities]: /docs/concepts/components/v2/capabilities/protocol.md#framework
[fx-test]: https://fuchsia.dev/reference/tools/fx/cmd/test
[integration-testing]: /docs/development/testing/components/integration_testing.md
[loader-service]: /docs/concepts/process/program_loading.md#the_loader_service
[manifests-offer]: https://fuchsia.dev/reference/cml#offer
[manifests-use]: https://fuchsia.dev/reference/cml#use
[resolvers]:  /docs/concepts/components/v2/capabilities/resolver.md
[restricted-logs]: /docs/development/diagnostics/test_and_logs.md#restricting_log_severity
[runners]: /docs/concepts/components/v2/capabilities/runner.md
[Starnix tests]: /src/sys/testing/meta/starnix-tests.shard.cml
[subpackages]: /docs/concepts/components/v2/subpackaging.md
[System Validation Tests]: /src/testing/system-validation/meta/system_validation_test_realm.shard.cml
[system-test-realm]: /src/sys/testing/meta/system-tests.shard.cml
[Test Architecture Tests]: /src/sys/testing/meta/test-arch-tests.shard.cml
[test-manager-as-a-service]: /docs/contribute/governance/rfcs/0202_test_manager_as_a_service.md
[test-suite-protocol]: /docs/concepts/components/v2/realms.md
[unit-tests]: /docs/development/components/build.md#unit_tests_with_generated_manifests
[VFS compliance tests]: /src/sys/component_manager/tests/capability_provider_vfs_compliance/vfs_compliance_test_realm.shard.cml
[Vulkan tests]: /src/lib/vulkan/vulkan_test_realm.shard.cml
