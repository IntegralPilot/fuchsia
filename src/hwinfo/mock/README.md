# mock

Mock hwinfo component. You can include this component in tests that depend on
hwinfo to have full control over the return values of `fuchsia.hwinfo` FIDL
protocols.

## Building

To add this component to your test, subpackage it as follows:

```
fuchsia_test_package {
    ...
    subpackages = ["//src/hwinfo/mock:package"]
}
```

You can then instantiate the mock component using the subpackage
URL `hwinfo-mock#meta/mock.cm`. See
`integration/testing/realm-factory/src/realm_factory_impl.rs` for
example RealmBuilder code using the mock.

## Use

Use the `fuchsia.hwinfo.mock.Setter/SetResponses` method to set
expected responses for the `fuchsia.hwinfo.{Board,Product,Device}`
protocols. The mock will drop your connection and emit an error if
you attempt to read before calling and awaiting the result of
`SetResponses`.

## Testing

Integration tests for mock are available in the `hwinfo-mock-test`
package.

```
$ fx test hwinfo-mock-test
```