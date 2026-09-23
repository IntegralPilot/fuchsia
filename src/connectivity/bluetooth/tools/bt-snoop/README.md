# bt-snoop

`bt-snoop` is a standalone tool that monitors Bluetooth packets sent to & received by the
local controller. It makes this data available through the `fuchsia.bluetooth.snoop.Snoop`
capability.

## Build

There are two main ways to include the `bt-snoop` package in your build.

1. If complete Bluetooth functionality is required, include it via the
[`core` Bluetooth group](//src/connectivity/bluetooth/BUILD.gn). Also include the
[snoop core shard](//src/connectivity/bluetooth/tools/bt-snoop/meta/bt-snoop.core_shard.cml).

2. If only `bt-snoop` is required, include the package in your build. For example, if using
`fx set`, add:
`--with //src/connectivity/bluetooth/tools/bt-snoop`.
Also include the [snoop eager core shard](//src/connectivity/bluetooth/tools/bt-snoop/meta/bt-snoop-eager.core_shard.cml)
so that the component starts on boot.
