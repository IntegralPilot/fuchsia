# Driver Metadata Examples

Here are examples of how to send, retrieve, and forward metadata between drivers using the `//sdk/lib/driver/metadata` library.

## Defining Metadata

The `fuchsia.examples.metadata` FIDL library is an example of how to define metadata.

## Sending Metadata

The `sender` driver is an example of how a driver can send metadata to its child nodes.

## Retrieving Metadata

The `retriever` driver is an example of how a driver can retrieve metadata from parent node.

## Forward Metadata

The `forwarder` driver is an example of how a driver can forward metadata from its parent node to its child nodes.

## Testing

Include the tests to your build by appending `--with-test //examples/drivers/metadata:tests` to your `fx set` command. For example:

```
$ fx set core.x64 --with-test //examples/drivers/metadata:tests
$ fx build
```

Run unit tests with the command:
```
$ fx test metadata_example_test
```
