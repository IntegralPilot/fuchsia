# Subpackage Examples

The following directory hosts examples resolving components from
[Subpackages (RFC-0154)](/docs/contribute/governance/rfcs/0154_subpackages.md).
Subpackages are explained in the
[developer guide](/docs/concepts/components/v2/subpackaging.md).

The tests are meant to demonstrate compilable code. This means that
it will always contain the latest API surface of each of the client libraries.

## Building

If these components are not present in your build, they can be added by
appending `--with //examples:tests` to your `fx set` command. For example:

```bash
$ fx set core.x64 --with //examples --with //examples:tests
$ fx build
```

## Testing

Run the tests for all languages using the `subpackage-examples` package.

```bash
$ fx test subpackage-examples
```

When successfully executed, the test output should include something similar to
the following:

```
[00028.249197][echo_client] INFO: Server response: Hello Fuchsia!
[00028.249630][echo_client] INFO: Server response: Hello subpackaged server!
```
