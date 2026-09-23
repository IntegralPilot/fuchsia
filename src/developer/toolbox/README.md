# Target-side realms for tools
Toolbox is designed as a place to represent the needs of tools connecting to a
Fuchsia device. Capabilities needed by tools are routed to the toolbox realm
and then tools can access this realm to use the capabilities.

The toolbox realm solves a platform evolution and stability challenge by
creating an explicit, intentional location to make available capabilities
intended for use by tools. Without a location the tools resort to connecting
directly to the components that implement the capabilities the tools need. This
creates a tight coupling between tool versions and platform versions, thereby
making it difficult to reorganize the the platform component topology while
maintaining compatibility. With the toolbox realm the platform component
topology can change almost at will so long as capabilities are updated to
preserve availability of capabilities in the toolbox realm.
