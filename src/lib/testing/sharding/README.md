# Fuchsia component test sharding

A system that shards a `fuchsia_test_component` into a `fuchsia_test_package`
containing multiple test components. Each component runs the same test binary,
but proxies its `fuchsia.test.Suite` instance through a `sharder` component that
filters for a subset of test cases whose names hash to the same shard.

This is intended to allow for Fuchsia tests that are at risk of bumping against
suite-level test timeouts (but whose individual test cases are small enough to
not time out) to be automatically sharded by test-case-name without needing
manual modification of the test source files.

## Usage

See `//src/lib/testing/sharding/tests/example_sharded_test` for a fully-worked
example, and the documentation comments on
`//src/lib/testing/sharding/fuchsia_sharded_test_package.gni` for details on all
the features of `fuchsia_sharded_test_package`.

The test being sharded needs to have its CML file configured to use the
test-sharding client shard:

```json5
{
    include: [
        // When using expectation-based testing, use
        // "//src/lib/testing/sharding/meta/client_with_expectations.shard.cml"
        // instead.
        "//src/lib/testing/sharding/meta/client.shard.cml",
        "syslog/client.shard.cml",
    ],
    program: {
        runner: "rust_test_runner",
        binary: "bin/underlying_test_to_be_sharded",
    },
}
```

Then use `fuchsia_sharded_test_package` to declare the test package in GN:

```gn
import("//build/rust/rustc_test.gni")
import("//src/lib/testing/sharding/fuchsia_sharded_test_package.gni")

rustc_test("underlying_test_to_be_sharded") {
  edition = "2024"
  sources = [ "src/lib.rs" ]
}

fuchsia_sharded_test_package("example-sharded-test") {
  test_components = [
    {
      name = "example-test"
      manifest = "meta/example-test.cml"
      deps = [ ":underlying_test_to_be_sharded" ]
      num_shards = 3

      # Captures just the "section" of the test so that cases
      # in the same section get sharded together. (optional)
      shard_part_regex = "section_(\d+)::case_.*"

      # Sharding can be used with tests that use
      # //src/lib/testing/expectation. (optional)
      expectations = "path/to/expectations_file.json5"
    },
  ]
}
```

Then `fx test example-sharded-test` will result in:
- The `example-sharded-test` package being built and published.
- The test components in that package `example-test_shard_0_of_3.cm`,
  `example-test_shard_1_of_3.cm`, and `example-test_shard_2_of_3.cm` being run.

When all is said and done, each shard's component topology looks like this:

```
      parent (test manager)

              ^      fuchsia.test.Suite
              |      (shard subset according to shard config.json)
              +-----------------------+
                                      |
+------------------------+            |
|                        |            |
|  test_shard_0_of_3.cm  |            |
|  +-------------------+ |            |
|  | <original binary> | |            |
|  +-------------------+ |            |
|                        |            |
++-----+-----------------+            |
 |     |                              |
 |     |fuchsia.test.Suite (original) |
 |     v                              |
 |   +-+------------+                 |
 |   |  sharder.cm  | +---------------+
 |   +--+-----------+
 |      ^
 |      | /pkg/data/testshards/shard_0/config.json
 +------+ as /testshard/config.json
```
