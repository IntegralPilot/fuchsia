<!-- mdformat off(templates not supported) -->
{% set rfcid = "RFC-0258" %}
{% include "docs/contribute/governance/rfcs/_common/_rfc_header.md" %}
# {{ rfc.name }}: {{ rfc.title }}
{# Fuchsia RFCs use templates to display various fields from _rfcs.yaml. View the #}
{# fully rendered RFCs at https://fuchsia.dev/fuchsia-src/contribute/governance/rfcs #}
<!-- SET the `rfcid` VAR ABOVE. DO NOT EDIT ANYTHING ELSE ABOVE THIS LINE. -->

<!-- mdformat on -->

<!-- This should begin with an H2 element (for example, ## Summary).-->

## Summary

Following the process outlined in [RFC-0193: Supported C++ Versions][rfc-0193],
this RFC proposes updating from C++17 to C++20 as the Fuchsia tree C++ version
and expanding the set of formally supported C++ versions in the SDK to include
C++20.

## Motivation

C++17 is the only supported version in the Fuchsia platform and the SDK
according to [RFC-0193: Supported C++ Versions][rfc-0193]. C++20 introduced a
number of new features that significantly improve the ergonomics, but these
improvements are currently unavailable to Fuchsia platform developers.

## Stakeholders

_Facilitator:_ davemoore@google.com

_Reviewers:_

* ianloic@google.com
* jamesr@google.com
* mcgrathr@google.com

_Socialization:_ Discussed among the Fuchsia C++ users and the toolchain team.

## Requirements

* Update the Fuchsia tree C++ version from C++17 to C++20.
* Allow in-tree platform code to rely on C++20 features.

### Non-goals

* Make every feature in the C++20 standard immediately available in Fuchsia.

## Design

The Fuchsia tree can be already compiled in either C++17 or C++20 modes. This
will change the C++ version used by the Fuchsia tree from C++17 to C++20 after
which it will no longer be an option to build in-tree (or "platform") code in
the C++17 mode.

## Implementation

The C++ standard used by the Fuchsia tree is controlled by a build argument. We
plan to submit a [change][cl] that updates the Fuchsia tree C++ version from
C++17 to C++20, but ask Fuchsia developers to avoid using C++20 features until
the RFC is approved, to preserve the ability to roll back to C++17 if needed.

There are existing CI/CQ builders that build Fuchsia tree in the C++20 mode.
These will be eventually turned down and the freed up resources will be reused
for testing future C++ versions (such as C++23).

## Performance

C++20 expanded the use of compile time evaluation, including marking
constructors for many of the standard containers as `constexpr`, which MAY
improve runtime performance in certain cases, but have not observed any
significant difference so far.

## Ergonomics

Access to all C++20 standard library features&mdash;beyond what is already
available in the [stdcompat][stdcompat] library&mdash;will reduce boilerplate
and improve the C++ language usability.

Language features such as concepts or coroutines can improve ergonomics even
further but their usage within the Fuchsia tree is outside of the scope for
this RFC and SHOULD be covered by a future RFC.

## Backwards Compatibility

C++17 MUST remain supported in the SDK. The in-tree (or "platform") code which
is not part of the SDK API surface can start relying on C++20 features and thus
become incompatible with C++17.

## Security considerations

N/A

## Privacy considerations

N/A

## Testing

We have a basic [SDK test][cpp_variants] that tries to compile all C++ headers
in the SDK using C++17 and C++20 to [ensure][test-cl] we do not introduce code
incompatible with either version, but this test will not catch issues in the
C++ source files.

To improve the SDK test coverage, we SHOULD compile tests for every package
included in the SDK using C++17 and C++20 as was already done for
[stdcompat][stdcompat-test].

## Documentation

The [SDK documentation][sdk-cxx-docs] will be updated to reflect that C++20 is
officially supported in addition to C++17; we will also document the
constraints on C++20 feature support available to the SDK users.

[C++ in Zircon][zircon-cxx-docs] will be updated to cover supported C++20
features and constraints on their use.

## Drawbacks, alternatives, and unknowns

While we have been trying to provide a subset of the C++20 standard library
functionality through the [stdcompat][stdcompat] library, implementing
additional features would require an outsized investment and in some cases
would be impossible without the C++20 language support.

## Prior art and references

Google and Chromium already [target C++20][cppguide]. The SDK already supports
C++20 and partners like Google and Chromium rely on C++20 features, even though
it has not been documented as formally supported.

[rfc-0193]: /docs/contribute/governance/rfcs/0193_supported_c++_versions.md
[iso-cpp]: https://isocpp.org/std/the-standard
[sdk-cxx-docs]: /docs/development/idk/documentation/compilation.md
[zircon-cxx-docs]: /docs/development/languages/c-cpp/cxx.md
[cl]: https://fxrev.dev/1093351
[stdcompat]: https://cs.opensource.google/fuchsia/fuchsia/+/main:sdk/lib/stdcompat/;drc=4b39193a714dcf9302814877dc900f7548102083
[stdcompat-test]: https://cs.opensource.google/fuchsia/fuchsia/+/main:sdk/lib/stdcompat/test/BUILD.gn;l=105;drc=8fdb1718dedced8a757b467d070cb0879b1ab948
[cppguide]: https://google.github.io/styleguide/cppguide.html#C++_Version
[cpp_variants]: https://cs.opensource.google/fuchsia/fuchsia/+/main:build/bazel_sdk/tests/fuchsia/cpp_variants/;drc=447e9827598cecaf325359f7e0f79228380f1fb7
[test-cl]: https://fxrev.dev/939203
