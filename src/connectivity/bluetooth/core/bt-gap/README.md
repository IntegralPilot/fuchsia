# bt-gap
This directory contains the implementation of the public Bluetooth
APIs (`fuchsia.bluetooth*`). The main pieces of this
implementation are:
- [HostDevice](src/host_device.rs):
  - Receives FIDL events from `bt-host`, and relays them via HostDispatcher.
  - Provides thin wrappers over some of the [Bluetooth Host API](/sdl/fidl/fuchsia.bluetooth.host/host.fidl),
    for use by HostDispatcher.
- [AccessService](src/services/access.rs): Implements the `fuchsia.bluetooth.sys.Access`
   interface, calling into HostDispatcher for help.
- [PairingService](src/services/pairing.rs): Implements the `fuchsia.bluetooth.sys.Pairing`
   interface, calling into HostDispatcher for help.
- [HostDispatcher](src/host_dispatcher.rs):
  - Implements all stateful logic for the `fuchsia.bluetooth.sys.Access` interface.
  - Implements server for the `fuchsia.bluetooth.sys.HostWatcher` interface.
- [GenericAccessService](src/generic_access_service.rs):
  - Implements the GATT Generic Access Service, which tells peers the name and
    appearance of the device.
- [main](src/main.rs):
  - Binds the Access, Central, Pairing, Peripheral, and Server FIDL APIs to code within
    this component (`bt-gap`).
    - The Access API is bound to AccessService.
    - The Pairing API is bound to the PairingService.
    - Other APIs are proxied directly to their host API counterparts.
  - Serves the HostReceiver API to receive new host protocols from `bt-host` components.
  - Configures ServiceFs to process API events.

## Tests
### Unit Tests
Run Rust unit tests with:
```
$ fx test bt-gap-unittests
```
### Integration Tests
Integration tests that test bt-gap and bt-host are located at [bluetooth/tests](../../tests/)
