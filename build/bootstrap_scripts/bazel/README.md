This directory contains a script, and support files, to build Bazel from sources.

When using the --fuchsia-dir option, it will use the Fuchsia toolchain
and Linux sysroot, this generates a binary that can run on Glibc 2.27
based systems, as required by some of Fuchsia partners.

For the record:
  - Ubuntu 18.04 uses Glibc 2.27, Ubuntu 20.04 uses 2.31
  - Debian 9 used 2.24, Debian 10 jumped to 2.28
  - Centos 7 used 2.18, Centos 8 jumped to 2.28

The main reason to do this is to build a version of Bazel that includes
Fuchsia-specific patches. The generated files have similar size and
performance than the official Bazel releases.

For more information, see `build-bazel.sh --help`
