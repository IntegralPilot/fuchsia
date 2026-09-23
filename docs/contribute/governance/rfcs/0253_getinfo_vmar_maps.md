<!-- Generated with `fx rfc` -->
<!-- mdformat off(templates not supported) -->
{% set rfcid = "RFC-0253" %}
{% include "docs/contribute/governance/rfcs/_common/_rfc_header.md" %}
# {{ rfc.name }}: {{ rfc.title }}
{# Fuchsia RFCs use templates to display various fields from _rfcs.yaml. View the #}
{# fully rendered RFCs at https://fuchsia.dev/fuchsia-src/contribute/governance/rfcs #}
<!-- SET the `rfcid` VAR ABOVE. DO NOT EDIT ANYTHING ELSE ABOVE THIS LINE. -->

<!-- mdformat on -->

<!-- This should begin with an H2 element (for example, ## Summary).-->

## Problem Statement

`ZX_INFO_PROCESS_MAPS` is inefficient when callers only care about one VMAR.

`ZX_INFO_PROCESS_MAPS` iterates a process's address space, providing the caller
with `zx_info_maps_t` structs that describe every address region and mapping
available to the process.  If a caller wants only a subset of this information,
e.g. just the contents of one specific VMAR, it must pay the full price anyway.
What's worse is that the caller must then filter the results.

## Summary

We propose adding a new `object_info` topic, `ZX_INFO_VMAR_MAPS`, that behaves
like `ZX_INFO_PROCESS_MAPS`, but for VMARs.

## Stakeholders

Who has a stake in whether this RFC is accepted? (This section is optional but
encouraged.)

_Facilitator:_

_Reviewers:_ adamperry@ (Starnix), adanis@ (Zircon VM), dworsham@ (Zircon VM),
mcgrathr@ (Zircon)

_Socialization:_ The proposal was briefly discussed with adamperry@ and adanis@
prior to drafting this RFC.

## Requirements

The purpose of this proposal is to improve Starnix performance by allowing it to
query a particular VMAR (the Linux process address space, a.k.a. the restricted
region) rather than the entire process's address space.

**Efficiency** - The new syscall should perform work proportional to the
size/complexity of the specified VMAR.

**Symmetry and Ergonomics** - The new call should produce results that are
structurally similar to those produced by the existing `ZX_INFO_PROCESS_MAPS` in
order to minimize work on existing callers that wish to use the new call.

## Design

We will add a new `object_info` topic named `ZX_INFO_VMAR_MAPS`.  This new topic
will behave like the existing `ZX_INFO_PROCESS_MAPS`, but instead of returning
results for the specified process, it will return results for the specified
VMAR.

Like `ZX_INFO_PROCESS_MAPS`, the result will be a depth-first pre-order
traversal.

The first result record will describe the specified VMAR.

Unlike `ZX_INFO_PROCESS_MAPS`, the depth fields of the results will be relative
to the specified VMAR rather than the specified process.  The depth of the first
result record will be 0.

To illustrate, the updated docs for `object_get_info` will say something like,

```
If *topic* is `ZX_INFO_VMAR_MAPS`, *handle* must be of type `ZX_OBJ_TYPE_VMAR`
and have `ZX_RIGHT_INSPECT`.
```

and

```
### ZX_INFO_VMAR_MAPS

*handle* type: `VM Address Region`, with `ZX_RIGHT_INSPECT`

*buffer* type: `zx_info_maps_t[n]`

The `zx_info_maps_t` array is a depth-first pre-order walk of the target
VMAR tree. As per the pre-order traversal base addresses will be in ascending
order.

See `ZX_INFO_PROCESS_MAPS` for a description of `zx_info_maps_t`.

The first `zx_info_maps_t` will describe the queried VMAR. The *depth*
field of each entry describes its relationship to the nodes that come
before it. The queried VMAR will have depth 0. All other entries have
depth 1 or greater.

Additional errors:

*   `ZX_ERR_ACCESS_DENIED`: If the appropriate rights are missing.
*   `ZX_ERR_BAD_STATE`: If the target process containing the VMAR has
    terminated, or if the address space containing the VMAR has been
    destroyed.
```

## Implementation

The new feature will be implemented with a series of small, backwards compatible
changes that include updates to tests and documentation.  Some existing code
will be refactored so that it can be reused for both `ZX_INFO_PROCESS_MAPS` and
`ZX_INFO_VMAR_MAPS`.

## Performance

No impact to existing code/systems.  Once complete, callers that are migrated
from `ZX_INFO_PROCESS_MAPS` to `ZX_INFO_VMAR_MAPS` should see a performance
improvement from the reduced work performed by the syscall as well as the
elimination of any post-call filter step.

## Ergonomics

For easy of use, the result types and semantics will closely match those of an
existing call, `object_get_info` with `ZX_INFO_PROCESS_MAPS`.

## Backwards Compatibility

The proposed change is backwards compatible.  No existing calls are impacted.
The new call will behave similar to an existing call.

## Security considerations

Because the returned information is a subset of what's returned by
`ZX_INFO_PROCESS_MAPS`, and because the new `object_get_info` call will require
the same handle rights required of the existing `ZX_INFO_PROCESS_MAPS` call,
there is no change to the security surface area or posture.

## Privacy considerations

There are no privacy concerns.

## Testing

The core-tests suite will be updated to test the new call.

## Documentation

The syscall docs will be updated to describe the new call.

## Drawbacks, alternatives, and unknowns

Implementing this proposal requires only a small amount of engineering effort.
Future maintenance of the new syscall should be minimal.  Further more, the new
call does not significantly reduce (kernel implementation) flexibility or (API)
freedom.

The primary alternative to implementing a new topic is to continue to use the
less efficient `ZX_INFO_PROCESS_MAPS` and continue to filter the results.

### Depth relative to target vs. address space ###

Under this proposal the depth field reported for each visited node is relative
to the target, the specified VMAR, starting at zero.

An alternative is to report the depth relative to the process's address space.
That is, report an absolute depth.  Both options meet the requirements and
perform similarly against the stated goals.

We selected depth relative to target because it exposes less information to
callers and because it allow for more implementation flexibility when it comes
to internal lock strategies.

## Prior art and references

See also [`ZX_INFO_PROCESS_MAPS`] and [`ZX_INFO_VMAR`].

[`ZX_INFO_PROCESS_MAPS`]: https://fuchsia.dev/reference/syscalls/object_get_info#zx_info_process_maps

[`ZX_INFO_VMAR`]: https://fuchsia.dev/reference/syscalls/object_get_info#zx_info_vmar
