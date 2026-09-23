This directory contains the "build API tool" script described
in http://go/fuchsia-bazel-migration, and whose tracking bug is
https://fxbug.dev/42084664.

Use `build/api/tool --help` for more details.

Run `build/api/run_all_tests.sh` to run all tests locally. This
can also be done at build time by adding `//build/api:tests`
to the build graph.
