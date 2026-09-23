# Frequently Asked Questions

Note: Compatibility testing documents are being overhauled in Q2
2024.  Some information in this document is out of date. See
https://fxbug.dev/339893257 for details.

## What is Fuchsia CTF? {#what-is-cts}

Please see the [CTF overview] for an explanation of what CTF is.

## What is the CTF release schedule? {#schedule}

CTF has multiple releases with separate cadences:

| Release  | Schedule |
|----------|----------|
| Canary   | ~4 hrs   |
| Milestone| ~6 weeks |

The canary release is created when new canary releases of the Fuchsia platform
are created. Likewise, milestone releases are created when new milestone releases
of the Fuchsia platform are created.

Milestone branches (e.g. releases/f7) often receive cherry picks. When this
happens, a new CTF for that milestone is generated and automatically rolled
into CI/CQ.

{% dynamic if user.is_googler %}

Internal contributors: Look for builders named cts*prebuilt-roller in Milo
to monitor new releases.

{% dynamic endif %}

## When will my CTF test start running on CQ? {#wait-time}

The tip-of-tree version of your test will immediately begin running on CI/CQ.
This version of the test does not guarantee backward compatibility.

When the next CTF release is cut, it will contain a snapshot of your test from
tip-of-tree which will begin running as soon as that CTF release rolls into
CI/CQ.  This version of your test guarantees backward compatibility.

See [this section](#schedule) above for the release schedule.

## What test environment does CTF use in CQ? {#environments}

See go/fuchsia-builder-viz. Look for builders whose names end in "-cts".

At minimum, all CTF tests run on the core.x64 image in the Fuchsia emulator.

## How can I tell which version of a CTF test is failing? {#which-test-version}

CQ may run several versions of the same CTF test at a time: The version from
tip-of-tree, from the latest canary release, and from a previous milestone
release.

CTF test packages are named after the Fuchsia API level they test:

| Version | Example package name |
|-|-|
| tip of tree  | my_test |
| canary       | my_test_apicanary |
| API level $N | my_test_api$N |

The full package URL will look something like:

```text
fuchsia-pkg://fuchsia.com/my_test_api24#meta/my_test.cm
```

## How do I reproduce a CTF test failure locally? {#repro}

To build and run a specific [version](#which-test-version) of a test, you can
use the following examples:

```sh
# Build the test.
fx set //sdk/ctf/tests/fidl/fuchsia.example:tests
fx build

# Run all versions.
fx test

# Run the version for API level 20.
fx test fuchsia.example_test_api20
```

Please also see [this guide][run_fuchsia_tests] about running Fuchsia tests.

## What do I do if a CTF test is blocking my CL? {#broken-test}

This is a sign that your CL is breaking a part of the platform surface area.
Please verify that there are no projects in downstream repositories that rely
on the APIs and ABIs modified by your CL. If so, you will need to make a
soft transition. The general workflow is as follows:

1. Submit a CL that introduces the new behavior in your change and verify that
   the tip-of-tree version of the CTF test passes.
1. Notify any downstream SDK Users of the upcoming breaking change, ask them to
   migrate and depend on the new behavior.
1. Wait for the next CTF release to roll into CI/CQ.
1. Submit a CL to remove the old behavior.

## Are there any examples of CTF tests? {#examples}

See [//sdk/ctf/examples] and [//sdk/ctf/tests].

## When and why should I write a CTF test? {#why-cts}

You should write a CTF test if the software being tested is in the public or
partner [SDK category].

## How do I remove a CTF test? {#remove-a-test}

See the section in the [contributing guide](contributing_tests.md) on
[removing tests](contributing_tests.md#remove-a-test).

## Additional questions

For additional questions please reach out to <fuchsia-ctf-team@google.com> or
file a bug in the [CTF bug component].

[CTF bug component]: https://bugs.fuchsia.dev/p/fuchsia/issues/entry?template=Fuchsia+Compatibility+Test+Suite+%28CTS%29
[CTF overview]: /docs/development/testing/ctf/overview.md
[run_fuchsia_tests]: /docs/development/testing/run_fuchsia_tests.md
[//sdk/ctf/examples]: /sdk/ctf/tests/examples/
[//sdk/ctf/tests]: /sdk/ctf/tests/
[SDK category]: /docs/contribute/sdk/categories.md
