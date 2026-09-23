# Create Test Realm

If you or your team owns [non-hermetic tests][non-hermetic-tests] or tests which
require custom runners, you need to create a test realm to execute those tests.

Below you can see a sample test realm

```cml
{% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="src/sys/testing/meta/test_realm.cml" %}
```

[Link to the file][sample_test_realm].

## Required sections

The test realm manifest must include specific sections to facilitate the
execution of test components by the Test Manager.

### Test Collections

Every test realm must have one or more test collections where the `Test
Manager` can execute tests. In the provided sample test realm, we utilize test
collections using shards. You can refer to the [System tests
collection][system-test-realm] as an example, which can run various non-hermetic
tests.

### Expose Section

Each test realm must expose protocol `fuchsia.component.Realm`. This protocol
provides Test Manager the capability to launch components inside the realm.

### Realm Builder

Each test realm must include the [realm builder shard][realm_builder.shard.cml]
which helps the Test Manager to execute tests dynamically inside the collection.

## Optional Sections

### Test Runners

While optional, it is recommended for most test realms to include [standard test
runners][standard-runners] for executing basic tests within the realm. In cases
where custom test runners are needed, they can be included and added to the
environment of the test collection.

## Integration with in-tree build rules

Register your moniker in our [build rules][test_type_map]:

```gn
_type_moniker_map = {
  system = "/core/testing/system-tests"
  test_arch = "/core/testing/test-arch-tests"
  ...
  <new_type> = "/path/to/test/realm/moniker"
}
```

Assign the new `test_type` to your test:

```gn
// BUILD.gn
fuchsia_test_component("my_test") {
  component_name = "my_test"
  manifest = "meta/my_test.cml"
  deps = [ ":my_test_bin" ]
  test_type = "<new_type>"
}
```

Now you can run test can be executed using `fx test`.

## Out of tree tests

Integrate your testing scripts to call `ffx test` with the realm moniker:

```bash
ffx test run --realm "/path/to/test/realm/moniker" <test_url>
```

[sample_test_realm]: /src/sys/testing/meta/test_realm.cml
[system-test-realm]: /src/sys/testing/meta/system-tests.shard.cml
[realm_builder.shard.cml]: /sdk/lib/sys/component/realm_builder.shard.cml
[standard-runners]: /src/sys/testing/meta/runners/standard-test-runners.shard.cml
[test_type_map]: /build/components/fuchsia_test_component.gni
[non-hermetic-tests]: /docs/development/testing/components/test_runner_framework.md#non-hermetic_tests
