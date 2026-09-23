# Metadata Integration Test
The goal of `:metadata_integration_test` is to make sure that drivers can send
and receive metadata to each other using the `//sdk/lib/driver/metadata/cpp`
library. The test will spawn a driver test realm and include the following four
drivers: `test_root`, `metadata_sender`, `metadata_forwarder`, and
`metadata_retriever`.

## Drivers
### test_root
The `test_root`'s purpose is to create nodes that the `metadata_sender` driver
variants can bind to.

### metadata_sender
The `metadata_sender`'s purpose is to serve metadata using
`fdf_metadata::MetadataServer`. The driver has two variants:
`metadata_sender_expose` which exposes the
fuchsia.hardware.test.MetadataSenderService FIDL service in its component
manifest and `metadata_sender_no_expose` which does not. It can also create
nodes that the `metadata_forwarder` driver and `metadata_retriever` driver
variants can bid to.

### metadata_retriever
The `metadata_retriever`'s purpose is to retrieve metadata via
`fdf_metadata::GetMetadata()`. It has two variants: `metadata_retriever_use`
which declares that the driver uses the metadata FIDL service in its component
manifest and `medatata_retriever_no_use` which does not.

### metadata_forwarder
The `metadata_forwarder`'s purpose is forward metadata using
`fdf_metadata::MetadataServer::ForwardAndServe()`. The `metadata_forwarder`
driver will create a child node that the `metadata_retriever_use` driver can
bind to.
