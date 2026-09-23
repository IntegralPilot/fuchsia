# User guide for CTF

This user guide provides a comprehensive overview of Compatibility Testing
for Fuchsia (CTF), a mechanism for freezing artifacts on release branches
and loading them on the `main` branch for testing purposes.

The most common use case of CTF is to freeze a test on a release branch and
then run that test against components on the `main` branch in CI/CQ. This
prevents backwards-incompatible changes that would break pre-built clients.

To learn more about CTF as a whole, see the [overview] and [motivation]
documents.

## Basic usage

This section describes how to use CTF and how to handle common scenarios.

### Freezing a Fuchsia Package

To freeze a Fuchsia package called {{ '<var>' }}my-package{{ '</var>' }}, do the following:

1. Change the package from `fuchsia_package` to `ctf_fuchsia_package`.
2. Include the `{PACKAGE_NAME}_archive` target in
[//sdk/ctf/tests:tests](https://fuchsia.googlesource.com/fuchsia/+/HEAD/sdk/ctf/tests/BUILD.gn#7).

Example:

```gn
# Old src/{{ '<var>' }}my-package{{ '</var>' }}/BUILD.gn
import("//build/components/fuchsia_package.gni")

fuchsia_package("{{ '<var>' }}my-package{{ '</var>' }}") {
  # ...
}
```

Change your file to this:

```gn
# New src/{{ '<var>' }}my-package{{ '</var>' }}/BUILD.gn
import("//sdk/ctf/build/ctf.gni")

ctf_fuchsia_package("{{ '<var>' }}my-package{{ '</var>' }}") {
  # ...
}
```

And update `//sdk/ctf/tests/BUILD.gn`:

```gn
group("tests") {
  deps = [
    # ...
    "//src/{{ '<var>' }}my-package{{ '</var>' }}:{{ '<var>' }}my-package{{ '</var>' }}_archive",
  ]
}
```

You can also create a `group("ctf_tests")` in your local `BUILD.gn`,
and reference that in `//sdk/ctf/tests/BUILD.gn`.

This will result in your package being compiled and stored in
`ctf-artifacts`
when the next Fuchsia milestone is released. After completing this step, you
**must** include instructions for loading your package (see next section).

### Loading a Fuchsia Package

To load a frozen Fuchsia package you need to update
[`generate_ctf_tests.gni`][generate_ctf_tests]
to include instructions for loading your package.

You will need to define a new template in the `generate_ctf_tests.gni`
file that is named `generate_{{ '<var>' }}my-package{{ '</var>'
}}`, and this template must expand to a Fuchsia test package.

The template will receive a parameter called `test_info` which
includes the downloaded package's label as `test_info.target_label`.

For example:

```gn
template("generate_{{ '<var>' }}my-package{{ '</var>' }}") {
  forward_variables_from(invoker, [ "test_info" ])
  fuchsia_package_with_test(target_name) {
    test_component = "//src/{{ '<var>' }}my-package{{ '</var>' }}:{{ '<var>' }}my-package{{ '</var>' }}-test-root",
    test_component_name = "test-root.cm"
    subpackages = [
      test_info.target_label, # prebuilt version of the test suite.
      # other packages needed to implement the test, such as a RealmFactory.
    ]
  }
}
```

### Freezing a host test

To freeze a host test do the following:

1. Change the build rule from `host_test` to `ctf_host_test`.
2. Include that target in
[//sdk/ctf/tests:tests](https://fuchsia.googlesource.com/fuchsia/+/HEAD/sdk/ctf/tests/BUILD.gn#7).

Example:

```gn
# Old src/{{ '<var>' }}my-host-test{{ '</var>' }}/BUILD.gn
import("//build/testing/host_test.gni")

host_test("{{ '<var>' }}my-host-test{{ '</var>' }}") {
  # ...
}
```

Change your file to this:

```gn
# New src/{{ '<var>' }}my-host-test{{ '</var>' }}/BUILD.gn
import("//sdk/ctf/build/ctf.gni")

ctf_host_test("{{ '<var>' }}my-host-test{{ '</var>' }}") {
  # ...
}
```

And update `//sdk/ctf/tests/BUILD.gn`:

```gn
group("tests") {
  deps = [
    # ...
    "//src/{{ '<var>' }}my-host-test{{ '</var>' }}:{{ '<var>' }}my-host-test{{ '</var>' }}",
  ]
}
```

You can also create a `group("ctf_tests")` in your local `BUILD.gn`,
and reference that in `//sdk/ctf/tests/BUILD.gn`.

This will result in your host test and its dependencies being bundled
in `ctf-artifacts` when the next Fuchsia milestone is released.
After completing this step, your test will automatically run in CTF
using any arguments you pass to `ctf_host_test`.

## Basic concepts

CTF consists of two mechanisms:

1. Build rules to freeze an artifact and upload it to [CIPD].
1. [Build rules][generate_ctf_tests] to thaw that artifact and turn
   it into a test.

The freezing and loading process is exercised at the tip of the `fuchsia.git`
repository for local tests through the `//sdk/ctf` build target. Including
`--with-test //sdk/ctf` in your `fx set` args results in the following
build output:

```bash
{{ '<var>' }}$FUCHSIA_DIR/out/default{{ '</var>' }}/cts/host_test_manifest.json
{{ '<var>' }}$FUCHSIA_DIR/out/default{{ '</var>' }}/cts/package_archives.json
{{ '<var>' }}$FUCHSIA_DIR/out/default{{ '</var>' }}/cts/*.far       # Packages referenced in manifest
{{ '<var>' }}$FUCHSIA_DIR/out/default{{ '</var>' }}/cts/host_x64/*  # Host tests referenced in manifest
```

This results in both the original tests and the thawed version of those tests
being included in your build:

```bash
$ fx test --dry
...
  fuchsia-pkg://fuchsia.com/fuchsia-diagnostics-tests-latest#meta/fuchsia-diagnostics-tests-root.cm
  fuchsia-pkg://fuchsia.com/fuchsia-driver-test_tests-package#meta/test-root.cm
...
  fuchsia-pkg://fuchsia.com/fuchsia-diagnostics-tests_ctf_in_development#meta/fuchsia-diagnostics-tests-root.cm
  fuchsia-pkg://fuchsia.com/fuchsia-driver-test_tests_ctf_in_development#meta/test-root.cm
```

Rules defined in [`generate_ctf_tests.gni`][generate_ctf_tests] are
responsible for thawing packages referenced in `package_archives.json`
by turning them into normal Fuchsia test packages. Since these
packages were thawed from the CTF build at the tip of tree, they
all have a `ctf_in_development` suffix. All of these tests can be
run using `fx test`.

Each Fuchsia milestone release (for example, F16 and F17) is
associated with a release branch in git (for example,
[`refs/heads/releases/F16`][release-f16]. The process above happens
for each change to a release branch, resulting in a new `ctf`
directory containing packages and host tests referenced in manifests.
On release branches, however, the resulting directory is zipped and
uploaded to [CIPD] where it is downloaded as a prebuilt by
`jiri run-hooks`. You can find these downloaded prebuilts in your
`fuchsia.git` checkout at `prebuilt/ctf/f*`, where there is one
directory per Fuchsia milestone release. The source of truth for
which milestones' prebuilts are downloaded is
[`version_history.json`][version-history], which lists `supported`
API levels to test.

The build target defined at `//sdk/ctf/release:tests` iterates over
the downloaded prebuilts and applies the
[`generate_ctf_tests.gni`][generate_ctf_tests] rules to thaw each
downloaded package in the same way that the in-development packages
were thawed. They can be included using `--with-test
//sdk/ctf/release:tests` in your `fx set` args.

This will add new tests for each supported level to your build:

```bash
$ fx test --dry
...
  fuchsia-pkg://fuchsia.com/fuchsia-diagnostics-tests_ctf18#meta/fuchsia-diagnostics-tests-root.cm
  fuchsia-pkg://fuchsia.com/fuchsia-driver-test_tests_ctf18#meta/test-root.cm
...
  fuchsia-pkg://fuchsia.com/fuchsia-diagnostics-tests_ctf19#meta/fuchsia-diagnostics-tests-root.cm
  fuchsia-pkg://fuchsia.com/fuchsia-driver-test_tests_ctf19#meta/test-root.cm
```

Each thawed test has a suffix indicating the release branch where
the frozen package originated (e.g. ctf18, ctf19 for F18 and F19
respectively). All of these tests can be run using `fx test`.

## Troubleshooting failures

CTF tests are designed to fail when there is an incompatibility
between the frozen package and packages produced at tip of
`fuchsia.git`, and this can cause the `core.x64-debug-ctf` builder
to fail during CL submission. See the dedicated
[troubleshooting guide][troubleshooting] for detailed instructions
on handling failure scenarios to unblock CL submission.

[CIPD]: https://chrome-infra-packages.appspot.com/p/fuchsia/cts/linux-amd64
[generate_ctf_tests]: https://fuchsia.googlesource.com/fuchsia/+/HEAD/sdk/ctf/build/generate_ctf_tests.gni
[motivation]: /docs/development/testing/ctf/compatibility_testing.md#motivation
[overview]: /docs/development/testing/ctf/overview.md
[troubleshooting]: /docs/development/testing/ctf/troubleshooting.md
[version-history]: https://fuchsia.googlesource.com/fuchsia/+/HEAD/sdk/version_history.json
[release-f16]: https://fuchsia.googlesource.com/fuchsia/+/refs/heads/releases/f16
