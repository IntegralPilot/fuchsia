# Starnix diagnostics tests

Integration tests to ensure the Starnix kernel's diagnostics match expectations.

## Setup tips and tricks

You can see *all* of the collaborators in a test realm like so, while a test is running.

```
ffx component list | grep test_root
```

This takes a few dozen seconds to complete, so do not despair or stop the command.

For tests that complete too fast for a human to react with the above command, you can add an
infinite loop in a test, and then inspect the resulting tree of realms at your leisure.  Familiarize
yourself with  the commands `ffx component {list,show,doctor,route,capability}`. These can be used
to diagnose any routing issues.

Or you can try using the debugger to attach to a process to cause it to freeze, though I had mixed
results with that approach.

For quite a while it was confusing to me where the capability routing of each of the realms and
collections is configured.  Getting the capability routing correct was by far the most time
consuming task in writing this test fixture.  And it is nontrivial, since dynamic collections
are sometimes statically defined, but sometimes dynamically created.  Furthermore, there are
opaque issues in internal routing which one must understand.

Incorrect routing and errors can manifest themselves in a number of ways, all of which are
confusing.  `ZX_ERR_PEER_CLOSED`, `ZX_NOT_FOUND`, stuck code, crashes without logs, almost anything
is fair game when routing is wrong.

Attempting to fix routing issues can lead to all sorts of fun side quests.  Finding bugs in routing,
missing a single route component leading to a FIDL call hanging, incessant `ZX_ERR_PEER_CLOSED` when
there are policy errors.  Eventually you get this right.

Logs from your test realm, *and* from component manager are both helpful in
diagnosis. Sadly, many integration tests I see only create a partial test fixture,
leading to many more errors from component manager than happen in production setups.
Unfortunately, you must use your good sense to figure out which ones are real
issues and which are red herrings, since even an `INFO` log line may carry
significant error information.

## Interpreting the test realm

One of the most taxing issues I had was cross-referencing the component and collection monikers to
where they are actually configured.  Here is a chart for interpreting the config-to-component
mapping for the test realm created in this test.

A running test setup in a test realm will contain component monikers like this one:

```
core/testing/starnix-tests:auto-88244dafd3dafdaf/test_wrapper/test:test_root/realm_builder:auto-a546ece1c56ea6e9/driver_test_realm/realm_builder:0
```

This long moniker implies several levels of nested collections.  Capability routing may need
to happen at each level for the test to run correctly.


* `core/testing/starnix-tests:auto-88244dafd3dafdaf`

  This is an auto-numbered collection under `.../starnix-tests`.

  Capabilities defined in [starnix-tests][st]. Note that not all capabilities can
  be routed willy-nilly. Routing is subject to [policy restrictions][sr].

  * `.../test_wrapper/test:test_root`

    This is a test root collection.

    Capabilities defined in [test root collection][trc].

    * `.../realm_builder:auto-a546ece1c56ea6e9

    This collection is created if you use the realm builder server. Either directly,
    or via the include directive: `include: "sys/component/realm_builder.shard.cml"`.

    To route capabilities in or out of this collection, the target is named
    `#realm_builder` by convention.

    * `.../driver_test_realm`

      This is the test realm created by the realm builder above, if you use `DriverTestRealm` in
      your test, and call `DriverTestRealm::driver_test_setup()`. It is created based on a
      [template][tt], and you can add to it as well.

      Capabilities are configured programmatically in your test.

      * `.../realm_builder:0`

        This is the final collection. This realm builder is created by the test realm,
        and contains the actual drivers and collections.

        This realm is configured [here][hh].  I noted that I had to add to this
        realm definition for it to work for me.  However, there is no simple way
        to modify this default programmatically at the time of this writing.

        There probably should be.

[st]: https://cs.opensource.google/fuchsia/fuchsia/+/main:src/sys/testing/meta/starnix-tests.shard.cml?q=starnix-tests
[sr]: https://cs.opensource.google/fuchsia/fuchsia/+/main:src/security/policy/component_manager_policy.json5;l=28
[trc]: suspend_inspect/meta/integration_test.cml.TODO
[tt]: https://cs.opensource.google/fuchsia/fuchsia/+/main:sdk/lib/driver_test_realm/meta/driver_test_realm.cml
[hh]: https://cs.opensource.google/fuchsia/fuchsia/+/main:sdk/lib/driver_test_realm/meta/test_realm.cml
