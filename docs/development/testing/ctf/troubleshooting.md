# Troubleshooting failures

This guide provides an overview of common scenarios encountered when
troubleshooting Compatibility Test for Fuchsia (CTF) test failures and walks
through some reasons for breakages and ways to unblock CL submission.

CTF tests assert on interactions between software frozen on a release branch and
software built on the `main` branch of `fuchsia.git`. If those assertions fail
due to an incompatibility, CL submission will be blocked.

The purpose of CTF is not to ban compatibility-breaking changes, rather it is
meant to make such breakages visible so that they can be appropriately
addressed.

## Scenarios

In each scenario, a real compatibility issue is introduced which causes a CTF
test to fail and block CL submission. The compatibility issue can be as simple
as changing the return value of an existing FIDL protocol used by a test. We
assume the following context common to each scenario (see [motivation] for more
testing scenarios):

-   A FIDL client is frozen as a `ctf_fuchsia_package` on F19 called
    `echo-service-tests`. This connects to a FIDL protocol and asserts on the
    responses.

-   [`generate_ctf_tests.gni`][generate_ctf_tests] contains a rule
    `template("generate_echo-service-tests")` which merges the incoming client
    package with a server package built on `main`. (see the [user guide] for
    details).

-   The client calls a method on the server called `Echo` which takes as input a
    string and returns as output a string as follows:

    ```c++
    auto server_proxy = connect_to_named_protocol("my_protocol.EchoService");
    ASSERT_EQ(
      server_proxy.echo("Hello"),
      "Hello"
    );
    ```

Suppose we want to change the behavior of echo to instead return the *lowercase*
representation of the string it is passed. We can update our test on `main` to
do the following:

```c++
ASSERT_EQ(
  server_proxy.echo("Hello"),
  "hello"
);
```

This passes for `ctf_in_development` tests, but when the frozen client with the
old assertion is run against this new server, the test will fail:

```bash
FAILURE: "hello" != "Hello"
```

CL submission is now blocked, and the path forward depends on the reason for
this breakage.

### Unintentional breaking change - soft transition

In this scenario, we do not intend to break compatibility. This is the case if
pre-built or out-of-tree components depend on the old behavior. Running those
components against a platform containing the changes to `Echo` will result in
unexpected behavior.

In this case, the safe thing to do is a soft transition to the new behavior:

1.  Introduce a new method with the new behavior:

    ```fidl
    protocol Echo {
     // ...
     @available(added=20)
     EchoLowercase(struct {input string}) -> (string);
    };
    ```

1.  Mark the old method as deprecated or removed (optional):

    ```fidl
    protocol Echo {
     @available(removed=20)
     Echo(/* ... */) -> (string);
     // ...
    };
    ```

1.  Implement the new method in the server.

1.  Change `fuchsia.git` callers to use the new method instead of the old one.

The server must support both methods until all API levels in which the old
method exists are no longer supported. In the above example, this will be when
F19 is unsupported, since the old Echo method is removed in F20 (due to
`@available(removed=20)`).

### Intentional breaking change - change test in release branch

In this scenario, the compatibility breakage is intentional. This may arise for
several reasons:

-   We want to change the behavior of an API, and we accept the risk of
    pre-built or out-of-tree clients breaking. This is a *true positive*
    failure, which we will explicitly acknowledge.

-   The behavior that changed is internal to a test, and does not represent a
    breaking change of the SDK surface itself. This is a *false positive*
    failure, because we caught a test incompatibility rather than an SDK
    incompatibility.

In either case, the solution is to modify the release branch so that the test
will no longer fail when run against either the pre-change or post-change code
on `main`.

The change can be made as follows:

1.  Check out the release branch:

    ```bash
    fx sync-to refs/heads/releases/{{ '<var>' }}f19{{ '</var>' }}
    ```

1.  Modify the assertion so it accepts both outputs (or comment it out):

    ```c++
    EXPECT_THAT(
     server_proxy.echo("Hello"),
     AnyOf(Eq("Hello"), Eq("hello"))
    );
    ```

1.  Test that your changes will work on `main` (see below)

1.  Commit and push changes:

    ```bash
    git push origin HEAD:refs/for/releases/{{ '<var>' }}f19{{ '</var>' }}
    ```

1.  Get CL reviewed and submit.

1.  Land the blocked CL.

1.  Clean up the release branch to accept only the new behavior (optional).

    ```c++
    EXPECT_EQ(
      server_proxy.echo("Hello"),
      "hello"
    );
    ```

You can test that your changes will work when applied to `main`. You need two
checkouts of Fuchsia, one synced to the release branch and one synced to `main`.
Do the following:

1.  On the **release branch** checkout, build a new CTF bundle.

    ```bash
    fx set core.x64 --with-tests //sdk/ctf
    fx build
    ```

1.  On the **`main`** checkout, build the CTF release tests.

    ```bash
    fx set core.x64 --with-tests //sdk/ctf/release:tests
    fx build
    ```

1.  In the output directory for the **release branch**, copy the built CTF
    bundle to the **`main`** repository.

    ```bash
    cp -fR \
    {{ '<var>' }}$RELEASE_BRANCH_FUCHSIA_OUT_DIR{{ '</var>' }}/cts/* \
    {{ '<var>' }}$MAIN_BRANCH_FUCHSIA_DIR{{ '</var>' }}/prebuilt/ctf/{{ '<var>' }}f19{{ '</var>' }}/linux-x64/cts/
    ```

1.  Rebuild the **`main`** checkout, repave device or restart emulator, and run
    the tests.

1.  Revert back to the version from CIPD once tests pass.

    ```bash
    jiri run-hooks
    ```

#### Example: SDK compatibility breakage (true positive)

[This CL](https://fxrev.dev/1041178) introduced a real compatibility breakage to
`fuchsia.ui.policy.MediaButtonsListener` protocol. A new parameter was added to
the `MediaButtonsEvent` with correct versioning annotations, however, the
behavior if that parameter is left empty changed from the previous
implementation. This incompatibility was caught by the CTF test for F19:

```
../../src/ui/tests/conformance_input_tests/media-button-validator.cc:243: Failure
Expected equality of these values:
  ToString(listener.events_received()[0])
    Which is: "\n    volume: 0\n    mic_mute: 0\n    pause: 0\n    camera_disable: 0\n    power: 0"
  ToString(MakePowerEvent())
    Which is: "\n    volume: 0\n    mic_mute: 0\n    pause: 0\n    camera_disable: 0\n    power: 1"
```

It was determined that this change is acceptable because there are not any
pre-built clients of this protocol targeting API level 19.

CLs [1044994](https://fxrev.dev/1044994) and
[1049612](https://fxrev.dev/1049612) were cherry-picked into the F19 release
branch using the instructions above. Once the changes were rolled into the CTF
release used on `main`, the original CL was submitted without modification.

#### Example: Test harness compatibility breakage (false positive)

[This CL](https://fxrev.dev/1007583) introduced a compatibility breakage to the
`fuchsia.tracing.controller.Controller` protocol. The `StopTracing` method was
made `flexible`, and changing a method from `strict` to `flexible` is not
ABI-safe.

The WLAN hw-sim CTF test crashes under this change because it asserts that
tracing successfully stops during the test, even though this is unrelated to the
tested WLAN protocols and is not needed for correctness. This is a false
positive compatibility failure because it affects only the test harness itself.

[This CL](https://fxrev.dev/1014607) was landed to turn tracing failures into
warnings rather than fatal assertions, and this change was cherry-picked to the
F18 release branch. Once the change was rolled into the CTF release used on
`main`, the original CL was submitted without modification.

### Intentionally drop support for an API level for specific tests

In this scenario, we want to explicitly drop support for an API level on a
per-test basis. While [`version_history.json`][version_history] provides the
canonical listing of supported API levels, there are reasons we would want to
drop compatibility guarantees for specific protocols on a specific API level.
For example, a major refactor of a non-critical subsystem where breaking old
clients is acceptable.

In this case, we can take modify the thawing process to skip tests targeting the
API level we want to drop:

1.  Modify [`generate_ctf_tests.gni`][generate_ctf_tests] on the `main` branch
    to conditionally skip thawing the test.

    ```gn
    template("generate_my-service-tests") {
     forward_variables_from(invoker, [ "test_info" ])
     if (defined(invoker.api_level) && invoker.api_level == "15") {
       # Do not thaw this test for F15
       not_needed([ "test_info" ])
       group(target_name) {
       }
     } else {
       // ...
     }
    }
    ```

1.  Commit and submit this CL.

The above example skips thawing the entire artifact at F15. If you do not want
to skip all tests, see the previous section for how to skip individual test
cases on the release branch.

#### Example: Major refactor to diagnostics subsystem

[This CL](https://fxrev.dev/1019042) deleted support for the `DirectoryReady`
component event, which is no longer in use. Its primary use was to support
obtaining diagnostics data from components (which is now published using the
`fuchsia.inspect.InspectSink`) protocol.

Unfortunately, components built at F15 still published their diagnostics using
`DirectoryReady`, and all diagnostics CTF tests of this behavior would fail on
`main` following its removal.

Fortunately, there are no pre-built components targeting F15 for which we need
to obtain diagnostics data. A CL was submitted to [disable][disable-cl] all
diagnostics CTF tests originating in F15.

<!-- Reference links -->

[generate_ctf_tests]: https://fuchsia.googlesource.com/fuchsia/+/HEAD/sdk/ctf/build/generate_ctf_tests.gni
[motivation]: /docs/development/testing/ctf/compatibility_testing.md#motivation
[user guide]: /docs/development/testing/ctf/user_guide.md
[version_history]: https://fuchsia.googlesource.com/fuchsia/+/HEAD/sdk/version_history.json
[disable-cl]: https://fuchsia-review.googlesource.com/c/fuchsia/+/1015705/19/sdk/ctf/build/generate_ctf_tests.gni#40
