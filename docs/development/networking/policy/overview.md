# Network policy

This page provides an overview of the network policy components in Fuchsia.

## Current components

[`Reachability`]: A component for running network health checks to determine the
service level of networks known to the host. It exposes the
[`fuchsia.net.reachability`] API, which other components can use to subscribe to
changes in network service levels.

[`Netcfg`]: A component for naming and installing discovered interfaces into the
network stack. It can also perform provisioning, such as starting a DHCP client.

[`Http Client`]: A FIDL server for [`fuchsia.net.http.Loader`], backed by
fuchsia-hyper.

## Future components

### Multi-networking support

**fuchsia_network_monitor_fs**: A Unix-compatible filesystem in [Starnix][starnix]
that receives properties of installed networks and communicates property updates
to the socket proxy.

**Socket Proxy**: A component for tracking currently registered networks and
assigning an appropriate mark to sockets that need one. Communicates with
`fuchsia_network_monitor_fs` to be notified of networks registered with Starnix.

## Testing

Netemul integration tests enable testing the components in hermetic test
environments and with virtual networks. Fuchsia currently has
[netemul integration test suites][netemul] for `reachability` and `netcfg`.
Tests not relying on interactions with the netstack exist within the
component-specific directories.

<!-- Reference links -->

[`Reachability`]: /src/connectivity/policy/reachability/README.md
[`fuchsia.net.reachability`]: /sdk/fidl/fuchsia.net.reachability/reachability.fidl
[`Netcfg`]: /src/connectivity/policy/netcfg
[`Http Client`]: /src/connectivity/policy/http-client
[`fuchsia.net.http.Loader`]: /sdk/fidl/fuchsia.net.http/client.fidl
[netemul]: /src/connectivity/policy/tests/integration
[starnix]: /docs/concepts/starnix/README.md
