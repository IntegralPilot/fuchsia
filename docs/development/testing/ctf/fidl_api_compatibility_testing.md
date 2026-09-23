# FIDL API compatibility testing

Note: Compatibility testing documents are being overhauled in Q2
2024.  Some information in this document is out of date. See
https://fxbug.dev/339893257 for details.

We use API compatibility tests to ensure that SDK Users targeting supported platform
API levels aren't broken by changes to FIDL APIs at tip of tree. All FIDL APIs published in
the [partner Fuchsia SDKs][SDK Categories] should be automatically tested for backward API
compatibility. This document describes what API compatibility tests are and how to use
them.

## Concepts

### FIDL versioning

The reader should be familiar with [FIDL versioning].

### API levels

To understand API compatibility tests, it's important to have a basic understanding of
[Fuchsia API levels]. An API level denotes the set of APIs available when building an
application. They are unsigned 64-bit integers which are assigned to the Fuchsia platform
in increasing order.

There are two API levels that are useful to keep in mind:

1. An in-development API level - This is what Fuchsia developers make additive changes to.
2. A stable API level - This level is stable and its surface area will not change.

The current implementation of platform versioning does not yet reflect this:
In the Fuchsia source tree, we record API and ABI version history, as well as level
status ("in-development" and "supported" aka stable) at
[//sdk/version_history.json](/sdk/version_history.json).

### API level evolution

An API level goes through several phases, illustrated by the following diagram:

```
         +----------------+ freeze +--------+  drop  +-------------+
START -> | in-development | -----> | stable | -----> | unsupported |
         +----------------+        +--------+        +-------------+
```

__In-development__

In this phase the API level is in active evolution, as new API elements
may be introduced, deprecations may be introduced, etc.
Breaking changes to APIs introduced at this level are not allowed and contributors
should make sure there are no partners still relying on APIs removed at this level.

__Stable__

The API level can no longer receive changes. Contributors should start introducing
APIs to partners at the next in-development API level. When we "freeze" an API
level, the level may no longer receive changes. This usually happens immediately
before a branch cut. It will remain supported for at least 6 weeks or until
the Fuchsia platform
drops support for it. APIs can be deprecated at this level but not deleted.

__Unsupported__

When we drop support for a level, Fuchsia contributors are free to delete or
modify any APIs at this level, and we
stop running compatibility tests for this level. There's no longer any guarantee
that end users can successfully
target this API level.

## Resolving compatibility issues

Usually compatibility issues can be fixed by adding `@available` annotations on FIDL
declarations.

Below are some good guidelines to follow when changing FIDL APIs.

1. Annotate new, not-yet-stable APIs with `@available(added=HEAD)`.
1. Annotate newly stable APIs that are ready to go to partners and will not change
   with `@available(added=NEXT)`.
1. When removing an API, first make sure no partners are still using the API, then
   annotate the old API with `@available(..., removed=NEXT)`.

For more examples, see the [API evolution guide] and the [FIDL compatibility guide].

[FIDL versioning]: /docs/reference/fidl/language/versioning.md
[Fuchsia API levels]: /docs/contribute/governance/rfcs/0002_platform_versioning.md
[SDK Categories]: /docs/contribute/governance/rfcs/0165_sdk_categories.md
[FIDL compatibility guide]: /docs/development/languages/fidl/guides/compatibility/README.md
[API evolution guide]: /docs/development/api/evolution.md