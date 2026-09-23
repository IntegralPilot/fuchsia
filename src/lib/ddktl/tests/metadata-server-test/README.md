# Metadata Test
The goal of `:metadata-server-test` is to make sure that drivers can send and
retrieve metadata to each other using the `ddk::MetadataServer` class and
`ddk::GetMetadata()` function. The test will spawn a driver test realm and
include the following drivers: `metadata_sender`, `metadata_forwarder`, and
`metadata_receiver`.

## Drivers
### metadata_sender
The `metadata_sender` driver's purpose is to serve metadata to it child
devices using `ddk::MetadataServer`. It can create child
devices named `metadata_{num}` (with `{num}` being a number) that the
`metadata_forwarder` or `metadata_retrievers` drivers may bind to.

### metadata_forwarder
The `metadata_forwarder` driver's purpose is to retrieve metadata from its
parent driver and forward the metadata to its child device using
`ddk::MetadataServer::ForwardAndServe()`. It will create a child device that the
`metadata_retriever` driver can bind to.

### metadata_retriever
The `metadata_retriever` driver's purpose is to retrieve metadata using
`ddk::GetMetadata()`.
