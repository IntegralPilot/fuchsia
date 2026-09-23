<!-- mdformat off(templates not supported) -->
{% set rfcid = "RFC-0273" %}
{% include "docs/contribute/governance/rfcs/_common/_rfc_header.md" %}
# {{ rfc.name }}: {{ rfc.title }}
{# Fuchsia RFCs use templates to display various fields from _rfcs.yaml. View the #}
{# fully rendered RFCs at https://fuchsia.dev/fuchsia-src/contribute/governance/rfcs #}
<!-- SET the `rfcid` VAR ABOVE. DO NOT EDIT ANYTHING ELSE ABOVE THIS LINE. -->

<!-- mdformat on -->

<!-- This should begin with an H2 element (for example, ## Summary).-->

## Problem Statement

Provide a mechanism for querying information about a handle that is clearly
separate from informational queries about the object referred to by a handle.

## Summary

Introduce a pair of call to support the `ZX_INFO_HANDLE_VALID` and
`ZX_INFO_HANDLE_BASIC` topics of `object_get_info`.

## Stakeholders

Zircon Kernel team.

_Facilitator:_

cpu@google.com

_Reviewers:_

* abarth@google.com
* maniscalco@google.com
* mcgrathr@google.com


_Consulted:_

Zircon Kernel team.

_Socialization:_

Discussed in the Zircon internal chat room.

## Requirements

A significant amount of existing code calls `object_get_info` with the
`ZX_INFO_HANDLE_BASIC` topic to query the KOID for diagnostic purposes. We need
to support this code with care.

## Design

Introduce a pair of new `handle_` centric calls replacing use of the handle
topics in `object_get_info`. These calls have the same behavior as the
`object_get_info` queries they replace with a modernized API design.

### Handle validity testing

`handle_check_valid` accepts a single `zx_handle_t` parameter and returns ZX_OK
if the handle is valid and an error otherwise. This is the replacement for
calling `object_get_info` with a topic of `ZX_INFO_HANDLE_VALID`. The exact
error conditions and codes will be decided during API review with the following
goals:
- Generate unique errors for statically invalid bit patterns such as
  `ZX_HANDLE_INVALID` and having one of the two least significant bits cleared.
- Avoid use of `ZX_ERR_BAD_HANDLE` to reserve this code for situations that
  would trigger the bad handle policy.

### Handle basic info

`handle_get_basic_info` populates a `zx_handle_basic_info_t` structure with
information about a handle. This is the replacement for calling
`object_get_info` with a topic of `ZX_INFO_HANDLE_BASIC`.

### object_get_info

This proposal does not include modifying the existing `object_get_info` call.
It will continue to support the handle-specific topics. We may want to encourage
new code to use the handle-specific entry point for edification.

## Implementation

These calls can be implemented initially as vdsocalls using the existing
`object_get_info` system call.

## Performance

This adds one more entry point to the vDSO that requires relocation on program
load. This is expected to be negligible.

## Ergonomics

This provides additional clarity on the separation of the "handle" and "object"
concepts in the Zircon API.

## Backwards Compatibility

This API will be added following the procedures defined in RFC-0239. The
existing functionality of `object_get_info` will not be changed at this time.
In the future, we may deprecate and eventually remove support for the handle
specific topics from `object_get_info` once sufficient time has passed to update
all callers and libraries using this functionality to these new calls.

## Future work

Once all existing code has been updated to use the new calls for the
handle-related topics we could update `object_get_info` to return an error
for these topics.

## Security considerations

No security considerations are anticipated.

## Privacy considerations

No security considerations are anticipated.

## Testing

The Zircon core tests will be expanded to cover these new entry points as they
cover `object_get_info` today.

## Documentation

New system call documentation will be added for the new entry points and the
"Zircon handles" document will be updated to cite these entry points.

## Drawbacks, alternatives, and unknowns

An alternative is to do nothing and continue to use the object entry point to
query handle information.

## Prior art and references

The rest of the syscall API distinguishes between operations on handles and
objects in the name. For example, `handle_close` and `handle_duplicate` operate
on handles and `object_get_property` and `object_signal` operate on objects.
