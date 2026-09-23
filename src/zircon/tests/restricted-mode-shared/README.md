# Restricted Mode Tests w/ Shared Processes

This directory contains tests that validate restricted mode functionality
when running in a shared process. It provides the kernel with a non-starnix way
to validate this functionality.

## Test types

There are two types of tests housed in this directory:

### Basic test

The basic test (in the subdirectory `basic/`) runs a simple, single-shot test
tries to enter restricted mode in a shared process and bounce back out on a
syscall. This is meant to be a basic functionality test, and by no means is a
comprehensive test of restricted mode features.

### Stress test

The stress test (in the subdirectory `stress/`) has two variants: a short
version and a long version. Both do roughly the same thing, see the comments
in `stress/lib.cc` for more information on what the test does. The main
difference between the two is how long they run for:
* The long version runs for an hour, and runs only on QEMU and VIM3 as those
are the only platforms that Fuchsia Infra runs kernel stress tests on. Because
it runs so many iterations, it's more likely to catch race conditions in the
unified aspace implementation.
* The short version runs for 10 seconds, and runs on emulators, NUC, and VIM3.
It is less likely to find race conditions, as it runs for a much shorter period
of time, but that is the tradeoff we must make to run on NUCs and ARM
emulators.

Each version of the test is a separate fuchsia test package.

## How to run the tests

The tests can be run like pretty much any other fuchsia component test. When
building, make sure to include
`--with //src/zircon/tests/restricted-mode-shared:tests` in your `fx build`
invocation. Then, you can run the tests as follows:
```
// Basic test
$ fx test fuchsia-pkg://fuchsia.com/restricted-mode-shared-test#meta/restricted-mode-shared.cm

// Short stress test
$ fx test fuchsia-pkg://fuchsia.com/restricted-mode-shared-short-stress-test#meta/restricted-mode-shared-short-stress.cm

// Long stress test
$ fx test fuchsia-pkg://fuchsia.com/restricted-mode-shared-long-stress-test#meta/restricted-mode-shared-long-stress.cm
```

## Limitations

Note that ASAN does not work with shared processes yet, so these tests are
excluded from the build graph if ASAN is enabled.
