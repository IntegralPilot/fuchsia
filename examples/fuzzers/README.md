# Example Fuzzers

Please note: some of the specific behaviors of these fuzzers are currently
relied upon for manual end-to-end and integration testing, so when making any
changes please ensure that the `//tool/fuzz:tests` e2e tests still pass and
update if necessary. For `cpp:crash_fuzzer` and `cpp:overflow_fuzzer` in
particular, behavioral changes should additionally be validated against the
ClusterFuzz integration tests. See https://fxbug.dev/42140318 for more details.
