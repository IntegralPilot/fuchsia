# Fuzzing

The Fuchsia Security team maintains tools and documentation to help developers write, build, and run
fuzzers that find bugs and security vulnerabilities.

To learn more, consult the [fuzzing documentation](/docs/development/testing/fuzzing/overview.md).

The tools themselves are located in several different locations. Most of these have OWNERS files
that point back to this location.

These locations include:

* [//build/fuzzing](/build/fuzzing): GN templates for building fuzzers.
* [//docs/development/testing/fuzzing](/docs/development/testing/fuzzing): Fuzzer documentation.
* [//examples/fuzzers](/examples/fuzzers): Illustrative sample fuzzers for reference.
* [//sdk/lib/fuzzing](/sdk/lib/fuzzing), [//sdk/lib/fidl/hlcpp/fuzzing](/sdk/lib/fidl/hlcpp/fuzzing):
  Library support for [FIDL protocol fuzzers](/docs/development/testing/fuzzing/fidl-fuzzing.md).
* [//src/developer/ffx/plugins/fuzz](/src/developer/ffx/plugins/fuzz): `ffx fuzz` tool used to
  manage v2 fuzzers.
* [//src/lib/fuzzing](/src/lib/fuzzing): Libraries to help build fuzzers and fuzzer tests.
* [//src/sys/fuzzing](/src/sys/fuzzing): The
  [fuzzing framework](/docs/contribute/governance/rfcs/0117_component_fuzzing_framework.md)
  and its engines.
* [//src/sys/test_runners/fuzz](/src/sys/test_runners/fuzz): Fuzz test runner for the
  [Test Runner Framework](/docs/development/testing/components/test_runner_framework.md).
* [//src/testing/fuzzing](/src/testing/fuzzing): Utilities for fuzzing tools that are mainly
  external to Fuchsia, including [syzkaller](https://github.com/google/syzkaller).
* [//tools/fuzz](/tools/fuzz): Undercoat tool for [ClusterFuzz][clusterfuzz] integration.

[clusterfuzz]: https://google.github.io/clusterfuzz/
