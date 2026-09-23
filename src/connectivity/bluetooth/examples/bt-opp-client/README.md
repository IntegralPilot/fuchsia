# Object Push Profile Client

This example demonstrates how to set up an OPP client that scans for and discovers remote peers
providing the OPP service.

## Build Configuration

The `bt-opp-client` component relies on the `fuchsia.bluetooth.bredr.Profile`
protocol, which is provided by the core Bluetooth service, `#bluetooth-core`.

Add the following to your Fuchsia set configuration to include the example.

```
--with //src/connectivity/bluetooth/examples/bt-opp-client
```

Include the bt-opp-client [core shard](/src/connectivity/bluetooth/examples/bt-opp-client/meta/bt-opp-client.core_shard.cml)
in your target product configuration.

## Component Startup

The example `bt-opp-client` is declared as an "eager" component -- it will start on device boot.
