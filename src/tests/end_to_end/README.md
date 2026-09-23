# End to end product tests

End to end product tests for fuchsia products are located here.

Unlike other kinds of tests, end to end product tests are specific to a product,
and therefore must not be included in the global `tests` build target mandated
by the [source tree layout](../../docs/development/source_code/layout.md).
Instead, end to end product tests are included directly in the build
configuration of their products in [//products](../../products/).

# Adding your end-to-end test directory

Please also add an `OWNERS` file so that you can edit your test at will. If
there are run-time dependencies needed for the test you can add them to the
`/bundles:end_to_end_deps` group.

# Running an end-to-end test locally

Because end to end tests are tied to a specific product or even architecture,
you should review the test's README.md to see which product or special
instructions are needed to run the test.

To run an end to end test use the `run-e2e-tests` command:

```
$ fx set <product>.<arch> && fx build
$ fx run-e2e-tests name_of_the_test
```

Where `name_of_the_test` is the name of the dart_test rule (like `sl4f_test`).

# Running an end-to-end test in infra
* Refer to [CQ VS CI vs FYI](#CQ-VS-CI-vs-FYI) section to decide which test
group the new test belongs to and which builder the test need to be run on
* Accordingly add the test to corresponding test group by updating [Lacewing User Tests Infra Groups]
* If the desired group is not present in [Lacewing User Tests Infra Groups] then
  * create new group test group in [Lacewing User Tests Infra Groups]
  * Update the corresponding [Lacewing Builders] configuration file (maintained
    by Build and HW service layer team) to include this newly created group. If
    your test depends on target side packages, make sure to include the
    appropriate `*_packages` group
* If the desired [Lacewing Builders] is not present then reach out to `lacewing-admin@google.com` to create the builder
* Update the test case's `python_mobly_test` rule in BUILD.gn to include
  appropriate BOARDS (based on what all the boards this test need to be run in
  infra) in `environments` field ([example](../../testing/end_to_end/examples/test_soft_reboot/BUILD.gn))
* Add yourself to `lacewing-builder-users@google.com` group so that you will be
  notified whenever any of the tests in the builder is failed. You can then
  check for your test result in that run and triage if it has failed.

Please reach out to Lacewing team if you need help with this one.

## CQ VS CI vs FYI
Lacewing test case can be run in Infra in of the following builder types:
* CQ builder - Builds and tests pending CLs before they get checked in, to make
  it less likely that breakages will make their way to main (and thus break the
  CI).
* CI builder - Builds and tests the code that has been checked in. Any failure
  in this builder will be reported Fuchsia Build gardeners and need to be
  triaged ASAP.
* FYI builder - Build and and tests the code that has been checked in, but
  unlike CI these builders are not expected to be green at all times.

Use the following approach in deciding whether to run the test case in CQ/CI/FYI:
* Anytime a new Lacewing test needs to be run in infra, always start in FYI.
* After 200 consecutive successful runs, test case can then be promoted to CQ/CI
  based on below criteria:
  * If infra has enough testbed capacity (that is needed to run this test) then
    it can be promoted to CQ.
  * If infra has limited testbeds (that is needed to run this test) to run this
    test then it can be promoted to CI.

Once your test case is ready to be promoted to CI/CQ:
* If a corresponding `stable`
  [Lacewing User Tests Infra Groups][Lacewing User Tests Infra Groups] exists,
  and that test group is already enabled in a CQ builder, simply move your test
  target from `unstable` to `stable`.
* Otherwise, reach out to the Lacewing team for further assistance.

Based on this we have created the following:
* Test case build groups:
  * Test group naming scheme: `<PRODUCT>_<BOARD>_<STABILITY>[ |_sl4f]_tests`
    * `<PRODUCT>` - The product that the tests require to run on - e.g. "core",
        "workbench".
    * `<BOARD>` - The board that the tests require to run on - e.g. "emulator",
        "nuc", "vim3".
    * `<STABILITY>` - Whether tests are stable or flaky - "stable" or "unstable".
        All newly added tests must be added to the "unstable" groups until 200
        passing runs in infra FYI builder have been observed, after which they
        may be promoted to "stable" groups.
    * `[ |_sl4f]` - If tests require SL4F server then include "_sl4f".
        Otherwise, leave it empty.
    * Examples:
      * `core_emulator_stable_tests`
      * `workbench_vim3_unstable_tests`
  * This naming scheme is chosen to facilitate infra builder configuration.
    * `<STABILITY>` informs whether a group is potential to run in CI/CQ
        (as it depends on `<BOARD>` also).
    * `[ |_sl4f]` informs whether a test group can be run on certain products.
    * `<BOARD>` informs whether a test group can be run on certain boards.
* Package groups:
  * Package group naming scheme: `<PRODUCT>_<BOARD>_<STABILITY>[ |_sl4f]_packages`
* Builders:
  * Builder naming scheme:  `<PRODUCT>.<BOARD>-[debug|release]-lacewing[-users| ]-[ |staging|ci]`
    * for "staging" builders `-users` will be used in the builder name
    * if builder is for "CQ" then no postfix is needed but for other stages,
      postfix is necessary (`-staging` or `-ci`)
  * Link to list of all [Lacewing Builders]
  * Examples:
    * `core.x64-debug-lacewing` - CQ builder to run stable emulator and NUC tests
    * `core.x64-debug-lacewing-users-staging` - FYI builder to run unstable emulator and NUC tests
    * `core.vim3-debug-lacewing-users-staging` - FYI builder to run unstable VIM3 tests

[Lacewing User Tests Infra Groups]: BUILD.gn

[Lacewing Builders]: https://luci-milo.appspot.com/ui/builder-search?q=lacewing
