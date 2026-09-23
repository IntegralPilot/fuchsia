# Shared Test Orchestration Tool

This directory contains Fuchsia's shared test orchestrator and fixtures.

Design: http://go/shared-infra-test-orchestration

The `orchestrate` tool (located at `//tools/orchestrate/cmd:orchestrate`) runs
as the swarming test bot entrypoint for all OOT Bazel-based repositories and
for google3 ftx tests.

## Distribution
The tool is uploaded to CIPD using the host_prebuilts-* CI builders and rolled
to fuchsia-infra-bazel-rules for use in OOT Bazel-based repositories.

It is also included in the vendor/google IDK for distribution to google3 to
allow for testing via fargon.
See http://go/orchestrate-cipd-distribution.

## Self Testing
Orchestrate has both unit and e2e tests, but unit testing alone is usually
insufficient when checking against regressions.

### Unit Testing
It's simple to run Orchestrate's unit tests locally:
```bash
fx add-test "//tools/orchestrate:tests(//build/toolchain:host_x64)"
fx test //tools/orchestrate
```

These unittests are also configured to run automatically in CQ.

### E2E Testing
E2E tests provide a higher testing fidelity. You can add them by running the
following command:
```bash
fx lsc presubmit <CL URL> google3 bazel_sdk
```

Alternatively, you can select manually select the following tryjobs on any
cl:
 - `turquoise/smart.try/sdk-google-linux-google3`
 - `sdk-bazel-linux-fuchsia_infra_bazel_rules`
 - `fuchsia/try/sdk-bazel-linux-intel_wifi_driver`

## Shared Fixtures
TODO(b/315216126): Make a note about test fixtures here.
