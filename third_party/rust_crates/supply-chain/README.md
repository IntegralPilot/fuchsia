# Using `cargo-vet` on Fuchsia

## Criteria

Fuchsia vets its dependencies according to two main sets of criteria:

### `ub-risk`

Many third-party crates include `unsafe` code and therefore introduce potential
for undefined behavior. The potential for undefined behavior doesn't change from
crate to crate, but we do allow some crates with a higher risk of UB and deny
others with a lower risk of UB. This is because our standards and criteria for
what constitutes UB risk are constant but what we consider acceptable varies.

UB risk is graded on a scale from `0` (no unsafe code) to `4` (high UB risk). By
default, all third-party crates on Fuchsia must be `ub-risk-2`, also called the
"average good crate" standard. When we want to use third-party crates that pose
a greater risk of UB, we make specific policy exceptions for them and leave a
note explaining our decision.

See `audits.toml` for a complete description of each `ub-risk` level.

### `safe-to-run` and `safe-to-deploy`

Separately, crates are also rated as either `safe-to-run` or `safe-to-deploy`.
`safe-to-run` crates can be thought of as "not actively malicious" - they won't
exfiltrate sensitive data or otherwise pose a risk when running on a development
machine. `safe-to-deploy` crates must meet a much higher standard - they won't
introduce a serious security vulnerability to production software. These crates
typically require a more detailed logical review, and also require at least some
unsafe code review.

By default, all third-party crates on Fuchsia must be `safe-to-deploy`. In
general, we prefer to keep crates audited to this standard. However, for crates
that are only used in host-side tooling (i.e. dependencies listed under
`[target.'cfg(not(target_os = "fuchsia")'.dependencies]`), we may allow crates
audited only to `safe-to-run`.

## Policies

Configuring the policy for a third-party crate is relatively straightforward:

- Crates that are included on both hosts (non-`fuchsia` targets) and targets
  (`fuchsia` targets) must be `safe-to-deploy`.
- Host-only crates may be relaxed to `safe-to-run` as long as it is reasonable
  to do so.
- By default, all crates must be `ub-risk-2` or lower. There are a few notable
  exceptions to this rule:
  - Crates that are critical to platform security (e.g. cryptography, crates
    that accept untrusted input and run in a privileged environment) must be
    `ub-risk-1` or lower.
  - Crate that are more dangerous (`ub-risk-3` and `ub-risk-4`) may be used with
    permission. See `audits.toml` for more details on what may qualify a
    high-risk crate for use on Fuchsia.

When exceptions to the default policy are made, the corresponding `[policy]`
section must include a note detailing the circumstances which justify a policy
variance.

### Policy variances

We'd prefer people don't use crates that are UB-RISK-3 or higher. We should push
users to look for alternatives, and if we do allow these crates to be added then
they should be restricted use if possible.

Crates that are UB-RISK-4 pose a very high risk of introducing undefined
behavior. Because these crates may be difficult even for the most experienced
domain experts, uses of these crates should be limited. UB-RISK-4 crates that
may be acceptable for use in Fuchsia include:

- Raw FFI bindings that require domain-specific expertise and have little to no
  Rust safety documentation (e.g. `ash` and `libc`).

## Audits

Code reviews may audit the `safe-to-run` and `safe-to-deploy` criteria. Any
trusted Fuchsia developer may audit crates as `safe-to-run` or `safe-to-deploy`.

Independently, unsafe code reviews may audit the `ub-risk-*` criteria. Because
unsafe code reviews require special expertise, only a member of Fuchsia's
unsafe reviewers may provide the `ub-risk-*` audits. As an exception, any crate
that does not use contain unsafe code may be audited as `ub-risk-0` by a
trusted Fuchsia developer.

### Recording violations

`cargo vet` allows us to record not only whether a crate fulfills some criteria,
but also whether it explicitly fails it. This violation recording overrides
exemptions, so as soon as Fuchsia or a project that we federate with records a
violation for a crate, it will prevent the crate from being used
inappropriately.

For binary criteria like `safe-to-run` and `safe-to-deploy`, we record
violations when doing reviews. However, for continuous criteria like `ub-risk-*`
we only record a violation if a crate doesn't fulfill the most permissive
criteria. So if a crate is `ub-risk-3`, we say that it fulfills `ub-risk-3` but
do not record that it violates `ub-risk-2`. If a crate does not fulfill
`ub-risk-4`, then we record that it violates `ub-risk-4`.

This does mean that as long as we have exemptions, we may be using crates that
don't meet our auditing policy. That's okay though, since we can evaluate each
case individually to determine how to proceed appropriately.

### Performing `ub-risk-*` audits

Many of the `ub-risk-*` criteria allow room for interpretation by individual
unsafe code reviewers. It's not possible to document every possible decision
that a reviewer may have to make, we rely on their discretion to reach the right
conclusion.

Here are a few situations which we have guidance for. They can inform how to
evaluate a novel situation.

#### Undocumented language or compiler invariants

Undocumented language and compiler invariants are relatively common, and some
pose a greater risk than others of causing undefined behavior.

Any code that relies on undocumented invariants must be UB-RISK-2 or higher.
UB-RISK-1 code must be exceptionally well-documented and justified.

Invariants that are widely-used and could not be broken without a large impact
on the community are acceptable in UB-RISK-2 crates. For example, creating a
typed reference to uninitialized memory is UB-RISK-2 as long as the value behind
the reference is not read from. Note that a write through the reference could
call drop code that reads from that uninitialized memory.

Invariants that are actively discouraged but unlikely to be changed without a
large impact are acceptable in UB-RISK-3 crates. For example, pointer provenance
violations (e.g. roundtripping a pointer through an integral type) are
UB-RISK-3 because they are discouraged by the language and are detected by MIRI.

Most invariants that are unacceptable for UB-RISK-3 crates can be considered to
be unsound assumptions. However, UB-RISK-4 crates may rely on other undocumented
invariants at the discretion of the reviewer. If possible, these invariants
should be checked at compile time or before use at run time.
