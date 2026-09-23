<!-- Generated with `fx rfc` -->
<!-- mdformat off(templates not supported) -->
{% set rfcid = "RFC-0286" %}
{% include "docs/contribute/governance/rfcs/_common/_rfc_header.md" %}
# {{ rfc.name }}: {{ rfc.title }}
{# Fuchsia RFCs use templates to display various fields from _rfcs.yaml. View the #}
{# fully rendered RFCs at https://fuchsia.dev/fuchsia-src/contribute/governance/rfcs #}
<!-- SET the `rfcid` VAR ABOVE. DO NOT EDIT ANYTHING ELSE ABOVE THIS LINE. -->

<!-- mdformat on -->

## Problem Statement

FIDL includes `zx.Status` as a built-in primitive type representing Zircon
status codes (`zx_status_t`). Under
[RFC-0085](/docs/contribute/governance/rfcs/0085_reducing_zx_status_t_space.md),
standard Zircon status codes define `0` (`ZX_OK`) as success and negative
integers as error codes. In addition, applications and subsystems within Fuchsia
use positive 32-bit integers as application-defined status or epitaph codes.

Currently, none of the FIDL bindings validate that `zx.Status` contains a
non-zero status code on the wire. The new Rust bindings (`rust_next`) map
`zx.Status` (or method error types using `error zx.Status`) to an idiomatic
error type (`zx::Status`).

While Rust is working towards enforcing non-zero status invariants for
`zx::Status`, `zx.Status` in FIDL currently allows `0` (`ZX_OK`) on the wire in
all contexts. In method response results that map status codes to error types,
receiving `0` (`ZX_OK`) as an error payload is nonsensical, as it forces
generated code to represent `Err(ZX_OK)` - an error variant holding a success
status code.

## Summary

This RFC proposes two related changes to FIDL's handling of status codes:

1.  **Change `zx.Status` to Non-Zero Status Codes:** The existing `zx.Status`
    primitive type in FIDL is changed to represent non-zero status codes across
    all FIDL bindings. Sending `0` (`ZX_OK`) in a `zx.Status` field or payload
    is prohibited at encoding time and rejected as a decoding error. Positive
    and negative status codes remain valid.
2.  **Add `zx.Result` Primitive:** A new FIDL primitive type, `zx.Result`, is
    added to explicitly represent operations that return a Zircon status result
    (which may be `0` / `ZX_OK` for success or any non-zero status code).

## Stakeholders

*Facilitator:* abarth@google.com

*Authors:*

-   csuter@google.com

*Reviewers:*

-   dkoloski@google.com
-   hjfreyer@google.com

*Socialization:*

-   Discussed within the FIDL team.

## Requirements

1.  **Cross-Binding Uniformity:** All FIDL language bindings should enforce
    identical wire validation rules for `zx.Status` (rejecting `0`).
2.  **Ergonomic Bindings:** There has been an attempt in the new Rust bindings
    to use more ergonomic types (such as `zx::Status`), and we should keep that
    goal in mind for this RFC, though there is no requirement to make other
    existing bindings more ergonomic.

## Design

### FIDL Primitive Definitions

-   **`zx.Status` (Changed):** Defined as a constrained 32-bit signed integer
    representing a non-zero Zircon status code.

    -   Valid wire values: `n != 0` (negative error codes representing
        `ZX_ERR_*`, and positive application-defined error codes).
    -   Invalid wire values: `0` (`ZX_OK`).
    -   Encoders must prohibit sending `zx.Status == 0`. Decoders across all
        bindings must reject messages containing `zx.Status == 0`.

-   **`zx.Result` (Added):** Added as a primitive type representing a Zircon
    status result.

    -   Valid wire values: any 32-bit integer (`0` / `ZX_OK` for success,
        non-zero for error or status codes).
    -   Invalid wire values: none (all 32-bit signed integers are valid).
    -   Behavior: Behaves identically to the current `zx.Status` representation
        on the wire today, accepting `0` (`ZX_OK`) as success and non-zero
        values as errors.

### Support for Positive Status Codes

Zircon defines standard kernel error codes as negative integers (`ZX_ERR_*`).
However, certain subsystems and applications in Fuchsia use positive 32-bit
integers as application-defined error or exit codes. For example, component
manager uses positive error codes for component runner termination epitaphs
(such as `fuchsia.component/Error`).

Because positive status codes are load-bearing for application-specific
epitaphs and error reporting in existing protocols, this RFC leaves positive
status codes supported for both `zx.Status` (`n != 0`) and `zx.Result`.
Restricting status codes to strictly negative integers would require broader
cross-tree migration of application-specific epitaph patterns, and there is no
pressing need to restrict positive values at this time.

### Language Binding Mappings

-   **New Rust Bindings (`rust_next`):**
    -   `zx.Status` maps to `zx::Status` (non-zero status code).
    -   `zx.Result` maps to `Result<(), zx::Status>` (decoding `0` to `Ok(())`
        and non-zero `n` to `Err(zx::Status)`).
    -   FIDL method error syntax (`-> (...) error zx.Status`) maps error
        payloads to `zx::Status`.
-   **Existing Rust Bindings (`fidl`):**
    -   `zx.Status` maps to `i32` / `zx_status_t` (validating `n != 0` on
        decode).
    -   `zx.Result` maps to `Result<(), zx::Status>` (decoding `0` to `Ok(())`
        and non-zero `n` to `Err(zx::Status)`).
-   **C++ Bindings:**
    -   `zx.Status` maps to `zx_status_t` (validating `n != 0` on decode).
    -   `zx.Result` maps to `zx::result<>` (decoding `0` to `zx::ok()` and
        non-zero `n` to `zx::error(status)`). Note that unlike Rust,
        `zx::result<>` in C++ does not utilize a niche optimization, resulting
        in an extra 4 bytes of in-memory size compared to `zx_status_t`.
-   **Go Bindings:**
    -   `zx.Status` maps to `int32` (validating `n != 0` on decode).
    -   `zx.Result` maps to `int32`.
-   **Python Bindings (`fidlgen_python` and Fuchsia Controller):**
    -   `zx.Status` maps to `int` (validating `n != 0` on decode).
    -   `zx.Result` maps to `int`.
-   **Other Languages (e.g., Dart):**
    -   For language bindings that lack dedicated result or non-zero status
        abstractions (or out-of-tree bindings such as Dart), both types fall
        back to standard integer representations (`int` / `int32`).
    -   `zx.Status` maps to an integer type (validating `n != 0` on decode).
    -   `zx.Result` maps to an integer type.

## Implementation

1.  **FIDL Toolchain Support:**
    -   Add `zx.Result` as a recognized primitive type in `fidlc`.
    -   Add `fidlc` compiler lints to guide developers toward the correct type:
        -   Lint against using `zx.Status` in a struct, table, or union (where
            `zx.Result` is almost certainly intended).
        -   Lint against using `zx.Result` in method error syntax (`-> (...)
            error zx.Result`), where `zx.Status` should be used instead.
    -   Update the `abi-compat` tool to reason about compatibility between
        `zx.Status` and `zx.Result`.
    -   Add GIDL conformance tests verifying that `zx.Result` accepts both `0`
        and non-zero status codes.
    -   Implement `zx.Result` mapping in all binding generators (`fidlgen_*`),
        and update out-of-tree generators (such as `fidlgen_dart`).
2.  **Schema and Call Site Migration:**
    -   Migrate existing non-error `zx.Status` usages to `zx.Result` in-tree and
        across SDK levels for OOT uses.
3.  **Wire Validation Enforcement:**
    -   Update `fidlgen_rust_next`, `fidlgen_cpp`, `fidlgen_rust`, `fidlgen_go`,
        and `fidlgen_python` to enforce `zx.Status != 0` during encoding and
        decoding, and coordinate corresponding updates to out-of-tree bindings
        (such as `fidlgen_dart`).
    -   Add GIDL conformance tests verifying that decoding a message with
        `zx.Status == 0` fails across all language bindings.
4.  **Documentation:**
    -   Update FIDL language reference and wire format specifications for
        `zx.Status` and `zx.Result`.

### Migration Strategy

The migration will proceed in three distinct phases:

1.  **Introduce `zx.Result`:**
    -   Add `zx.Result` to the FIDL toolchain (`fidlc`, GIDL, and in-tree
        binding generators), and update out-of-tree generators (such as
        `fidlgen_dart`).
    -   Update the `abi-compat` tool to recognize migrations between
        `zx.Status` and `zx.Result` as ABI compatible.
    -   At this stage, `zx.Status` remains unchanged on the wire and continues
        to accept all status values.

2.  **Migrate Uses of `zx.Status` to `zx.Result`:**
    -   Migrate existing FIDL fields and method payloads that expect `0`
        (`ZX_OK`) or status results from `zx.Status` to `zx.Result`.
    -   Changing a FIDL field from `zx.Status` to `zx.Result` is a
        source-incompatible change, as bindings will generate structured result
        types (such as `Result<(), zx::Status>` in Rust or `zx::result<>` in
        C++) instead of raw integer status types.
    -   **Unstable APIs:** For unstable APIs, making atomic changes is
        supported since all usage is either in-tree or "at your own risk". We
        will migrate FIDL definitions and their call sites directly.
    -   **Stable APIs:** For stable APIs published in the SDK, we will use
        standard FIDL API versioning: we will introduce the change at the `NEXT`
        API level. Once the API level with the `zx.Result` change is frozen and
        downstream repositories migrate to it, we'll retire API levels
        referencing `zx.Status` and the migration will be complete.

3.  **Enforce Non-Zero Validation for `zx.Status`:**
    -   When we are certain there are no legacy uses of `zx.Status` expecting or
        sending `0` (`ZX_OK`), we will update all FIDL bindings (both in-tree
        and out-of-tree generators such as `fidlgen_dart`) to reject sending
        and receiving `0` for `zx.Status`.
    -   Encoders will prohibit sending `0`, and decoders will return a decoding
        error upon receiving `0` on the wire for `zx.Status`.

## Performance

Minimal performance impact. Decoding `zx.Status` requires a single non-zero
check (`raw != 0`).

## Ergonomics

Changing `zx.Status` to non-zero status codes allows generated method signatures
in Rust to remain clean and idiomatic:

```rust
// Ergonomic Result in new Rust bindings
fn my_method(&self) -> impl Future<Output = Result<Result<MyResponse, zx::Status>, FidlError>>;
```

For C++, `zx.Result` maps directly to `zx::result<>`. Other bindings without
dedicated result types (such as Go, Python, and Dart) use integer
representations.

To prevent API authors from accidentally using the wrong type, `fidlc` will
provide lints:

-   Warn on uses of `zx.Status` inside structs or tables (where fields usually
    represent operation results that may succeed with `ZX_OK` and should use
    `zx.Result`).
-   Warn on uses of `zx.Result` in method error syntax (`-> (...) error
    zx.Result`), where error payloads already represent a failure condition and
    should use `zx.Status`.

## Backwards Compatibility

-   **Source Compatibility:** Changing FIDL fields from `zx.Status` to
    `zx.Result` is source incompatible. Unstable APIs will be migrated directly,
    while stable APIs will be migrated across SDK API levels as described in the
    Migration Strategy.
-   **Wire/ABI Compatibility:** Changing `zx.Status` to reject `0` (`ZX_OK`) on
    the wire is an ABI breaking change, specifically the kind of breaking change
    that [RFC-0229: FIDL at API Level 2023][rfc-0229] commits to avoiding until
    after 2026-10-11. We will respect RFC-0229 (the rollout to downstream users
    across API level stabilization and retirement will take longer than that
    date anyway). Furthermore, as far as we know, no users ever send `Err(OK)`
    and we are willing to take on the risk that we are wrong about that.
    Enforcing wire validation only in Phase 3 after migrating all valid status
    result uses to `zx.Result` ensures no legitimate communications are broken.

## Security considerations

Enforcing non-zero status validation at the decoder boundary prevents unexpected
logic errors caused by processing an error payload that contains `ZX_OK`.

## Privacy considerations

No privacy implications.

## Testing

Comprehensive unit tests and GIDL conformance tests will be added across all
language bindings to verify:

-   `zx.Status` decodes correctly for non-zero values (both negative and
    positive) and fails for `0`.
-   `zx.Result` decodes correctly for `0`, positive, and negative values.

## Documentation

-   Update FIDL wire format specification for `zx.Status` and `zx.Result`.
-   Update FIDL language reference.

## Drawbacks, alternatives, and unknowns

### Alternatives Considered

1.  **Introduce `zx.Error` and leave `zx.Status` unchanged**

    -   *Description:* Introduce a new `zx.Error` type that rejects `0`
        (`ZX_OK`), intended for use in error contexts (e.g. `... error
        zx.Error`). `zx.Status` would remain unchanged, continuing to allow `0`.
    -   *Why Dismissed:* Using `zx.Status` in error contexts (`error zx.Status`)
        is far more common than using status codes in non-error contexts. With
        `zx.Error`, developers would frequently write `error zx.Status` by
        habit, forgetting to use `zx.Error`. Changing `zx.Status` to reject `0`
        aligns with developer intuition for Rust `zx::Status`.

2.  **Context-aware FIDL compiler validation**

    -   *Description:* Make the FIDL compiler context-aware such that
        `zx.Status` disallows `0` (`ZX_OK`) on the wire when used in an error
        context (`... error zx.Status`), but allows `0` when used as a field in
        a struct or table.
    -   *Why Dismissed:* Violates FIDL's canonical representation design
        principle. The valid value domain of a FIDL primitive type should be
        consistent regardless of where it appears in a schema declaration.

3.  **Generic FIDL Result syntax**

    -   *Description:* Extend FIDL syntax to support generic result types
        directly in protocol method returns (e.g., `flexible Frobinate() ->
        zx.Result<()>`).
    -   *Why Dismissed:* This represents a larger change to FIDL language
        grammar and syntax, expanding scope beyond resolving status code zero
        representation.

4.  **Treat `Err(ZX_OK)` as a framework-level error in `rust_next` only**

    -   *Description:* Keep `zx.Status` unchanged in FIDL and on the wire. In
        `rust_next`, translate received `0` in an error payload into a
        framework-level error (e.g. `Err(FidlError::OkError)`), while leaving
        other bindings unchanged.
    -   *Why Dismissed:* Violates cross-binding uniformity. Having `rust_next`
        reject messages that other bindings accept creates cross-language
        incompatibilities and complicates conformance testing.

## Prior art and references

-   [RFC-0085: Reducing the zx_status_t Space](/docs/contribute/governance/rfcs/0085_reducing_zx_status_t_space.md)
-   [RFC-0229: FIDL at API Level 2023][rfc-0229]
-   [FIDL Canonical Representation Design Principle][fidl-canonical-principle]

[fidl-canonical-principle]: /docs/contribute/contributing-to-fidl/design-principles.md#canonical-representation
[rfc-0229]: /docs/contribute/governance/rfcs/0229_fidl_2023.md
