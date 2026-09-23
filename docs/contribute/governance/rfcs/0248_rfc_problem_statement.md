<!-- mdformat off(templates not supported) -->
{% set rfcid = "RFC-0248" %}
{% include "docs/contribute/governance/rfcs/_common/_rfc_header.md" %}

# {{ rfc.name }}: {{ rfc.title }}

{# Fuchsia RFCs use templates to display various fields from _rfcs.yaml. View the #}
{# fully rendered RFCs at https://fuchsia.dev/fuchsia-src/contribute/governance/rfcs #}
<!-- SET the `rfcid` VAR ABOVE. DO NOT EDIT ANYTHING ELSE ABOVE THIS LINE. -->

<!-- mdformat on -->

<!-- This should begin with an H2 element (for example, ## Summary).-->

## Problem Statement

Fuchsia RFCs sometimes "stall out" and take a long time to converge, or suffer
from disagreement about what problem is being solved. There's a perception from
some subteams that the RFC process is very slow. This can take a number of
forms, but a common anti-pattern is disagreement late in the process about what
problem an RFC is actually solving. RFCs can also stall out when it takes a long
time to identify stakeholders, or when CL reviewers are slow to provide
feedback. We should provide a streamlined process for time-sensitive RFCs and a
path to escalate to the [Fuchsia Engineering Council (FEC)][FEC] when needed.

## Summary

This RFC makes several changes to the RFC process text. These include:

* Clarifies the importance of agreeing on a problem.
* Adds the option to reach out to FEC for a facilitator after identifying a
  problem but before settling on a specific solution.
* Clarifies places where authors may choose to start a prototype or leave the
  RFC process.

## Stakeholders

_Facilitator:_

abarth@google.com

_Reviewers:_

FEC members: <abarth@google.com>, <cpu@google.com>
RFC authors: <harshanv@google.com>, <wittrock@google.com>

_Consulted:_

RFC authors: TODO, identify a few folks familiar with the process

_Socialization:_

This RFC was discussed in multiple FEC meetings and in a number of
information-gathering discussions with RFC authors.

## Requirements

* The RFC process should "fail fast" and help authors avoid investing a lot of
  time in the wrong problem.
* The RFC process should have a clear path to escalate when timely feedback is
  required.
* The RFC process should avoid putting undue process burden on authors.
* The RFC process should encourage authors to test out their ideas and "learn by
  doing".

## Design

See RFC template changes. The proposed RFC process phases are:

* Step 1: Identify the problem
* Step 2: Problem statement
* Step 3: Assign facilitator
* Step 4: Stakeholder discovery
* Step 5: Socialization
* Step 6: Draft and iteration
* Step 7: Last call
* Step 8: Submit

For many RFCs the process will look quite similar to before, as authors are
already writing up problem statements within their RFC. However, frontloading
the problem statement and faciliator assignment allows authors to get help from
the FEC early to identify and get feedback from stakeholders. The engineering
council can act as a point of escalation for any issues moving through the
process.

Authors are encouraged to prototype their ideas throughout the process, and
especially before doing a full writeup. This will help prove out ideas and
discover additional constraints early on and gives author the opportunity to
adjust before a lot of time is invested. Additionally, prototypes decrease the
chance that the RFC will require later amendments.

## Implementation

Check in the CL about RFC process changes, and give a tech talk to the Fuchsia
team about how to navigate the process.

## Ergonomics

While this change does introduce additional steps in the RFC process, these
steps are typically fairly quick. An author who already has a written-up RFC may
choose to "speed run" the early phases of the process in a single day so long as
they are confident that their problem statement and solution will not prove
controversial with stakeholders.

In the case where one of these steps takes longer (for example, the author
identifies a problem but gets feedback that this problem isn't framed
correctly), the additional delay adds value by allowing the author to
course-correct before investing time in a full writeup of their design.

## Backwards Compatibility

Older RFCs do not need to be updated.

## Security and privacy considerations

By pushing stakeholder identification earlier in the process, this change should
encourage earlier involvement from security and privacy teams when relevant.

## Documentation

All relevant documentation changes are included in the RFC CL. These change the
RFC process and the RFC template.

## Drawbacks, alternatives, and unknowns

We could leave the RFC process as-is. The current issues with the process would
remain in place, but could be mitigated by vigilant FEC involvement.

[FEC]: /docs/contribute/governance/eng_council.md
