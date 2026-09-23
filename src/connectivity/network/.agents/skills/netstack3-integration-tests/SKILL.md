---
name: netstack3-integration-tests
description: >
  Guides on how to write proper, reliable, and fast Netstack3 integration and
  FIDL tests by referencing high-quality patterns in
  src/connectivity/network/tests/.
---

# Writing Netstack3 Integration Tests

Integration tests for Fuchsia's Netstack3 networking stack (Netstack3) live
under `src/connectivity/network/tests`. These tests verify the correctness of
the network stack's behavior when running as a Fuchsia component, interacting
with other system services, and processing network traffic over virtualized
environments. For all intents and purposes, the netstack thinks it's running in
a real environment.

To write proper, fast, and non-flaky integration tests, you should leverage
existing high-quality patterns instead of starting from scratch.

## Core Concepts

Netstack3 integration tests use `netemul` (Network Emulator), a powerful
sandboxing framework. Netemul allows you to:

1.  Spawn isolated component instances (called Realms) representing isolated,
    virtualized environments.
2.  Attach fake devices (called Endpoints) to individual netstack instances.
3.  Create virtualized Ethernet/IP networks.
4.  Connect realms to these networks programmatically to simulate physical
    network topologies.
5.  Intercept and mock capabilities that the netstack depends on.

A common mental model is to think of netemul tests as allowing you to put the
full as-deployed-in-real-life Netstack3 into a box and poke at it, sending
various kinds of packets and seeing how it responds (by looking at the packets
it sends back out).

## Test Structure & Parameterization

Most integration tests use the `#[netstack_test]` attribute macro from
`netstack_testing_macros` and are registered in a centralized `BUILD.gn` file.

### Central BUILD.gn and Package Naming

Integration test suites are defined in
`src/connectivity/network/tests/integration/BUILD.gn` in the `tests` list:

```gn
tests = [
  {
    label = "sys"
  },
  {
    label = "socket"
    err_logs = true
    long = true
  },
]
```

The configuration keys affect how the test packages are generated:
- `label`: The directory name containing the test source code (e.g., `socket/`,
  `sys/`).
- `err_logs`: If `true`, the build generates two packages:
  - `netstack-<label>-integration-test-no-err-logs`: Runs test cases that do not
    log errors.
  - `netstack-<label>-integration-test-with-err-logs`: Runs test cases that log
    errors (configured with `max_severity = "ERROR"`). If `false` (default), it
    generates a single package named `netstack-<label>-integration-test`.
- `long`: If `true`, the test is marked as heavyweight and the build will
  automatically shard it (typically into 10 shards) to run faster in CQ.

When running tests, you may always specify the base name of the test (e.g.
`netstack-socket-ingration-test`), but for tests with `err_logs = true`, this
will run two different underlying targets. You may also run those directly,
which is also required if you're trying to filter the tests, since any target
without a matching test name will fail.

```bash
./scripts/fx test netstack-socket-integration-test-no-err-logs
./scripts/fx test netstack-socket-integration-test-err-logs
```

### The `#[netstack_test]` Macro

This macro automates the creation of test variants for different Netstack
versions or IP versions.

- `#[variant(N, Netstack)]`: Permutes the test for `Netstack2` and `Netstack3`.
  The macro appends `_ns2` or `_ns3` to the test case name.
- `#[variant(I, Ip)]`: Permutes the test for `Ipv4` and `Ipv6`. The macro
  appends `_v4` or `_v6` to the test case name.

Example:
```rust
#[netstack_test]
#[variant(N, Netstack)]
#[variant(I, Ip)]
async fn test_ping<N: Netstack, I: Ip>(name: &str) { ... }
```
This expands into four distinct tests:
- `test_ping_ns2_v4`
- `test_ping_ns2_v6`
- `test_ping_ns3_v4`
- `test_ping_ns3_v6`

If you don't use `#[variant]`, you must still use `#[netstack_test]` to receive
the test name as the first argument (e.g. `name: &str`), which is useful for
creating unique realm names.

To run a specific variant, use the `--test-filter` flag:

```bash
./scripts/fx test netstack-socket-integration-test-no-err-logs --test-filter '*test_udp_socket_ns3*'
```

### Test Sharding

For heavyweight test suites (like `socket` and `multicast-forwarding`), the
central `BUILD.gn` configures sharding by setting `long = true`.

#### How it works

1.  Component Generation: The build system generates multiple shard components
    (typically 10) named `<test_name>_shard_X_of_10.cm`.
2.  Dynamic Partitioning: A helper component named `sharder` is injected into
    the test topology. It intercepts the `fuchsia.test.Suite` protocol.
3.  Grouping Regex: The sharder partitions tests using a regular expression that
    is specified in `fuchsia_sharded_test_package` with `shard_part_regex`.  For
    example, using `([^::]+)(?:::.*)?` extracts the first `::`-delimited section
    of the test name.
    - For example, in `my_module::test_foo`, the sharder groups by `my_module`.
    - All tests in the same group are guaranteed to run in the same shard. This
      prevents setup overhead if the module has shared state (though netemul
      tests should generally be hermetic).
4.  Execution: When you run the test package, all shards are launched.  Shards
    that do not contain any tests matching the active filter (or that hashed to
    different shards) will fail, which is something to watch out for.

#### Running Sharded Tests

You can run all shards together:

```bash
./scripts/fx test netstack-socket-integration-test-no-err-logs
```

#### Filtering Sharded Tests

If you try to filter for a specific test case using the direct `fx test
--test-filter` flag on the whole package, the other shards will fail because
they will have zero matching tests:

```bash
# THIS WILL FAIL (other shards will fail with "No test cases match")
./scripts/fx test netstack-socket-integration-test-no-err-logs --test-filter 'test_udp_socket_ns3::synchronous_protocol_not_mapped_to_ipv6'
```

To work around this, you must target the specific shard component directly:

Target the specific shard component directly: If you know which shard contains
the test case (e.g., from a previous run), you can run only that shard:
```bash
./scripts/fx test netstack-socket-integration-test-no-err-logs_shard_2_of_10 --test-filter 'test_udp_socket_ns3::synchronous_protocol_not_mapped_to_ipv6'
```

Note: Passing the filter to the underlying test runner using `--` (i.e., `fx
test <suite> -- --test-filter <filter>`) does **not** work. The filter is
ignored by the runner, and all tests in all shards will still run.

## Recommended Testing Patterns & Code Blueprints

When writing a new integration test, identify which of the following
architectural tiers fits your needs and reference its recommended code blueprint
in `src/connectivity/network/tests/integration/`.

### Pattern 1: Basic Socket Testing (Fast & Deterministic)

Use this pattern when you want to test application-level socket APIs (UDP, TCP,
raw sockets) over the netstack, without dynamic network topology changes.

- Reference Code: `src/connectivity/network/tests/integration/socket/src/lib.rs`
  (around `test_udp_socket`)
- Key Best Practices Demonstrated:
  - Single-Network Setup: Creates a simple virtual network `"net"` and attaches
    two endpoints (client and server).
  - Static Neighbor Entries (Flake Prevention): Dynamic ARP/NDP resolution can
    introduce nondeterministic latency or packet drops, leading to flaky tests.
    This blueprint registers static neighbor entries using the
    `fuchsia.net.neighbor` protocol:
    ```rust
    // In socket/src/lib.rs:
    realm.add_neighbor_entry(interface_id, ip_address, mac_address).await;
    ```
    This ensures immediate socket delivery and prevents test flakiness.
  - Async Rendezvous: Uses `futures::future::join` to coordinate client and
    server execution concurrently inside their respective realms.
  - Standard Async Wrappers: Utilizes Fuchsia's async socket wrappers like
    `fasync::net::UdpSocket::bind_in_realm` to seamlessly interact with the
    socket within the target sandbox realm.

### Pattern 2: Multi-Realm / Routing Topologies

Use this pattern when simulating complex networking environments (e.g., a router
connecting two subnets, firewalls/filtering, packet forwarding, or NAT).

- Reference Code:
  `src/connectivity/network/tests/integration/forwarding/src/lib.rs` (around
  `forwarding` and `ping_other_router_addr`)
- Key Best Practices Demonstrated:
  - Multi-Realm Sandbox Construction: Spawns three realms (`client`, `router`,
    and `server`) across two distinct virtual networks (`client_net` and
    `server_net`).
  - Interface Administration: Demonstrates how to dynamically control interface
    properties such as IP address assignment, default gateways, and unicast
    forwarding programmatically using `fnet_interfaces_ext::admin::Control`:
    ```rust
    // Dynamically enable routing/forwarding on an interface
    let configuration = fnet_interfaces_admin::Configuration {
        ipv4: Some(fnet_interfaces_admin::Ipv4Configuration {
            unicast_forwarding: Some(true),
            ..Default::default()
        }),
        ..Default::default()
    };
    control.set_configuration(&configuration).await;
    ```
  - End-to-End Assertions: Uses `ping::new_unicast_sink_and_stream` or TCP
    listener/connect in the separate client/server realms to verify that packets
    are successfully routed across the middle routing realm based on
    administrative configuration.

### Pattern 3: Mocking System Dependencies

Use this pattern when the Netstack interacts with other Fuchsia system
components, and you need to mock out or intercept these dependencies to verify
Netstack's outbound requests.

- Reference Code: `src/connectivity/network/tests/integration/sys/src/lib.rs`
  (around `ns_sets_thread_profiles`)
- Key Best Practices Demonstrated:
  - Capability Interception: Declares a mock provider using `netemul::ChildDef`
    with `Mock` as its source:
    ```rust
    // Intercept scheduler profile provider capability
    netemul::ChildDef {
        name: Some("profile_provider_mock".to_string()),
        source: Some(netemul::ChildSource::Mock(mock_provider)),
        ..Default::default()
    }
    ```
  - Mocking with `ServiceFs`: Runs a custom FIDL mock server using
    `fuchsia_component::server::ServiceFs` inside the test itself to handle
    calls from the netstack.
  - Verifying Outbound Requests: Captures incoming FIDL requests (e.g.
    `ProfileProviderRequest::RegisterCpuProfile`) and asserts that the Netstack
    requests the expected thread configurations and thread roles (e.g.
    `"fuchsia.netstack.go-worker"`) upon startup.

## FIDL API Contract Tests

While integration tests in `src/connectivity/network/tests/integration/` focus
on end-to-end scenarios and network traffic, FIDL compatibility tests live under
`src/connectivity/network/tests/fidl/`.

These tests verify that Netstack3 (and Netstack2, where applicable) correctly
implements Fuchsia network FIDL protocols, particularly for "userspace" control
and state query APIs.

Key areas tested here include:
- `interfaces`: Verifying `fuchsia.net.interfaces/State` and `Watcher` behavior
  (e.g., event ordering, address assignment states).
- `interfaces-admin`: Verifying administrative control over interfaces (e.g.,
  enabling/disabling, adding/removing addresses).
- `routes`: Verifying route table inspection.
- `routes-admin`: Verifying routing table manipulation.
- `neighbor`: Verifying neighbor table (ARP/NDP) APIs.
- `sockets`: Verifying socket provider APIs.

### Test Structure & Key Files

Each FIDL test suite under `src/connectivity/network/tests/fidl/` has its own
subdirectory (e.g. `routes/`) and typically contains:
1.  `BUILD.gn`: Defines a `rustc_test` target with dependencies.
   ```gn
   rustc_test("routes") {
     edition = "2024"
     output_name = "netstack_routes_fidl_test"
     deps = [
       "//src/connectivity/network/testing/netemul/rust:lib",
       "//src/connectivity/network/tests/integration/common:netstack_testing_common",
       "//src/connectivity/network/tests/integration/macros:netstack_testing_macros",
       # ...
     ]
     sources = [ "src/lib.rs" ]
   }
   ```
2.  `src/lib.rs`: The Rust test implementation. Uses `#[netstack_test]` to run
    against the under-test netstack.
3.  `meta/netstack-<label>-fidl-test.cml`: The component manifest.
   - For non-sharded tests:
     ```json5
     {
         include: [
             "//src/connectivity/network/tests/integration/common/client.shard.cml",
             "//src/lib/testing/expectation/meta/client.shard.cml",
             "syslog/client.shard.cml",
         ],
         program: {
             runner: "rust_test_runner",
             binary: "bin/netstack_routes_fidl_test",
         },
     }
     ```
   - For sharded tests:
     ```json5
     {
         include: [
             "//src/connectivity/network/tests/integration/common/client.shard.cml",
             "//src/lib/testing/sharding/meta/client_with_expectations.shard.cml",
             "syslog/client.shard.cml",
         ],
         program: {
             runner: "rust_test_runner",
             binary: "bin/netstack_filter_fidl_test",
         },
     }
     ```
4.  `expectations/<label>.json5`: Expectations for pass/fail/skip of test cases.

### Registration in the Parent BUILD.gn

All FIDL test suites are registered in the parent
`src/connectivity/network/tests/fidl/BUILD.gn`.
- Non-sharded tests are added to the `non_sharded_tests` list:
  ```gn
  non_sharded_tests = [
    {
      label = "routes"
      err_logs = true
    },
    {
      label = "sockets"
    },
  ]
  ```
- Sharded tests are built using the `fuchsia_sharded_test_package` template.

### How Error Logs are Managed (`err_logs = true`)

Because the test runner automatically fails any test case that outputs logs of
`ERROR` severity or higher, the framework splits suites into two packages when
`err_logs = true`:
1.  `netstack-<label>-fidl-test-no-err-logs` (uses `SKIP_CASES_WITH_ERROR_LOGS`
    expectation treatment).
2.  `netstack-<label>-fidl-test-with-err-logs` (uses
    `RUN_ONLY_CASES_WITH_ERROR_LOGS` expectation treatment).

In the `expectations/<label>.json5` file:
- Test cases that are expected to output error logs must be annotated with the
  action types `expect_pass_with_err_logs` or `expect_failure_with_err_logs`.
- These cases will automatically be skipped in the `-no-err-logs` package, and
  will only run in the `-with-err-logs` package.

### Step-by-Step Guide to Writing a New FIDL Test Suite

To create a new FIDL integration test suite:
1.  Create a subdirectory under `src/connectivity/network/tests/fidl/`, e.g.
    `src/connectivity/network/tests/fidl/my_feature`.
2.  Write a `BUILD.gn` that compiles a `rustc_test("my_feature")`.
3.  Write `src/lib.rs` using standard netemul setup helpers like
    `TestSandboxExt::create_netstack_realm` to spin up a sandboxed realm.
4.  Create the test component manifest under
    `src/connectivity/network/tests/fidl/meta/netstack-my_feature-fidl-test.cml`.
5.  Create an expectations file under
    `src/connectivity/network/tests/fidl/expectations/my_feature.json5`.
6.  Add `my_feature` to the `non_sharded_tests` list in
    `src/connectivity/network/tests/fidl/BUILD.gn`.
7.  Build and run:
   ```bash
   fx build
   fx test netstack-my_feature-fidl-test
   ```

## Test Expectations (Flake & Failure Management)

Integration and FIDL tests use the expectations framework to manage known
failures or tests that should be skipped.

- For integration tests, expectations live in
  `src/connectivity/network/tests/integration/expects/` and are named
  `netstack-<label>-integration-test.json5`.
- For FIDL tests, expectations live in
  `src/connectivity/network/tests/fidl/expectations/` and are named
  `<label>.json5`.

Example expectation file:
```json5
{
    actions: [
        {
            type: "expect_pass",
            matchers: [
                "*",
            ],
        },
        {
            type: "expect_failure_with_err_logs",
            matchers: [
                "udp_recv_msg_postflight_fidl_ns3*",
            ],
        },
        {
            type: "skip",
            matchers: [
                "tcp_connect_bound_to_device_ns2",
            ],
        },
    ],
}
```

If you add a test that is known to fail on a specific netstack variant (e.g.
Netstack2 because it lacks a feature), you should add it to the expectations
file rather than disabling it in code.

## Best Practices & Common Pitfalls

1.  Always Use Static Neighbor Entries for Protocol Tests: Unless your test is
    explicitly verifying dynamic ARP/NDP behavior, configure static neighbors.
    This makes tests faster and eliminates dynamic network race conditions.
2.  Apply NUD Flake Workaround in Multi-Netstack Tests: Tests that involve
    multiple netstacks communicating with each other (e.g., via pings or
    sockets) can experience spurious Neighbor Unreachability Detection (NUD)
    failures when running on slow infrastructure (CQ). Unless the test is
    explicitly verifying NUD behavior, you should apply the NUD flake workaround
    to the interfaces. This increases the number of multicast solicitations to
    prevent premature NUD timeouts.
    ```rust
    // Import TestInterfaceExt from netstack_testing_common::interfaces
    client_interface.apply_nud_flake_workaround().await.expect("nud flake workaround");
    ```
3.  Use `--test-filter` to run a specific test case and avoid running the whole
    suite:
    ```bash
    # For non-sharded tests:
    ./scripts/fx test netstack-sys-integration-test --test-filter test_name

    # For sharded tests, you must target the specific shard:
    ./scripts/fx test netstack-socket-integration-test-no-err-logs_shard_2_of_10 --test-filter test_name
    ```
