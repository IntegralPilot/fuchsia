# Bluetooth Profile Implementations

This workspace provides implementations of profiles and services defined in
[published specifications][1] by the Bluetooth SIG.

There are four types of crates: Services, Profiles, Utility and Interface

Each crate is expected to include it's own unit tests that are runnable
with `cargo test` and should be passing on every commit.  Crates that use
async features of rust are executor-agnostic - using any async executor
should be possible.

Additionally, any changes to the repository should also be formatted and
checked for style, using the commands:
```
$ cargo fmt
$ cargo check
```

## Services

Crates that implement services as defined by the published specifications.
They should provide parsed representations of any of the specified formats of
characteristics or descriptors, as well as control operations and their
arguments.

Some services crates may be usable on their own, especially if they are not
usually associated directly with a profile.

List of currently implemented or in-development services crates:
 * bt-ascs - Audio Stream Control Service
 * bt-pacs - Published Audio Capabilities Service
 * bt-bass - Broadcast Audio Scan Service
 * bt-vcs - Volume Control Service
 * bt-battery - Battery Service

## Profiles

Crates that implement all or some of a profile specification and provide an API
for use of their included services. BAP Broadcast Assistant is one example of a
role that could be a single crate.

List of currently implemented or in-development profiles crates:
 * bt-broadcast-assistant - Basic Audio Profile Broadcast Assistant Role

## Utility

Crates that implement some common set of shared code or types expected to be
shared between multiple other crates in the workspace.

List of current utility crates:
 * bt-common - Defines types and formats from the [Core Specification][core]
   or [Assigned Numbers][assigned]
 * bt-bap - Common types shared across Basic Audio Profile roles

## Interface

Crates that provide an interface to system capabilities that are required for
the other crates to work.  Each interface crate represents a requirement for
the system to provide for some of the implementations in Profiles or Services
crates.

### bt-gatt

Host stack GATT interface.

Bluetooth Host implementations using the profile and services crates above
usually require reference to `GattTypes`, and sometimes `ServerTypes`, to
allow the services and profiles crates to find, connect to, and read/write from
piconet peers.

Generally profile or service crates will be grounded in one of these two
Dependency Traits to use types provided by the platform stack for searching,
publishing services, advertising, connecting, and communicating via GATT.

`GattTypes` provides common types expected to be needed for any GATT operation,
and these types should be provided by any implementation crate.

`ServerTypes` are specific to publising a GATT service.

Implementers are encouraged to read the README in the `bt-gatt` crate to follow
best practices.

# Important Note about User Data and Privacy

As Bluetooth is often used for data sharing and can send sensitive
information, care should be taken by integrators of this library to ensure
that user data and privacy is maintained.

The crates here do not store any data persistently, and identifiers are stable
only as long as the connection is maintained.

Interface implementations should take special care to randomize peer IDs
as well as any other identifiers when possible to prevent leakage of
location and/or other information through metadata.

Indications of Bluetooth usage should be present on interfaces when possible to
make it clear to users that the technology is in use and data is being
transmitted wirelessly.

[1]: https://www.bluetooth.com/specifications/specs/
[core]: https://www.bluetooth.com/specifications/specs/core-specification-5-4/
[assigned]: https://bitbucket.org/bluetooth-SIG/public/src/main/assigned_numbers/
