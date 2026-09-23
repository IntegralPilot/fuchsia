<!-- Generated with `fx rfc` -->
<!-- mdformat off(templates not supported) -->
{% set rfcid = "RFC-0283" %}
{% include "docs/contribute/governance/rfcs/_common/_rfc_header.md" %}
# {{ rfc.name }}: {{ rfc.title }}
{# Fuchsia RFCs use templates to display various fields from _rfcs.yaml. View the #}
{# fully rendered RFCs at https://fuchsia.dev/fuchsia-src/contribute/governance/rfcs #}
<!-- SET the `rfcid` VAR ABOVE. DO NOT EDIT ANYTHING ELSE ABOVE THIS LINE. -->

<!-- mdformat on -->

## Summary

Following the process outlined in [RFC-0193: Supported C++ Versions][rfc-0193],
this RFC proposes updating from C++20 to C++23 as the Fuchsia tree C++ version
and expanding the set of formally supported C++ versions in the SDK to include
C++23.

## Motivation

C++20 is the only supported version in the Fuchsia platform, while the SDK
supports both C++17 and C++20 according to
[RFC-0193: Supported C++ Versions][rfc-0193] and
[RFC-0258: Update from C++17 to C++20][rfc-0258]. C++23 introduces a number of
new features that significantly improve the ergonomics, but these improvements
are currently unavailable to Fuchsia platform developers.

## Stakeholders

_Facilitator:_

* leannogasawara@google.com

_Reviewers:_

* mcgrathr@google.com
* phosek@google.com

_Consulted:_

_Socialization:_

## Requirements

* Update the Fuchsia tree C++ version from C++20 to C++23.
* Allow in-tree platform code to rely on C++23 features.

### Non-goals

* Make every feature in the C++23 standard immediately available in Fuchsia.

## Design

The Fuchsia tree can be already compiled in either C++20 or C++23 modes. This
will change the C++ version used by the Fuchsia tree from C++20 to C++23 after
which it will no longer be an option to build in-tree (or "platform") code in
the C++20 mode.

## Implementation

The C++ standard used by the Fuchsia tree is controlled by a build argument. We
plan to submit a change that updates the Fuchsia tree C++ version from
C++20 to C++23, but ask Fuchsia developers to avoid using C++23 features until
the RFC is approved, to preserve the ability to roll back to C++20 if needed.

## Performance

N/A

## Ergonomics

Access to all C++23 standard library features will reduce boilerplate
and improve the C++ language usability.

## Backwards Compatibility

Both C++17 and C++20 MUST remain supported in the SDK. The in-tree (or
"platform") code which is not part of the SDK API surface can start relying on
C++23 features and thus become incompatible with C++17 and C++20.

## Security considerations

N/A

## Privacy considerations

N/A

## Testing

In accordance with [RFC-0193][rfc-0193], the SDK tests will be built and run in
every supported C++ version, now including C++23.

## Documentation

The [SDK documentation][sdk-cxx-docs] will be updated to reflect that C++23 is
officially supported in addition to C++17 and C++20; we will also document the
constraints on C++23 feature support available to the SDK users.

[C++ in Zircon][zircon-cxx-docs] will be updated to cover supported C++23
features and constraints on their use.

## Drawbacks, alternatives, and unknowns

## Prior art and references

[rfc-0193]: /docs/contribute/governance/rfcs/0193_supported_c++_versions.md
[rfc-0258]: /docs/contribute/governance/rfcs/0258_update_from_c++17_to_c++20.md
[iso-cpp]: https://isocpp.org/std/the-standard
[sdk-cxx-docs]: /docs/development/idk/documentation/compilation.md
[zircon-cxx-docs]: /docs/development/languages/c-cpp/cxx.md
