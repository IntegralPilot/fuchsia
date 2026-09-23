<!-- Generated with `fx rfc` -->
<!-- mdformat off(templates not supported) -->
{% set rfcid = "RFC-0246" %}
{% include "docs/contribute/governance/rfcs/_common/_rfc_header.md" %}
# {{ rfc.name }}: {{ rfc.title }}
{# Fuchsia RFCs use templates to display various fields from _rfcs.yaml. View the #}
{# fully rendered RFCs at https://fuchsia.dev/fuchsia-src/contribute/governance/rfcs #}
<!-- SET the `rfcid` VAR ABOVE. DO NOT EDIT ANYTHING ELSE ABOVE THIS LINE. -->

<!-- mdformat on -->

<!-- This should begin with an H2 element (for example, ## Summary).-->

## Summary

This proposal changes the definition of an API Level to be an unsigned 32-bit
integer. This supersedes [RFC-0002], which defined it as an unsigned 64-bit
integer.

## Motivation

The following code can currently be found in
[`target_api_level.gni`][target-api-level-gni] (paraphrased for clarity):

    if (override_target_api_level == -1) {
        clang_fuchsia_api_level = 4294967295
        fidl_fuchsia_api_level = "LEGACY"
    }

Roughly speaking, this says that if the API level is not specified when building
in fuchsia.git, `clang_fuchsia_api_level` should be set to `0xFFFFFFFF`, and
`fidl_fuchsia_api_level` should be set to the string value `"LEGACY"`. This
string is later interpreted by `fidlc` to be an alias for the value
`0xFFFFFFFFFFFFFFFF`, as defined in [RFC-0083].

This demonstrates two issues:

1. There is a "clang API level" and a "FIDL API level", and they are often given
   different values.
2. API levels are sometimes represented as strings, sometimes as integers.

[target-api-level-gni]: https://cs.opensource.google/fuchsia/fuchsia/+/main:build/config/fuchsia/target_api_level.gni;l=36;drc=0d3980127b610a5ad03f2216d3d6f83baa24f51e

### clang vs FIDL vs... ?

[RFC-0002] defined API levels as 64-bit integers, and subsequent RFCs have
allocated specific 64-bit integers and given them names and meaning. For
instance, `LEGACY` is identified by `0xFFFFFFFFFFFFFFFF`. But based on the code
above, we'll instead pass `0xFFFFFFFF` to Clang because, unfortunately, API
levels are limited to 32 bits in Clang.

Well, that's not exactly true.

Clang reasons about compatibility via the [`availability`
attribute][clang-availability], which represents versions as
[`VersionTuple`s][clang-versiontuple]. `VersionTuple` packs a tuple of four
integers `major[.minor[.subminor[.build]]]`, representing a version, into a
128-bit structure. Fuchsia API levels are modeled as "major versions" in this
structure, and are therefore limited to 32-bits.

Clang is a critical part of the Fuchsia platform's versioning story, so this
inconsistency must be resolved.

[clang-availability]: https://clang.llvm.org/docs/AttributeReference.html#availability

[clang-versiontuple]: https://github.com/llvm/llvm-project/blob/b8b2a279d002f4d424d9b089bc32a0e5d6989dbb/llvm/include/llvm/Support/VersionTuple.h

### `string` vs `int` vs... ?

Host tools and build systems are inconsistent in terms of the type used to
represent API levels. Even within `fuchsia.git`'s build system, there is
inconsistency, as seen in the code block above.

This isn't necessarily a problem, and this RFC doesn't fully resolve it.
However, it will attempt to provide guidance for navigating this ambiguity.

## Stakeholders

_Facilitator:_ abarth@google.com

_Reviewers:_

- ddorwin@google.com
- mkember@google.com
- haowei@google.com

_Consulted:_

- chaselatta@google.com
- phosek@google.com
- ianloic@google.com

_Socialization:_

Discussed on a [Google-internal bug][32-bit-bug] with members of the Platform
Versioning working group and the toolchain team.

[32-bit-bug]: https://fxbug.dev/311675988

## Requirements

API levels must be representable as strings, including on both command lines,
and in the name of files and directories.

API levels must be totally ordered, so we can say things like: "`Foo` was added
at API level `N`, and `N <= M`, therefore `Foo` is a part of API level `M`."

We must be able to assign some API levels special names and behaviors. The other
requirements in this section also apply to these "special" API levels.

## Design

### Integer representation

API levels will be redefined as unsigned 32-bit integers.

The lower half of this space (that is, API levels less than `0x80000000`) are
considered "normal" API levels. The upper half of this space is reserved. This
RFC and future RFCs will define the meaning of specific values greater than or
equal to `0x80000000`.

Tools that treat all API levels uniformly may ignore the distinction between
"normal" and reserved values, and treat them all as unsigned 32-bit integers,
ordered in the typical way.

Tools with special logic for particular API levels should reject input that
specifies a reserved ABI level value that it doesn't understand.

### String representation

API levels can be represented by strings in multiple different ways:

- Any UTF-8 string that represents a base-10 number in the interval \[0,
  2<sup>32</sup>) is a string representation of an API level (for example,
  `"7"`).
- A special API level can be represented by its name, given in upper case (for
  example, `"HEAD"`).
- Tools should not accept more esoteric values like `"0016"`, or `"0x20"`,
  because doing so can create ambiguity. For example, is `"0016"` in octal or
  decimal? `"2A"` is likely hexadecimal, but if it is accepted as such, is
  `"29"` hexadecimal or decimal? However, string parsing logic is often handled
  by libraries beyond an individual tool author's control, so tools _may_ accept
  such values.

Every API level has exactly one **canonical string representation**:

- For "normal" API levels, the base-10 UTF-8 string representation is canonical
  (for example, `"13"`).
- For "special" API levels, the name in upper-case is canonical (for example,
  `"NEXT"`).

Tools that need to know the canonical string representation for an API level
must return an error if they need to get the canonical representation for a
reserved API level (that is, one greater than or equal to `0x80000000`) for
which they don't know the special name.

### Special API Levels

The following API levels are given special names:

- `PLATFORM = 0xFFF00000 = 4293918720`. `PLATFORM` plays the role previously
  provided by `LEGACY`, in that by default, the platform will be built at API
  level `PLATFORM`. Previously, `LEGACY` had the value `0xFFFFFFFFFFFFFFFF` in
  FIDL, and could not be represented in Clang.

  `LEGACY` was deprecated by [RFC-0232] and is in the process of being removed
  from FIDL, but even after that work is complete, the platform build, C++, and
  Rust code will use `PLATFORM` to detect a normal platform build.

  `PLATFORM` is only useful in code that is used both in the OS build and as
  part of the IDK. In such libraries, a target API level less than `PLATFORM`
  means that the code is being built as part of the SDK, and can only use API
  elements available in the specific API level the build is targeting. If the
  target API level _equals_ `PLATFORM`, the code is being built as part of the
  OS, and must provide support for _all_ API levels in the "supported" or
  "sunset" phase. See [RFC-0239] for more information.
- `HEAD = 0xFFE00000 = 4292870144`. Previously, `HEAD` had the value
  `0xFFFFFFFFFFFFFFFE` in FIDL and `0xFFFFFFFF` in Clang.
- `NEXT = 0xFFD00000 = 4291821568`. `NEXT` was described in [RFC-0239], but was
  not assigned a numerical value.

This set may grow or shrink over time, as necessary.

Initially, these values will be hard-coded into SDKs that support targeting
`HEAD` or `NEXT`, but eventually they should be defined in
[`//sdk/version_history.json`](/sdk/version_history.json).

### When to use integers vs strings

Command line tools accept input and produce output as strings, and as such, they
should accept any of the string representations for API levels described above,
and should preferentially output API levels using their canonical string form.
However, sometimes this is not possible or is very inconvenient, so use of
canonical string forms is not required. For example, Clang is not aware of the
names of special Fuchsia API levels, so the value of `-ffuchsia-api-level` must
be provided as an integer.

Within the implementation of build tools, storing an API level in integer form
is preferred.

Build systems may represent API levels in whichever way is most natural for that
build system.

## Performance

This change should have no impact on performance.

## Backwards Compatibility

The changes in the numerical values of `LEGACY` and `HEAD` are not
backwards-compatible, strictly speaking. However, the previous 64-bit values
were really only used within `fidlc` and were essentially invisible to users of
the Fuchsia SDK.

The current definition of `HEAD` as `0xFFFFFFFF` in C++ code is theoretically
visible to SDK users, but since no code outside of the Fuchsia source tree
currently targets `HEAD` or `LEGACY`, the change should also go unnoticed. LSC
presubmits will confirm this.

The new values of `PLATFORM` (previously, `LEGACY`), `HEAD`, and `NEXT` were
chosen to provide substantial gaps between them. This will allow us to create
additional special API levels without redefining the existing ones. Any such new
API levels we create should be added half-way between the two adjacent API
levels. In the worst case, we'll be able to subdivide each interval 20 times.

## Security considerations

This change should have no impact on security.

## Privacy considerations

This change should have no impact on privacy.

## Testing

This RFC primarily pertains to code in Fuchsia's build systems and SDKs. For
better or worse, dedicated automated testing of that code is minimal. However,
in practice, if the build system is broken, it will be noticed fairly quickly
when tests fail, build breaks, and local development flows go haywire.

## Documentation

The meaning and values of `NEXT`, `HEAD`, and `PLATFORM` will be included in
forthcoming documentation of the concepts introduced in RFC-0239.

## Drawbacks, alternatives, and unknowns

### Drawback: Is 32-bits enough?

If allocated sequentially, even if we issued a new API level every hour, it
would take around 245,000 years to run out of the 2<sup>31</sup> API levels set
aside in this RFC. That seems sufficient.

However, API levels need not always be allocated densely. This very RFC defines
special 3 API levels with 1048576 unused API levels between each. Is that okay?

With 64-bit API levels, it's difficult to imagine the possibility of running
out, even if we allocate them exceptionally sparsely (for example, by leaving
thousands, millions, or billions of gaps between consecutive versions, as Clang
does with `VersionTuple`).

With 32-bit API levels, we must be a little more judicious in our allocation.

Putting myself at risk of becoming a historical laughing stock, I'll say it:
2<sup>32</sup> API levels ought to be enough for anybody.

### Alternative: Switch to a Major.Minor scheme

`VersionTuple.h` was clearly written under the assumption that functionality
could be introduced or removed at versions that look like `12.5`, or even
`30.1.2.3`. Platforms that structure their versions this way can represent up to
2<sup>125</sup>[^clang-125] distinct versions. Perhaps the fact that Fuchsia can
only use 2<sup>32</sup> of these values indicates that Fuchsia's versioning
scheme is inappropriate?

There are multiple successful platforms that version their API surface with a
single integer (for example, Chromium and Android). There is insufficient reason
to believe that we cannot also be successful following the same strategy.

[^clang-125]: Clang uses the most significant bits of each of `minor`,
    `subminor`, and `build` as flags.

[RFC-0002]: /docs/contribute/governance/rfcs/0002_platform_versioning.md
[RFC-0083]: /docs/contribute/governance/rfcs/0083_fidl_versioning.md
[RFC-0232]: /docs/contribute/governance/rfcs/0232_fidl_bindings_for_multiple_api_levels.md
[RFC-0239]: /docs/contribute/governance/rfcs/0239_platform_versioning_in_practice.md
