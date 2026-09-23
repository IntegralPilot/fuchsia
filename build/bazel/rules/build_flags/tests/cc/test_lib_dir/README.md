# Test Library Directory

This directory contains a prebuilt static library `libtest_prebuilt.a` used to
verify that the `lib_dirs` and `ldflags` attributes of Fuchsia's `build_flags()`
Bazel rule work correctly.

The fact that this is a prebuilt is intentional, as it must appear as a source
file in the Bazel graph for the test to work.

It is generated from the `libtest_prebuilt.c` source file using the `./regenerate.sh`
script, in case it needs to be updated.

