# Fuchsia package server

Almost all software running on Fuchsia is organized into
[Fuchsia packages][fuchsia-packages], and a Fuchsia package
server manages the distribution of Fuchsia packages for Fuchsia
devices. In a Fuchsia ecosystem, one or more dedicated Fuchsia
package servers act as a secure hub for Fuchsia devices to
query and fetch the latest Fuchsia software.

## Serving Fuchsia packages

At its core, a Fuchsia package server is a specialized HTTP(S)
server that hosts and distributes Fuchsia packages. A Fuchsia
package is a hierarchical collection of files that provides
one or more programs, components, or services to a Fuchsia
system. When a Fuchsia device needs to install new software
or update existing one, it uses an available Fuchsia package
server to download the necessary packages for the install or
update.

## Security through signed packages

Every Fuchsia package’s BLOBs is cryptographically signed
using TUF ([The Update Framework][tuf]{:.external}). This
security mechanism guarantees that a Fuchsia package
delivered by a Fuchsia package server originates from a
trusted source and its contents remain unchanged. In turn,
this mechanism ensures that only trusted and verified
software updates can be pushed to Fuchsia devices.

## Package servers for developers

Fuchsia developers can use a local Fuchsia package server
setup to work with packages that are part of the product
foundation as well as additional packages that are compiled
during development. The
[`ffx repository server`][ffx-repository-server] command can
help set up and manage Fuchsia package servers in a
development environment.

<!-- Reference links -->

[fuchsia-packages]: package.md
[tuf]: https://en.wikipedia.org/wiki/The_Update_Framework
[ffx-repository-server]: /docs/development/tools/ffx/workflows/start-package-servers.md
