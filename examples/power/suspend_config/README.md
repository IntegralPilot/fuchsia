# Config example

This example demonstrates how a client can use the
`fuchsia.power.SuspendEnabled` configuration capability to gate its
power-management behavior at runtime. There isn't any meaningful power
management to be done here; rather, the example demonstrates how to make your
way from the configuration capability to an if/else block based on its value,
with a test protocol and component to guarantee that all works. In that respect,
it's mostly an example of how to use a configuration capability, but specific to
the capability you should use for gating power management behavior.

This example is currently implemented only in Rust. Addition of a C++ version
can be prioritized upon request.

The most relevant pieces are summarized below.

## Manifest

The configuration client's [manifest](rust/meta/power_config_client.cml)
includes a `use` declaration for the `fuchsia.power.SuspendEnabled` capability,
indicating that its value should be stored in the `should_manage_power` field of
the client's structured configuration. You can use a different field name if
you would prefer.

## Structured config

The [`fuchsia_structured_config_rust_lib` template](rust/BUILD.gn) uses the
client's manifest to generate a library for its structured configuration.

## Test protocol

The [ConfigUser](fidl/configexample.test.fidl) protocol provides a way for the
configuration client to expose whether it is managing power. The protocol is
contrived for this example - it just provides a way for the client to change
*some* behavior in a way that is observable to the test component.

## Client code

The [client](rust/src/power_config_client.rs) reads its structured
configuration. It uses the value of `should_manage_power` to determine what
value it will return to a caller
of`test.configexample/ConfigUser.IsPowerManaged`.

## Test component

The [test component](rust/src/power_config_client_integration_test.rs)
implements the `fuchsia.power.SuspendEnabled` capability with a value determined
by each test case. It routes this capability to the client, then queries the
client via `ConfigUser.IsPowerManaged` to verify that the client is using
the appropriate value of this capability.
