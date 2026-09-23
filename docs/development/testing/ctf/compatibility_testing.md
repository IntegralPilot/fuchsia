# Compatibility Tests for Fuchsia

Compatibility Testing for Fuchsia (CTF) is a mechanism for running
different versions of pre-built Fuchsia software against the Fuchsia
platform surface area to detect compatibility problems.

CTF tests ensure that a client frozen at a previous release milestone
is compatible with the Application Programming Interface (API) and
Application Binary Interface (ABI) made available to developers via
the SDK. It simulates the situation where a behavioral change is
landed in the Fuchsia platform that breaks a pre-compiled client.

CTF fundamentally consists of the following parts:

- A mechanism to freeze artifacts on a release branch and then use
  those artifacts in a test on the main branch.
- A set of build rules to select artifacts to freeze, and a set
  of build rules to simplify thawing those frozen artifacts on
  main.
- A suite of tests using those build rules to test the compatibility of
  old client code with the latest platform surface area.
- The coverage provided by that suite of tests, in terms of which
  FIDL methods and syscalls are tested for compatibility across
  versions.

CTF was originally proposed as [RFC 0015][rfc-0015], and the project
code is located at [`//sdk/ctf`][sdk-ctf-directory].

## Motivation

The Fuchsia platform defines a surface area consisting of a number of
FIDL protocols which are exposed in the Fuchsia SDK. Developers may
download and use the Fuchsia SDK to write software targeting Fuchsia systems
at a specific *API level*.

As the Fuchsia platform surface area evolves over time, changes are
represented through availability annotations on FIDL protocols:

```fidl
protocol Entry {
  @available(added=12)
  SetValue(struct { value string; });

  @available(added=20)
  GetTimestamp() -> (struct { timestamp int64; });

  @available(added=13, removed=20)
  Encrypt();
};
```

In the above example, the protocol `Entry` has three methods:

- `SetValue`, which was added at API level 12
- `GetTimestamp`, which was added at API level 20
- `Encrypt`, which was added at API level 13 but removed at API level
20 when it was found to not be needed any longer.

Components built using the Fuchsia SDK must declare the API level
they target, affecting which definitions are visible. Consider the following
scenarios:

- A component is built targeting API level 19:
  - `SetValue` and `Encrypt` are visible.
  - `GetTimestamp` is absent, because it did not yet exist at API level 19.
- A component is built targeting API level 21:
  - `SetValue` and `GetTimestamp` are visible.
  - `Encrypt` is absent, because it was removed at API level 20.

Not all API levels are currently supported by the Fuchsia Platform.
The canonical list of [supported API levels][version-history] is checked
in to the `fuchsia.git` repository and is used to determine which API
levels will be supported in releases of the Fuchsia SDK.

Targeting a supported API level ensures that a client can connect
only to protocols that are supported and guaranteed to be implemented,
but it does not provide a guarantee that the behavior of the platform
will remain consistent for pre-built components targeting that API
level.

Ensuring consistent behavior is important to support components
built outside of the `fuchsia.git` repository, especially when those
components are downloaded as pre-built binaries and assembled into
product images.

### Example: A compatibility problem

Consider a simplified version of the `Entry` protocol above:

```fidl
protocol Entry {
  @available(added=20)
  GetTimestamp() -> (struct { timestamp int64; });
};
```

The `GetTimestamp` method returns the timestamp of an `Entry` *in seconds*.

We can test it as follows:

```c++
TEST(Timestamp) {
  EntrySyncPtr entry = CreateEntryAtMidnightJan1stUTC();
  ASSERT_EQ(entry.GetTimestamp(), 1704067200);
}
```

Suppose that we decide we actually want nanosecond granularity. We
will change the *implementation* of `GetTimestamp` to return
nanoseconds, but note that the *FIDL definition* does not change.

We would update our test as follows:

```diff
TEST(Timestamp) {
  EntrySyncPtr entry = CreateEntryAtMidnightJan1stUTC();
- ASSERT_EQ(entry.GetTimestamp(), 17040672000);
+ ASSERT_EQ(entry.GetTimestamp(), 17040672000000000);
}
```

This will pass all checks in `fuchsia.git` and submit, but we have now
introduced a **compatibility breakage to the Fuchsia Platform**.

#### Failure timeline

Let's look at a complete timeline that leads to a breakage.

1. **Before the SDK change**: Suppose the FooWidget team wants to
   implement their FooWidget for Fuchsia. They download the latest
   Fuchsia SDK and build their component targeting API level 20. This
   component uses the `Entry` protocol and calls the `GetTimestamp`
   method. At this point in time, `GetTimestamp` returns seconds.

   This works for them, so the FooWidget team will publish a Fuchsia
   package called `foo-widget`. Upon inspection, users of this package
   see it targets API level 20.

1. **FooWidget is included in a Fuchsia product**: A Fuchsia product
   is assembled that includes the `foo-widget` package and the Fuchsia
   platform from an SDK. The `foo-widget` package was ingested as
   a pre-built binary without source code access, but product assembly
   sees that the package was built targeting API level 20 and that the
   Fuchsia platform image supports API level 20.

1. **The SDK change is rolled**: At this point, the change we defined
   above lands in `fuchsia.git`, a new SDK is produced, and it is
   released.

1. **The product is assembled with the new SDK**: The product
   repository rolls to the new SDK and assembles the product using
   a new Fuchsia platform image combined with the existing pre-built
   `foo-widget`.

1. **FooWidget is broken on that product**: When the `foo-widget`
   component calls `GetTimestamp()`, the results will be in nanoseconds
   rather than seconds, which can have very strange results.
   For instance, the FooWidget UI may start displaying times millions
   of years in the future because it is treating nanoseconds as
   seconds!

   It is dangerous to change the behavior of platform functionality
   when pre-existing software depends on the old behavior.

### Catching compatibility problems

CTF tests simulate the above scenario by taking advantage of Fuchsia's
release branching process. If a CTF test fails, it means that
pre-built components targeting an old Fuchsia platform implementation
will fail when targeting the latest platform implementation. It works
as follows:

1. Each Fuchsia [milestone release][milestone-release] has an
   associated release branch in `fuchsia.git`. This represents the
   state of Fuchsia's platform implementation at the time of release.
1. That platform implementation passes the set of tests on that
   release branch.
1. A set of artifacts are included in a *CTF Artifacts* bundle for
   that release branch.
1. When the release branch is created and/or modified, the CTF
   Artifacts for that branch are compiled and uploaded to CIPD.
1. The CTF Artifacts for each supported API level are downloaded as
   pre-builts on the `main` branch and are *thawed* by turning them
   into test packages.
1. Those test packages are run against the latest Fuchsia platform
   on the `main` branch, which will block CL submission if they fail.

This exactly maps to the scenario in which a component is built
against an old Fuchsia SDK and platform, and then is run as a
pre-built against the current Fuchsia platform.

## Background and history

CTF exists to prevent breaking changes to the implemented Fuchsia
API and ABI from landing in the platform.

CTF originally was designed to provide confidence that an arbitrary
Fuchsia system correctly implemented the platform surface area for
a particular ABI revision. This would mean that components built
for that revision would work on the system, and that the system is
backwards compatible with that revision.

The current purpose of CTF is to provide a mechanism for freezing
and "thawing" test artifacts across release branches.

This mechanism can be applied to ensure the current Fuchsia platform
supports the old API and ABI behavior for each version that is
supposed to be supported. Additionally, the frozen tests may be
applied to arbitrary Fuchsia images to achieve the original goal
of CTF if desired.

<!-- Reference links -->

[rfc-0015]: /docs/contribute/governance/rfcs/0015_cts.md
[sdk-ctf-directory]: https://fuchsia.googlesource.com/fuchsia/+/refs/heads/main/sdk/ctf/
[version-history]: https://fuchsia.googlesource.com/fuchsia/+/HEAD/sdk/version_history.json
[milestone-release]: /docs/concepts/versioning/release.md#milestone
