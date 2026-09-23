# SDK history

This directory contains golden files for the APIs exposed by the platform at
each API level. The covered APIs include those exposed directly to developers in
the IDK/SDK and indirectly via prebuilts in the IDK/SDK. Additionally, any API
that has compatibility checks, such as those used by host tools and for CTF test
configuration, are covered. (See [SDK categories] for more information.)

## Subdirectories

There is a subdirectory for each API level in the [Supported] or [Sunset] phase
plus `NEXT`. See
[Numerical subdirectory addition and removal](#subdirectory-changes).

## Golden files

For FIDL libraries, there is a golden `<library_name.api_summary.json` file for
each API level. If a library is not supported at a specific API level, the
corresponding file will be empty. This correlates with the presence of generated
files (e.g., build rules and C header files) when targeting any Supported API
level in the SDK, even when the API is not supported. For unstable FIDL
libraries, the golden files for all API levels are empty.

For shared object libraries, there is a golden `lib<library_name>.ifs` file for
each API level that represents the functions exposed by the library at that
specific API level.

Source and static libraries are not yet covered by golden files.

## Allowed changes

### Numerical API levels

No changes to the files in the numerical sub-directories are allowed as these
represent part of the [Fuchsia System Interface] for that API level. The
exceptions are the addition or removal of _empty_ files when a library is added
to or removed from, respectively, the SDK at `NEXT`.

_Note: Currently, there are very rare occasions where exceptions are made due to
limitations of the implementation of versioning mechanisms or, for example to
remove APIs that were never implemented. Over time, this will no longer be
relevant or allowed._

All changes to these directories should be explained in the commit message.

A very limited set of approvers in `FROZEN_API_LEVEL_OWNERS` enforce this
policy.

### `NEXT` API level

All changes must go through [API review]. This is enforced by
`FROZEN_API_LEVEL_OWNERS`.

When adding new files, the library API must have gone through [API calibration]
unless it is unstable.

### Numerical subdirectory addition and removal {:#subdirectory-changes}

New numerical API level subdirectories are added when a new stable API level is
created. Existing numerical API level sub-directories are removed when an API
level is [retired] and ABI
compatibility for the level is no longer required.


[SDK categories]: /docs/contribute/sdk/categories.md
[Supported]: /docs/concepts/versioning/api_levels.md#supported
[Sunset]: /docs/concepts/versioning/api_levels.md#sunset
[Retired]: /docs/concepts/versioning/api_levels.md#retired
[Fuchsia System Interface]: /docs/concepts/kernel/system.md
[API review]: /docs/contribute/governance/api_council#api_review
[API calibration]: /docs/contribute/governance/api_council#calibration