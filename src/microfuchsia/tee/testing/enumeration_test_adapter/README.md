# Enumeration test adapter

This is an adapter component that combines a test using the basic ELF test
runner with a configuration file listing tests. This can be useful with test
frameworks that do not support dynamically enumerating tests or for frameworks
that enumerate tests in a format that we do not support.

## Use

This adapter works as a component that implements `fuchsia.test.Suite` and
instantiates instances of a test component or components in a provided
container. The expected component topology looks something like this:

```
Test realm root:
  children: [
    enumeration-test-adapter
  ]
  collection: <A>
  offer [
    {
      [ capabilities needed by test instances ],
      to: #<A>
    }
    {
      // Provide access to collection <A>
      protocol: fuchsia.component.Realm
      to: #enumeration-test-adapter
    }
  ]
  expose [
    {
      protocol: fuchsia.test.Suite
      from: #enumeration-test-adapter
    }
  ]
```

When using the expectations comparer, instead of exposing fuchsia.test.Suite
from #enumeration-test-adapter instead offer that to the expectation comparer
component and then expose fuchsia.test.Suite from it like so:

```cml
  offer: [
    ...
    {
      protocol: "fuchsia.test.Suite",
      from: "#enumeration-test-adapter",
      to: "#expectation-comparer",
    },
  ],
  expose: [
    {
      protocol: "fuchsia.test.Suite",

      // #expectation-comparer is added by
      // src/lib/testing/expectation/meta/common.shard.cml
      from: "#expectation-comparer",
    },
  ],
```

The root test realm can contain multiple collections for containing tests. This
is useful if different test cases need different capabilities from each other.

The adapter also requires a configuration file specifying the test cases and
invocation details for each case including the component URL, arguments, and
collection to instantiate the components within. This configuration file must
be provided at the path `test_config.json5` in a directory capability named
`config` offered to the enumeration component. This can be included directly in
the package containing the test root realm as a resource and offered from the
framework provided pkg directory like so:

```cml
  {
    directory: "pkg",
    subdir: "enumeration_test_adapter_config/config",
    as: "config",
    from: "framework",
    to: "#enumeration-test-adapter",
  }
```

For example, if `my_test_component.cm` is a test with cases "1" and 2" selected
by the command line argument `--test-case=` then the following config file:

```json5
{
  cases: [
    {
      name: "test1",
      url: "#meta/my_test_component.cm",
      args: [ "--test-case=1" ],
      collection: <A>,
    },
    {
      name: "test2",
      url: "#meta/my_test_component.cm",
      args: [ "--test-case=2" ]
      collection: <A>,
    },
  ],
}
```

Will tell the list_config runner to expose those as "test1" and "test2" to the
fuchsia.test.Suite protocol.
