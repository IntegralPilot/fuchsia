# Commit message options

When uploading changes  to [Fuchsia's Gerrit instance][gerrit-link],
there are various options that can change the behavior of presubmit.
These options are represented by special strings added to the commit message
of a change.

This page documents the options that can be used for Fuchsia presubmit.

## Buganizer issue options

These options control associated bugs in [Buganizer][buganizer].

### Bug

`Bug: #` adds a comment on the given [Buganizer][buganizer] issue
when a change is submitted.

For example:

```none {:.devsite-disable-click-to-copy}
Bug: 372314445
```

The above line resulted in [this
comment](https://issues.fuchsia.dev/issues/372314445#comment11).

### Fixed

`Fixed: #` adds a comment, **and marks as "Fixed"**, the given
[Buganizer][buganizer] issue when a change is submitted.

For example:

```none {:.devsite-disable-click-to-copy}
Fixed: 297456438
```

The above line resulted in
[this comment](https://issues.fuchsia.dev/issues/297456438#comment3)
and status change on the issue.

## Dependent changes options

### Depends-on

`Depends-on: <other-change-id>` marks the change as depending on another change,
possibly in a separate repository.

The change with the `Depends-on` footer will
not be submittable until all dependencies have been submitted, and any
dependencies will also be patched in during presubmit testing.

For example:

```none {:.devsite-disable-click-to-copy}
Depends-on: Idc82d1483b4be8480aaa87bb48af8d03cfa45858
```

{% dynamic if user.is_googler %}

To make a change in turquoise-internal-review.googlesource.com depend on a
change in fuchsia-review.googlesource.com:

```none {:.devsite-disable-click-to-copy}
Depends-on: fuchsia:I9916ccaa4b95b6e9babdee33014fa6bd3d478f2e
```

To make a change in fuchsia-review.googlesource.com depend on a
change in turquoise-internal-review.googlesource.com:

```none {:.devsite-disable-click-to-copy}
Depends-on: turquoise-internal:I9916ccaa4b95b6e9babdee33014fa6bd3d478f2e
```

{% dynamic endif %}

## Test options

These options control how tests are executed.

### Multiply

`Multiply: <test name>` will run the given test multiple times.
This is helpful to confirm that specific tests are not flaky.
This applies to builders triggered by CQ as well as manually-triggered
builders started using the **Choose Tryjobs** dropdown.

For example:

```none {:.devsite-disable-click-to-copy}
Multiply: socket-integration
```

The above line reruns the "socket-integration" test multiple times.

### Run-All-Tests

`Run-All-Tests: true` runs all tests, even if static analysis
marks them as unaffected by a change.  This option is helpful when
making changes that can implicitly affect the entire system, such
as changes to the Zircon kernel, Component Framework, Test Manager,
or Diagnostics.

### Cq-Include-Trybots

`Cq-Include-Trybots <list>` runs the given builders as part of presubmit
for the change, in addition to the default set of presubmit builders.

For example:

```none {:.devsite-disable-click-to-copy}
Cq-Include-Trybots: luci.fuchsia.try:fuchsia-coverage-absolute
```

The above line forces execution of `fuchsia-coverage-absolute`
along with the other jobs for presubmit.

Use the **Choose Tryjobs** dropdown in Gerrit to see the full set of
available builders.

<!-- Reference links -->

[buganizer]: https://issues.fuchsia.dev
[gerrit-link]: https://fuchsia-review.googlesource.com/
