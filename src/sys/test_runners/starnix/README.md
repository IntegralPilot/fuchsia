# Starnix Test Runners

Reviewed on: 2021-04-14

This directory contains two [test runners][test-runner]:

  * `starnix_test_runner`: runs test binaries that are compiled for Linux.
  * `starnix_unit_test_runner`: runs unit tests for the Starnix kernel itself.

## Starnix Test Runner

The Starnix test runner runs test binaries that are compiled for Linux. Each
such Linux binary expects to run in a particular environment (e.g., a particular
system image). Starnix calls such an environment a "container."

All containers share the same `starnix_kernel.cml`, but they differ in their
configuration. A container is simply a package that contains the Starnix kernel
component along with a system image and configuration values for its
`starnix_kernel.cml`.

The Starnix test runner expects each test component to use a
`fuchsia.component.runner/ComponentRunner` FIDL protocol capability, which will
be used to run the test binary. Such capability is exposed from a starnix
container component. Typically the container and the use declaration is included
from an existing test cml shard, e.g.
[`debian_container_for_test.shard.cml`][debian-shard].

This means that the same test runner can be used for all Starnix containers, and
each test component runs hermetically.

To create a new Starnix test component, it needs to include
`starnix_test.shard.cml`, and include the appropriate container used by the test.
Here's an example that is using `bionic_container`.

```
include: [
    "//src/starnix/containers/bionic/meta/bionic_container_for_test.shard.cml",
    "//src/starnix/tests/starnix_test.shard.cml",
]
```

Then we need to add a `program` block describing the command that will be run
using this container. Here's an example running `/system/bin/sh -c ls`.

```
program: {
    binary: "/system/bin/sh",
    args: [
        "-c",
        "ls",
    ],
},
```

## Starnix Unit Test Runner

This runner is intended to be used by the starnix kernel's unit tests. It is a
wrapper around the regular Rust test runner that adds additional permissions
required by starnix unit tests that shouldn't be available to regular Rust test
components.

[test-runner]: ../README.md
[bionic]: https://android.googlesource.com/platform/bionic/
[debian-shard]: https://cs.opensource.google/search?q=src%2Fstarnix%2Fcontainers%2Fdebian%2Fmeta%2Fdebian_container_for_test.shard.cml&ss=fuchsia%2Ffuchsia