# BT LE Battery Monitor

This example uses the GATT Battery Service (BAS) to monitor the battery level of connected peers.
When run, the monitor scans for compatible peers advertising the Battery service. The monitor will
initiate a GATT connection and subscribe to notification updates about the peer's battery level.

## Build

The `bt-le-battery-monitor` relies on the `fuchsia.bluetooth.le.Central` capability to scan and
connect to remote peers.

Add the example to your Fuchsia configuration. Note, because the component is eagerly started, make
sure to include the package in the cached set of packages. For example, for the workstation
configuration, add the `bt-le-battery-monitor` to the
[legacy_cache_package_labels](/products/common/workstation_base.gni).

Include the [core_shard](meta/bt-le-battery-monitor.core_shard.cml) in your target product
configuration. For example, if using `fx set`, add:

```
--args 'core_realm_shards += ["//src/connectivity/bluetooth/examples/bt-le-battery-monitor:core-shard"]'
```

## Component startup

The example `bt-le-battery-monitor` is implemented as a Component Framework v2 component. This
component is eagerly started on device boot.
