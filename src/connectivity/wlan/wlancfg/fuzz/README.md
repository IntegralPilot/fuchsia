# WLAN Fuzz Test

## What is tested

The fuzz test in this directory tests the `SavedNetworksManager`. It tests the storing, looking up, and persistence of saved networks. The SSID and credentials to save are fuzzed.

## Where to see fuzz test results

The fuzz test can be run locally using `ffx fuzz`. Fuchsia fuzzers also run regularly on [ClusterFuzz](https://google.github.io/clusterfuzz/) and bugs are filed for failing tests on [Monorail](https://bugs.fuchsia.dev/p/fuchsia/issues/list?q&can=41018254).

More information can be found on the [Fuchsia site fuzzing page](https://fuchsia.dev/fuchsia-src/development/testing/fuzzing/handle-results)

## Adding Tests

More tests can be easily added alongside the existing fuzz test. Tests can be written similar to unit tests, but with the fuzz attribute and with variables to be fuzzed set as test arguments.

In this fuzz test, NetworkIdentifier and Credential can be used as fuzz input because they derive the [Arbitrary](https://docs.rs/arbitrary/0.4.0/arbitrary/trait.Arbitrary.html) trait. If another type is needed and does not already implement Arbitrary, the Arbitrary trait can be derived for that type or the value can be constructed within the fuzz test using a buffer of bytes.

For more information, go to [the Fuchsia doc for writing a fuzzer](https://fuchsia.dev/fuchsia-src/development/testing/fuzzing/write-a-fuzzer#rust_1).