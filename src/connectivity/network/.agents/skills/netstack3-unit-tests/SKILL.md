---
name: netstack3-unit-tests
description: >
  How to write Netstack3 unit and functional tests, ranging from pure module-
  level inline tests to multi-node synchronous core simulations.
---

Unless otherwise specified, all paths that follow are relative to
`src/connectivity/network/netstack3`.

# Writing Netstack3 Unit and Core Functional Tests

Netstack3 is Fuchsia's modern, secure network stack written in Rust. To maintain
high velocity and absolute reliability, Netstack3 relies heavily on rapid,
deterministic unit and functional tests.

When developing or modifying features in Netstack3, you should select the
correct testing tier to verify your changes in isolation. You may also want to
add integration tests depending on the complexity of the feature and if it's
technically feasible to add one. See the appropriate skill for how to accomplish
that.

## Choosing the Right Testing Tier

### Tier 1: Inline Module Tests

- Location: Within implementation files.
- Focus: Pure business logic, stateless data structures, parsing, state
  machines.
- Use Case: Testing a private helper, validation rules, or specific state
  transitions.
### Tier 2: Core Functional Tests

- Location: Separate sibling `tests/src` crates (e.g. `core/ip/tests/`,
  `core/udp/tests/`).
- Focus: `CoreApi` surface, loopback routing, local interface configurations,
  forwarding, and simulated packet flows.
- Use Case: Testing multi-node virtual networks, socket lifecycle, routing table
  updates, or timer expirations (timers are mocked).
## Pattern 1: Inline Module Tests (Tier 1)

Inline module tests are the fastest, most lightweight tests. They execute
synchronously and bypass the complexity of mock devices, simulated networks, or
time-stepping.

### Best Practices & Design Patterns

- Crucial Dependency Restriction: The core implementation crates (like
  `core/udp`, `core/ip`, `core/tcp`) do NOT depend on the top-level
  `netstack3_core` crate (which actually depends on them). Therefore, Tier 1
  inline module tests cannot import anything from `netstack3_core::testutil`
  (such as `FakeCtxBuilder` or the core-level `FakeBindingsCtx`).
- Lightweight Fakes: Avoid pulling in full stack builders. Implement lightweight
  implementations of local traits directly in the test module. If
  implementations of Bindings contexts are needed, you should generally use
  `FakeBindingsCtx` (found in `core/base/src/testutil/fake_bindings.rs`) from
  `netstack3_base::testutil` for crate-level tests.
- Crate-Local Test Utilities: Many core crates define their own fake contexts
  and helpers in a local `testutil` or `testutils` module (often conditionally
  compiled with `#[cfg(any(test, feature = "testutils"))]`). Before implementing
  your own fakes, check the crate's `lib.rs` or main implementation files (like
  `base.rs`) for these modules.
  - Example: `core/udp/src/base.rs` defines `mod testutils` which provides
    `FakeUdpCtx`, `FakeUdpCoreCtx`, and `FakeUdpBindingsCtx`.
  - These utilities are often exposed to other crates for testing purposes via
    specialized GN targets named `netstack3-<crate>-testutils` (e.g.,
    `netstack3-udp-testutils`), which enable the `testutils` feature.
- API Interaction Pattern:
  - Create the fake context: `let mut ctx =
    FakeUdpCtx::with_core_ctx(FakeUdpCoreCtx::new_fake_device::<I>());`
  - Wrap it in the API: `let mut api = UdpApi::<I, _>::new(ctx.as_mut());`
  - Perform actions using `api` and local device/socket references.
  - Assert on received packets via the local fake bindings context state:
    `bindings_ctx.state.received::<I>()` (which returns a map of received
    packets per socket) instead of `take_udp_received(&socket)`.
- Logic Isolation: Focus on testing pure inputs and outputs or discrete,
  state-machine transitions. Leave full-system verification to functional and
  integration tests.
- State Assertions: Inspect the inner state variables directly (since inline
  tests can access private module members).
- Exhaustive Validation: Prefer to be exhaustive in all tests. You should
  validate every field possible and not just the trivial ones. The `assert_*`
  macros (and especially `assert_matches!` are your friends here; Use them
  liberally.
### Reference Example

- File: `core/filter/src/logic.rs` (search for `mod tests`)
- Why it's a great blueprint:
  - It comprehensively tests the core routing/filtering engine (`check_routine`)
    in absolute isolation.
  - The local `FakeIpPacket` helper struct is used to bypass constructing real
    packet buffers.
  - Asserts on specific return options (`RoutineResult::Return`,
    `RoutineResult::Drop`) with minimal boilerplate. This is an example of
    testing very complex code without requiring full networking. It does require
    specific mocking infrastructure in `FakeIpPacket`, but it's used a lot
    throughout the tests so it's not extraneous.

### TCP Socket & Functional Testing (Special Case)

Since TCP does not have a separate sibling tests crate, its functional and
socket-level tests are written inline in `core/tcp/src/socket.rs` under `mod
tests`.

- No Core-Level Fakes: Because these tests are inline, they cannot depend on
  `netstack3_core::testutil`. They define local fake context types (`TcpCoreCtx`
  and `TcpBindingsCtx`) and utilize a simulated network wrapper `TcpTestNetwork`
  around `netstack3_base::testutil::FakeNetwork`.
- Manual Trait Bounds Required: The `#[netstack3_macros::context_ip_bounds]`
  macro cannot be used in TCP inline tests. You must manually add a `where`
  clause constraining `TcpCoreCtx` for the generic IP version `I`, as shown in
  the patterns below.
- Node Constants: The simulated network uses `LOCAL` (client) and `REMOTE`
  (server) which are `&'static str` constants defined as `"local"` and
  `"remote"`.

#### Pattern A: Single-Node Tests (Stateless or Local Loopback/Socket Configuration)

Use this pattern when you want to test API calls in isolation on a single node
without simulated network exchanges.

```rust
#[ip_test(I)]
fn test_socket_marks_example<I: TcpTestIpExt>()
where
    TcpCoreCtx<FakeDeviceId, TcpBindingsCtx<FakeDeviceId>>:
        TcpContext<I, TcpBindingsCtx<FakeDeviceId>>,
{
    // 1. Create a single-node context pair with mocked IP addresses
    let mut ctx = TcpCtx::with_core_ctx(TcpCoreCtx::new::<I>(
        I::TEST_ADDRS.local_ip,
        I::TEST_ADDRS.remote_ip,
    ));

    // 2. Instantiate a TCP API client using the TcpApiExt trait
    let mut api = ctx.tcp_api::<I>();

    // 3. Create a TCP socket
    let socket = api.create(Default::default());

    // 4. Perform actions on the socket API
    api.set_mark(&socket, MarkDomain::Mark1, Mark(Some(42)));

    // 5. Assert expectations
    assert_eq!(api.get_mark(&socket, MarkDomain::Mark1), Mark(Some(42)));
}
```

#### Pattern B: Multi-Node Simulated Network Tests

Use this pattern when you want to verify handshake logic, packet drops, data
transfers, or connections between two simulated hosts: `LOCAL` (client) and
`REMOTE` (server).

```rust
#[ip_test(I)]
fn test_handshake_example<I: TcpTestIpExt>()
where
    TcpCoreCtx<FakeDeviceId, TcpBindingsCtx<FakeDeviceId>>:
        TcpContext<I, TcpBindingsCtx<FakeDeviceId>>,
{
    // 1. Setup a simulated network containing LOCAL and REMOTE hosts
    let mut net = new_test_net::<I>();
    let backlog = NonZeroUsize::new(1).unwrap();
    let server_port = NonZeroU16::new(1234).unwrap();

    // 2. Configure server on the REMOTE host
    let server = net.with_context(REMOTE, |ctx| {
        let mut api = ctx.tcp_api::<I>();
        let server = api.create(Default::default());
        api.bind(&server, None, Some(server_port)).expect("failed to bind");
        api.listen(&server, backlog).expect("failed to listen");
        server
    });

    // 3. Configure client on the LOCAL host
    let client = net.with_context(LOCAL, |ctx| {
        let mut api = ctx.tcp_api::<I>();
        let socket = api.create(Default::default());
        api.connect(&socket, Some(ZonedAddr::Unzoned(I::TEST_ADDRS.remote_ip)), server_port)
            .expect("failed to connect");
        socket
    });

    // 4. Drive the packet exchange (handshake)
    // net.step() drives a single exchange; net.run_until_idle() runs until no more packets/timers fire.
    net.run_until_idle();

    // 5. Assert the socket state
    net.with_context(LOCAL, |ctx| {
        let mut api = ctx.tcp_api();
        assert_matches!(
            api.get_info(&client),
            SocketInfo::Connection(ConnectionInfo { .. })
        );
    });
}
```

#### Pattern C: Connected Client-Server Helper

For tests that verify behavior *after* a connection is established, use the
helper `bind_listen_connect_accept_inner` to avoid connection setup boilerplate:

```rust
#[ip_test(I)]
fn test_data_transfer_example<I: TcpTestIpExt>()
where
    TcpCoreCtx<FakeDeviceId, TcpBindingsCtx<FakeDeviceId>>: TcpContext<
            I,
            TcpBindingsCtx<FakeDeviceId>,
            SingleStackConverter = I::SingleStackConverter,
            DualStackConverter = I::DualStackConverter,
        >,
{
    // 1. Establish the connection immediately (bind, listen, connect, accept)
    let (mut net, client, client_snd_end, accepted) = bind_listen_connect_accept_inner::<I>(
        I::UNSPECIFIED_ADDRESS,
        BindConfig {
            client_port: None,
            server_port: PORT_1,
            client_reuse_addr: false,
            send_test_data: false, // Don't send data immediately
        },
        0,   // Random seed for dropping packets (0 = no drops)
        0.0, // Drop rate
    );

    // 2. Queue some data on the client send buffer
    client_snd_end.lock().extend_from_slice(b"Hello from Client!");

    // 3. Tell the stack to process and send queued data
    net.with_context(LOCAL, |ctx| {
        ctx.tcp_api().do_send(&client);
    });

    // 4. Run the network so packets transit and are processed
    net.run_until_idle();

    // 5. Assert on REMOTE (server) buffers to see if data was received
    // ...
}
```

## Pattern 2: Sibling Core Functional Tests (Tier 2)

Core functional tests live in dedicated sibling test crates alongside the
implementation crate (for example, `core/ip/tests` or `core/udp/tests`). They
leverage a rich framework of simulated devices, mock stacks, and a synchronous
simulated network executor. This setup makes certain kinds of tests possible at
all (e.g. timers, which would be flaky in a full integration test) while making
tests that only execute against core not require a full-fledged running
netstack.

Unlike Tier 1 tests, these tests *do* depend on the top-level `netstack3_core`
crate, so they can use `netstack3_core::testutil::*`.

### Core Testing Utilities

- Fake Context Builder (`FakeCtxBuilder`): Located in `netstack3_core::testutil`
  (defined in `core/src/testutil.rs`). It is the primary tool for constructing a
  synchronous mock stack (`FakeCtx`) for Tier 2 tests.
  - `FakeCtxBuilder::with_addrs(I::TEST_ADDRS).build()` is the standard pattern
    to initialize a single-node stack with a single device preconfigured with
    local/remote IP and MAC addresses.
  - It returns `(FakeCtx, Vec<EthernetDeviceId<FakeBindingsCtx>>)`.
  - For dual-stack testing, you can add more IP addresses to the device after
    building using
    `ctx.core_api().device_ip_any().add_ip_addr_subnet(&device_id,
    addr_subnet)`.
- Fake Bindings Context (`FakeBindingsCtx`): The core-specialized version is in
  `netstack3_core::testutil`. It provides utility methods to inspect events and
  packets delivered to bindings, such as `take_udp_received(&socket)` to assert
  on received UDP packets.
- Test API (`TestApi`): Accessed via `ctx.test_api()`. It provides test-only
  methods to manipulate the stack state directly, such as:
  - `add_loopback()`: Adds a loopback device with default loopback addresses.
  - `add_route(entry)`: Direct insertion of routes into the forwarding table.
  - `handle_queued_rx_packets()`: Drives packet processing (essential for
    loopback delivery).
- IP-Generic Testing: Prioritize writing generic tests that run against both
  IPv4 and IPv6 to ensure protocol parity. See the dedicated "Writing IP-Generic
  Tests" section below.
- Simulated Networks (`FakeNetwork`): Orchestrates multiple virtual nodes
  connected to virtual links. It allows driving execution synchronously using
  `net.run_until_idle()` to process all timers, loops, and queued virtual
  packets.
- Mock Frame Injection: Serializes Ethernet, IP, or TCP/UDP frames
  programmatically using the `packet` and `packet_formats` crates and injects
  them into the receive path.
### Reference Examples

#### A. Multi-Node Orchestration and Socket Behavior

- File: `core/icmp_echo/tests/src/socket.rs`
- Why it's a great blueprint:
  - Uses `new_simple_fake_network` to construct a dual-host topology containing
    "alice" and "bob".
  - Connects and binds standard sockets, simulates a raw ICMP packet
    transmission from one host to another, and triggers `net.run_until_idle()`.
  - Asserts that packets are received on the peer bindings context and counters
    are incremented.
#### B. IP Version & Parametric Testing

- File: `core/udp/tests/src/loopback.rs`
- Why it's a great blueprint:
  - Perfect demonstration of the `#[ip_test(I)]` macro coupled with
    `#[test_case]` from the `test_case` crate.
  - Clean, parameterizable UDP socket binding tests over loopback using the
    `FakeCtxBuilder` setup.
#### C. Local Configuration & Forwarding

- File: `core/ip/tests/src/forwarding.rs`
- Why it's a great blueprint:
  - Focuses on routing table manipulation, local gateway configurations, and
    packet forwarding metrics within a single node stack.
  - Demonstrates how to programmatically alter routing rules and verify behavior
    without complex multi-node setups.
## Writing IP-Generic Tests

Many netstack behaviors are identical for IPv4 and IPv6. To avoid duplicating
test logic, you should write IP-generic tests that run for both versions.

### The `#[ip_test]` Macro

Use `#[ip_test(I)]` and the generic type `I: Ip` (or a trait extending it) to
write a single test body that runs once for IPv4 and once for IPv6.

This macro is widely used in both inline module tests (e.g.
`core/tcp/src/socket.rs`) and core functional tests (e.g.
`core/udp/tests/src/loopback.rs`).

### The `#[context_ip_bounds]` Macro

When writing IP-generic tests that call into core APIs, you often need to
satisfy complex trait bounds on `UnlockedCoreCtx`. The
`#[netstack3_macros::context_ip_bounds(I, FakeBindingsCtx)]` attribute macro
automatically generates these bounds.

Always apply this macro to your IP-generic test functions (or impl blocks) that
use `FakeBindingsCtx` (specifically when using core-level fakes from
`netstack3_core::testutil`) to avoid verbose and brittle trait bound compilation
errors.

Example:
```rust
#[netstack3_macros::context_ip_bounds(I, FakeBindingsCtx)]
#[ip_test(I)]
fn my_test<I: IpExt + TestIpExt>() {
    // ...
}
```

### Combining `#[ip_test]` with `#[test_case]` or `#[test_matrix]`

By default, the `#[ip_test(I)]` macro generates standard `#[test]` attributes.
If you combine it with other test-case generating macros like
`#[test_case(...)]` or `#[test_matrix(...)]`, the other macro will also try to
generate test attributes, causing a compile failure due to duplicate or
mismatched test headers.

To solve this, pass `test = false` to `ip_test` so it generates the
parameterized helper code without injecting the `#[test]` attribute, letting the
other macro drive test generation:

```rust
#[netstack3_macros::context_ip_bounds(I, FakeBindingsCtx)]
#[ip_test(I, test = false)]
#[test_case(true; "bind to device")]
#[test_case(false; "no bind to device")]
fn loopback_bind_to_device<I: IpExt + TestIpExt>(bind_to_device: bool) {
    // ...
}
```

### Using `TestIpExt` for IP-Generic Code

When writing generic tests, you often need IP-version-specific constants (like
addresses or subnets) or helper methods.

#### Global `TestIpExt`

The shared `TestIpExt` trait (and `TestDualStackIpExt` for dual-stack tests),
defined in `core/base/src/testutil/addr.rs`, provides a standard set of test
addresses via the `TEST_ADDRS` associated constant.

```rust
#[ip_test(I)]
fn my_generic_test<I: TestIpExt>() {
    let config = I::TEST_ADDRS;
    let local_ip = config.local_ip;
    let subnet = config.subnet;
    // ...
}
```

#### Local `TestIpExt` Pattern

It is a common pattern to define a local `TestIpExt` trait within your test
module to specify test-specific IP-version-generic constants or behavior. This
local trait typically inherits from `IpExt` or the global `TestIpExt`.

```rust
pub trait TestIpExt: IpExt {
    const SRC_IP: Self::Addr;
    const DST_IP: Self::Addr;
}

impl TestIpExt for Ipv4 {
    const SRC_IP: Self::Addr = net_ip_v4!("192.0.2.1");
    const DST_IP: Self::Addr = net_ip_v4!("192.0.2.2");
}

impl TestIpExt for Ipv6 {
    const SRC_IP: Self::Addr = net_ip_v6!("2001:db8::1");
    const DST_IP: Self::Addr = net_ip_v6!("2001:db8::2");
}

#[ip_test(I)]
fn test_something<I: TestIpExt>() {
    let builder = TcpSegmentBuilder::new(I::SRC_IP, I::DST_IP, ...);
    // ...
}
```

## Rules of Thumb & Common Pitfalls

1.  IP Parity: Always prioritize parameterizing your core tests with
    `#[ip_test(I)]` to guarantee test coverage for both IPv4 and IPv6 stacks
    simultaneously without duplicating logic.
2.  Use the `#[test_case]` attribute macro liberally to avoid boilerplate. You
    may need to define a custom enum for a test, but that's much better than
    needing to duplicate the test body for each case.

## Running Tests

To run the tests, use `./scripts/fx test <test_package_name>`.

### Finding the Test Package Name

1.  For Inline Unit Tests (Tier 1): Look at the `BUILD.gn` in the crate's
    directory (e.g., `core/udp/BUILD.gn`). Look for a `fuchsia_unittest_package`
    target.
    - Example: `core/udp/BUILD.gn` defines
      `fuchsia_unittest_package("netstack3-core-udp-test")`.
    - Command to run: `./scripts/fx test netstack3-core-udp-test`
2.  For Sibling Core Functional Tests (Tier 2): Look at the `BUILD.gn` in the
    `tests` subdirectory (e.g., `core/udp/tests/BUILD.gn`). Look for a
    `fuchsia_unittest_package` target.
    - Example: `core/udp/tests/BUILD.gn` defines
      `fuchsia_unittest_package("netstack3-core-udp-integration-test")`.
    - Command to run: `./scripts/fx test netstack3-core-udp-integration-test`
      Note: Sibling tests are often named `*-integration-test` in GN, but they
      are referred to as "Core Functional Tests" in this document to distinguish
      them from full system integration tests (which live in `tests/` relative
      to the netstack3 root, not `core/.../tests/`).

### Running Host Tests for Speed

Running tests on a local Fuchsia target (emulator or hardware) can be slow. You
can run unit tests directly on your host machine for faster iteration:

- For TCP tests: `./scripts/fx test netstack3-tcp-instrumented_test`
- For UDP unit tests: `./scripts/fx test netstack3-udp-instrumented_test` (host
  counterpart of `netstack3-core-udp-test`)
- For IP unit tests: `./scripts/fx test netstack3-ip-instrumented_test`
