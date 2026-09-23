# Churn Policy

Fuchsia is now a large project with several independent teams working hard to
meet the goals of various customers. At the same time, platforms like Fuchsia
must evolve in ways that require intermittent effort from many contributors
across the codebase, informally referred to as "churn".

This policy was ratified in [RFC-0214][rfc-0214].

## Goals

 1. Align Fuchsia engineering practices with the project's stated support and
    stability goals.
 2. Accurately estimate the time spent on migrations and other externally
    imposed changes.
 3. Clarify the boundary between RFC designs and FPS staffing decisions.
 4. Provide a collection of migration strategies for teams to consider.

## Non-goals

 1. Change the strategy for large scale changes that have already begun.
 2. Decide what changes should be made.
 3. Mandate a specific migration strategy for all changes.
 4. Reduce the rate of changes.
 5. Set policy for changes that require no effort from client teams.

## Policy

Informally, a change is considered "Fuchsia-wide" if it requires development
effort or workflow changes from other contributors. This includes changes that
impact the [ABI][fuchsia-abi], any public portion of the [SDK][fuchsia-sdk], the
contents of a [product][build-products], or generated code.

All Fuchsia-wide changes will incur an engineering cost for many Fuchsia
contributors. This policy centralizes those costs so that they can be minimized.
For the purposes of this policy, an "impacted team" is any team that must either
approve or make changes to their own code, workflows, or docs in order to
accommodate churn.

It is the responsibility of the contributor or team that initiated the change to
resolve any breakages. Most of the time, this will mean taking a "revert first,
diagnose second" approach.

### Impacted teams

If your team is impacted by an approved change:

 * Respond to incoming CL reviews or other changes within two business days.

 * Support the change author in arriving at a satisfactory conclusion, such as
   by approving their work, responding to surveys, clearly rejecting proposed
   changes, requesting specific modifications to the change, and/or answering
   questions from the change author about the subject matter.

 * If more than 10% of your time is spent responding to churn, you may flag this
   issue with eng-council@fuchsia.dev

 * If the change is in a style guide (e.g. lints, compiler warnings, etc.), you
   decide how quickly to resolve new style violations.

### Initiating teams

Without the churn policy, there are no formal requirements or expectations for
changes that create churn. This section adds responsibilities for the authors of
such changes.

If your team is initiating a change and will be doing 100% of the work:

 1. Send mail to the [FEC][fec] explaining the minimal impact to other teams.
 2. Send mail to announce@fuchsia.dev to notify contributors of the migration.
 2. Proceed with the migration.

If your team initiates a change that will require substantial effort from
others, including changes initiated by an RFC:

 1. Create a plan that demonstrates to the [FEC][fec] that your team will expend
    at least 80% of the manual effort that is not addressed by automation. Plans
    must include a list of the impacted teams and the estimated cost of the
    change.
 2. After the [FEC][fec] approves your plan, notify impacted teams. They must be
    able to schedule the work using quarterly planning over at least two
    quarters, so notify teams more than a week before the quarter begins.
 3. Proceed with the migration.

[build-products]: /docs/development/build/software_assembly/build_configuration.md#products
[fec]: /docs/contribute/governance/eng_council.md
[fuchsia-abi]: /docs/concepts/kernel/system.md
[fuchsia-sdk]: /docs/development/sdk/index.md
[rfc-0214]: /docs/contribute/governance/rfcs/0214_fuchsia_churn_policy.md
