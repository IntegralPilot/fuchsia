# Fuchsia RFC Process

The Fuchsia RFC process has evolved from the following RFCs:

* [RFC-0001: Fuchsia Request for Comments process](0001_rfc_process.md)
* [RFC-0006: Addendum of the RFC process for Zircon](0006_addendum_to_rfc_process_for_zircon.md)
* [RFC-0067: Additions to Fuchsia RFC process](0067_rfc_process_additions.md)
* [RFC-0017: The FTP Process is dead, long live the RFC Process!](0017_folding_ftp_into_rfc.md)
* [RFC-0122: RFC Stakeholders](0122_stakeholders.md)
* [RFC-0248: RFC Problem Statement](0248_rfc_problem_statement.md)

This page collates the above RFCs and captures the current process.

After reviewing this process, you can follow
[this guide to create an RFC][create].

[TOC]

## Summary

The Fuchsia RFC process is intended to provide a consistent and transparent path
for making project-wide, technical decisions. For example, the RFC process can
be used to evolve the project roadmap and the system architecture.

## Motivation

The RFC process establishes a consistent and transparent path for making
project-wide technical decisions and ensuring that the appropriate stakeholders
are involved in each decision.

## Design

This section describes the design of the RFC process.

### When to use the process {#when-to-use-the-process}

The RFC process can be used for any change to Fuchsia that would benefit from
its structured approach to decision making, escalation help from the [Eng
Council](../eng_council.md) and/or its durable record of the decision.

The vast majority of changes do not _require_ an RFC. Instead, these changes can
be made using the [code review
process](/docs/development/source_code/contribute_changes.md). Contributors
should feel free to use the RFC process if they find it useful, and may stop at
any point if there's agreement that the process is not required or does not add
value in a particular case. However, technical decisions that have broad impact
across the project require broader agreement and _must_ be socialized with the
project using the RFC process.

The following kinds of changes must use the RFC process:

* _Adding constraints on future development._ Some decisions, once made,
   constrain the future development of the system. We need to be careful when
   making such decisions because they can be difficult to revise later.

* _Making project policy._ Project policies have broad impact across the
   system, often affecting contributors throughout the project. Examples
   include: changing the set of supported languages (impacts everyone who needs
   to debug and understand the system), deprecating a widely-used API, and
   changing testing requirements for a broad class of code changes.

* _Changing the system architecture._ The system architecture describes how the
   system fits together as a whole. Changing the system architecture, by
   definition, crosses boundaries between subsystems and requires careful
   consultation with many stakeholders.

* _Delegating decision-making authority._ There are often classes of decisions
   that the project needs to make frequently and that benefit from specialized
   expertise. Rather than making all these decisions through the RFC process,
   the project can delegate decision-making authority for those classes of
   decisions to another group or process. For example, we often need to make
   decisions about platform APIs, which add constraints on future development,
   but it would not be practical to use the RFC process for every change to the
   platform API.

* _Escalations._ Finally, contentious changes can benefit from the transparency
   and clarity of the RFC process. If there is a disagreement about technical
   direction that cannot be resolved by an individual technical leader, the
   decision can be escalated to the RFC process either by one of the disagreeing
   parties or by another contributor.

In addition to the general considerations outlined above, some areas declare
additional criteria. Please consult these documents when relevant:

| Area                | Criteria RFC |
|---------------------|--------------|
| Component Framework | [RFC-0098](0098_component_framework_rfc_criteria.md)|
| FIDL                | [RFC-0049](0049_fidl_tuning_process_evolution.md) |
| Software Delivery   | [RFC-0103](0103_software_delivery_rfc_criteria.md) |
| Zircon              | [RFC-0006](0006_addendum_to_rfc_process_for_zircon.md) |

Other changes that might benefit of the RFC process are ones that require manual
or automated large scale changes of the codebase. For example how logs are
written or how error paths are handled. Rather than live with islands of
consistency, the aspiration is to find the best patterns and uniformly apply
them to the entire codebase.

### When to start the RFC process

Authors are encouraged to initiate the RFC process with a problem statement as
soon as they find it useful. This should be relatively lightweight, and the
author may choose to exit the RFC process later if they decide that publishing
an RFC is not necessary.

Some ideas may benefit from early discussion with stakeholders to discover
requirement while other ideas benefit from a prototype to understand the
tradeoffs.

### Roles and responsibilities {#roles-and-responsibilities}

People interact with the RFC process in several roles:

* _RFC Authors._ An RFC Author is a person who writes an RFC. Everyone who
   contributes to Fuchsia can be an RFC Author. A given RFC can have one or more
   authors. The authors of a given RFC drive the process for that RFC.

* _Eng Council._ The [Eng Council (FEC)](../eng_council.md) facilitate
   discussion and make the final decision as to whether the project accepts an
   RFC.

* _Facilitator._ The person appointed by FEC to shepherd this RFC through the
   RFC process. Today, this person is typically an FEC member. The facilitator
   advises the author and may also act as a point of escalation to ensure
   timeline feedback from stakeholders.

* _Stakeholder._ A stakeholder is a person who has a stake in whether the
   project accepts a given RFC. Stakeholders are typically Fuchsia contributors,
   but some RFCs might have stakeholders beyond the Fuchsia project. For
   example, stakeholders might be involved in other projects that use Fuchsia or
   are otherwise affected by changes to Fuchsia. Stakeholders do not always
   participate directly in discussions about RFCs. Instead, stakeholders are
   often _represented_ by someone, often a technical lead or other person
   responsible for a group of stakeholders.

* _Reviewer(s)._ The stakeholders whose +1 or -1 will be considered when the
   FEC decides to accept or reject the RFC. (While a +2 is the "approve" on code
   CLs, we tend to look to reviewers to +1 or -1 to indicate their support or
   lack thereof, and look to the facilitator to +2 upon approval.)

* _Consulted._ The stakeholders whose feedback on the RFC was sought, but whose
   +1 or -1 is not considered when the FEC decides to accept or reject the RFC.

### How the process works

This section describes each step involved in the RFC process.

#### Step 1: Identify the problem

The first step in the RFC process is to identify a problem, and socialize that
problem with your project. Are other people aware of this problem? Someone else
might already be working on the problem or might have some background or context
about the problem that would be useful to you. The earlier you discover this
information, the better.

Please note that the problem statement does not need to be polished or formally
written up before starting this step. It's best to start socializing as early as
possible to receive feedback on whether the idea is feasible and if the
direction is correct. This can potentially save the authors time and effort in
case the idea does not materialize or if the direction needs to change
significantly.

There are no constraints on the method(s) of communication used at this stage of
the process. While the formal writeup for an RFC eventually takes shape as a
markdown file reviewed using a Gerrit code change, authors should use whatever
medium they find useful at this stage.

*Exit criteria*: The author has a clear understanding of the problem they want
to solve OR the author decides this problem does not require the RFC process.

#### Step 2: Problem statement

The author should write up a brief statement of the problem to be
solved. This can be in any format that's useful for discussion, and
should provide enough information so that readers can determine which
Fuchsia subsystems may be affected. This problem statement should be specific
about any requirements known at this stage, although more may be discovered
later. It should also make any "non-goals" clear, to avoid scope-creep.

*Exit criteria*: The author emails the problem statement to [Eng Council
(FEC)](../eng_council.md).

#### Step 3: Assign facilitator

Engineering council assigns a facilitator. This facilitator is a resource for
the RFC author to help refine the problem statement and identify relevant
stakeholders. The facilitator may also advise the author about whether this
problem requires or will benefit from a formal RFC writeup. In some cases, a
facilitator may advise the author to simply proceed to design and implementation
outside the RFC process.

*Exit criteria*: Facilitator is assigned, author receives email notifying them
of their facilitator.

#### Step 4: Stakeholder discovery

During this phase, the RFC author should identify the stakeholders for this RFC.
The author may ask their facilitator to advise in this process. The list of
stakeholders should be written down alongside the problem statement. This may
take place in a Google Doc, or the author may choose to create an RFC CL with a
mostly-empty RFC template and fill out its stakeholder section.

*Note*: If needed, stakeholders may be added or removed later in the process by
the author or the facilitator.

*Exit criteria*: Author and facilitator agree on a set of stakeholders, and the
stakeholders are recorded in a Google Doc or a changelist with the RFC template.

#### Step 5: Socialization

During this phase, identify solutions to your problems and the pros and cons of
each solution. Socialize these solutions with your stakeholders, and incorporate
their feedback. If you are unsure how to socialize your idea, consider asking
your facilitator or a technical leader for advice. They will often have more
experience socializing ideas and might be able to point you in a good direction.

> _Example._ This RFC was socialized by having a discussion in the Eng Forum,
> which is a regular meeting inside Google of various engineering leaders
> involved in the project. The RFC was also socialized with the creators of the
> FTP and CTP process, who have good background and context about these
> processes.

*Exit criteria*: When the author feels they have enough information to proceed.
This is per the author's discretion. If they feel that this is accomplished,
then they have several options:

* _Prototype._ If the socialization phase turns up different approaches with
  unclear tradeoffs, the author may choose to go build a prototype to better
  evaluate these options. This prototype may lead to further socialization.
* _Leave the RFC process._ If the author (possibly with help from the
  facilitator) concludes that this problem doesn't require or benefit from an
  RFC, they can proceed with building a solution using the [code review
  process](/docs/development/source_code/contribute_changes.md). The author may
  also choose to publish a design document informally.
* _Writeup._ Proceed to step 6.

#### Step 6: Draft and iteration {#draft}

Once the author has gathered all the background and context they can through
socialization, they are ready to start the formal part of the RFC process. The
next step is to write a draft of the RFC document itself. Early drafts and
feedback may happen outside of markdown (e.g. in a Google Doc). However if there
is a formal RFC writeup later in the process, it is strongly encouraged to carry
over the relevant context from the more dynamic medium over to RFC writeup in
markdown. For instance, back-and-forth conversations may lead to additional
"alternatives considered" entries to be added.

When the author is ready, they should create a CL that adds a file to the
`//docs/contribute/governance/rfcs` directory, starting with a copy of the
[RFC template](TEMPLATE.md).

Any other files that are part of the RFC, diagrams for example, can be added to
the `resources` directory under a subfolder with the same name as the RFC itself.
Example:`//docs/contribute/governance/rfcs/resources/<RFC_name>/diagram.png`.

Do not worry about assigning a number to the RFC at this stage. Instead, use
`NNNN` as a placeholder. For example, the file name should be something like
`NNNN_my_idea.md`. The RFC will get a number shortly before landing.

> _Tip._ Consult the [RFC best practices doc](best_practices.md) for advice
> about drafting and iterating on the RFC.

> _Suggestion._ Consider marking the CL containing the RFC as a
> "work-in-progress" until you are ready for feedback.

The author should invite stakeholders to provide feedback on the RFC by adding
them to the "Reviewers" (for stakeholders whose +1 is required) or "CC" fields
(for "consulted" stakeholders) in the CL, as for a normal code review. In
addition, the author may email their CL to <eng-council-discuss@fuchsia.dev>
soliciting additional feedback. The stakeholders should provide feedback by
leaving comments on the RFC in the code review tool.

Reviewers are expected to respond to requests for review and to followup
comments within five business days. The author and reviewers should use Gerrit's
"attention set" feature to keep track of who needs to respond. If responses will
take longer than this (e.g. if a protype needs to be built) this should be
indicated in CL comments.

If the discussion is too complex for the code review tool or is taking longer
than you would like, consider scheduling a meeting with the relevant
stakeholders to have a more efficient discussion. After the meeting, you must
post a summary of the meeting in a comment on the CL so that people who were not
at the meeting can understand what was discussed during the meeting.

If the author runs into any issues or delays during the discussion, the
facilitator can help resolve them. Below are some common issues that arise
during these discussions:

* The discussion becomes contentious or non-productive. Often a face-to-face
   meeting can get the discussion back on track more efficiently than text-based
   communication.
* A stakeholder is overly nit-picky (fillibuster) or stalls the discussion
   (non-responsive). Sometimes the stakeholder dislikes the proposal but does
   not have a solid technical argument and resorts, perhaps unconciously, to
   these delaying tatics.
* You receive many comments, suggests, and pushback from many people and are
   unsure which comments are from relevant stakeholders(s). Often much of this
   feedback is irrelevant to the core issue and will not block the RFC being
   accepted.
* The discussion is simply taking too long to converge.

If the discussion becomes contentious or if you have difficulty getting feedback
from a stakeholder, please let your faciliator know. Your faciliator can help
move the discussion forward, for example by providing additional structure to
the discussion or moving the discussion to another forum, such as a synchronous
meeting or an [engineering
review](/docs/contribute/governance/eng_council.md#eng-review). Regardless of
how the discussion proceeds, the results of any off-CL discussion must be
captured in the CL, often by posting a summary of the discussion as a CL
comment.

Feedback may include comments from people who are not stakeholders. The author
should respond to these comments if relevant, but settling them is not
necessarily required to move to the _Last Call_ stage. If the comments point to
a disagreement about who is a stakeholder, FEC can help resolve this.

If the problem ceases to be relevant or the author and stakeholders agree that
the problem does not warrant an RFC, the author may withdraw the RFC at this
point. In this case, the author should mark the CL containing the RFC as
abandoned. The author, or someone else, can always resurrect your RFC later if
circumstances change. When resurrecting an RFC created by someone else,
you should start the RFC process over from the beginning, but you can use the
withdrawn RFC as a starting point rather than `TEMPLATE.md`. Please confer with
the original authors to determine whether they wish to continue to have their
names associated with the new incarnation of the RFC.

*Note to reviewers:* The RFC process is meant to encourage a variety of
perspectives and vibrant discussions. Often, giving negative feedback in a public
forum might be difficult. If needed, reviewers can reach out to their leads,
peers or Eng Council for help.

> _Suggestion._ If you are interested in RFCs, consider configuring the Gerrit
> Code Review tool to [send you an email notification](https://gerrit-review.googlesource.com/Documentation/user-notify.html)
> when a CL modifies the `//docs/contribute/governance/rfcs` directory.

*Exit criteria:* RFC CL is out for review, with feedback solicited and
incorporated into the RFC CL OR RFC is withdrawn.

#### Step 7: Last call {#last-call}

Once the discussion on the RFC is converging, the author must ask their
facilitator to move the RFC's status to Last Call. The faciliator will mark
the RFC as being in Last Call in the tracker, which generates automatic email
to <eng-council-discuss@fuchsia.dev>, and will comment on the CL to solicit any
final feedback before moving to the decision step. The RFC will be open for
feedback for the next 7 calendar days.

Typically, reviewers sign off with a +1 and the facilitator will sign off with a
+2. Consulted stakeholders may also sign off with a +1 or +2 if they wish to
express their enthusiasm for the RFC, although this is not required.

Stakeholders who wish to object to an RFC can set the Code-Review flag to -1 or
-2, depending on how strongly they feel that the RFC should not move forward.
When setting the Code-Review flag to -1 or -2, a stakeholder must state their
reason for objecting, ideally in a way that would let someone understand the
objection clearly without having to read the entire discussion that preceded
the objection.

A stakeholder setting the Code-Review flag to -1 or -2 does not necessarily
prevent the project from accepting the RFC. See the ["How decisions are made"
section](#how-decisions-are-made) below for more details about how the project
decides whether to accept an RFC.

After all the stakeholders have weighed in with their Code-Review flags, send an
email to <eng-council@fuchsia.dev> to prompt the Eng Council to decide whether to
accept your RFC.

*Exit criteria:* Feedback provided by all stakeholders; all feedback addressed.

#### Step 8: Submit

If the project decides to accept your RFC, a member of the Eng Council will
comment on your CL stating that the RFC is accepted and will assign the RFC a
number, typically the next available number in the series. If there are any -1
or -2 Code-Review flags, the Eng Council will explicitly clear each flag by
summarizing the objection and by describing why the RFC is moving forward
despite the objection. Eng Council will indicate if any additional information
needs to be documented in your RFC, such as rationale for a different approach
or tradeoffs being made.

If the project decides to reject your RFC, a member of the Eng Council will
comment on your CL stating that the RFC is rejected, provide a rationale
for the rejection and will assign the RFC a number. Rejected RFCs are valuable
engineering artifacts. The Eng Council will work with the RFC Authors to land
a version of the RFC that is marked as rejected and incorporates the rationale.

Rarely, if the Eng Council identifies one or many unresolved open items, the
RFC may be moved back to the [draft](#draft) stage. The Eng Council will ask of
the author(s) to resolve the open items identified with the relevant
stakeholders before another request to review the RFC will be granted.

You should upload a new patchset of your RFC with the assigned number, both in
the title of the RFC and in the filename. If your RFC is approved and requires
implementation, please make sure you have an issue filed in the issue tracker
and put a link to the issue in the header of your RFC.

The Eng Council will then mark your CL Code-Review +2 and you can land your RFC!

*Congratulations! You have contributed a valuable engineering artifact to the
project!*

*Exit criteria:* RFC number assigned; any applicable rationale, tradeoffs and
Eng Council feedback incorporated; RFC merged.

#### On Hold

If you area not currently interested in moving your RFC forward, for example
because your priorities have shifted or because the problem the RFC was aiming
to solve is no longer as salient, you can move your RFC to the _On Hold_ state.
When an RFC is On Hold, stakeholders are not expected to provide feedback and
the FEC will not actively facilitate progress.

If there is no visible progress on the CL for your RFC in a month, the RFC bot
will send an email to you and your facilitator to prompt a discussion about
whether to move your RFC to the On Hold state or how to move your RFC forward.
Ideally, if you are encountering difficulities or delays, you would have
escalated this situation to your faciliator before this point, but this
conversation is also a good time to surface those issues.

You can move your RFC out of the On Hold state at any time by contacting your
faciliator and letting them know that you plan to resume work on your RFC. If
you would like a new faciliator, contact any member of the FEC and they can
assign you a new facilaitor. At this point, your RFC will move back to the
[draft](#draft) stage.

*Exit criteria:* RFC Author states their intention to resume active work on
their RFC; Faciliator comments on the CL for the RFC indicating that the RFC is
no longer on hold.

### How decisions are made

The decision whether to accept an RFC is made by the Eng Council, acting in
[rough consensus](https://en.wikipedia.org/wiki/Rough_consensus) with each
other. If the decision involves an RFC that has Eng Council members as authors,
those members must recuse themselves from the decision.

If the Eng Council cannot reach rough consensus, the RFC is not accepted.
In deciding whether to accept an RFC, the Eng Council will consider the
following factors:

* Does the RFC advance the goals of the project?
* Does the RFC uphold the values of the project?
* Were all of the stakeholders appropriately represented in the discussion?
* If any stakeholders objected, does the Eng Council understand the objections
   fully?

Decisions made by the Eng Council can be escalated to the governing authority
for the project.

### Process to amend RFCs

An existing RFC can be amended if the following criteria are met:

* Clarifications on what was already approved.
* Mechanical amendments such as updating links, documentation, usage, etc.
* Any improvement or minor changes in design discovered later, for example,
 during implementation.

For changes in design, please capture what the original design goals were, and
why and how they changed.

For any significant changes in design, please submit a new RFC.

* In the new RFC, please reference the original RFC(s) and explicitly call out
 the type of change in the title, e.g., Addendum.
* If the design in the original RFC is being deprecated, amend the original RFC
  to call this out and reference the new RFC.
* If there are multiple RFCs that make changes to the same area, create a new
 RFC compiling the existing RFCs. Please also amend the existing RFCs to
 reference the new one.

If the RFC process is being updated, please also update the
[RFC process page](rfc_process.md).

## Documentation

This RFC serves as documentation for the RFC process.

## Drawbacks, Alternatives, and Unknowns

The primary cost of implementing this proposal is that introducing a formal
decision-making process might slow down the pace of decision-making. The process
might be heavier than necessary for some kinds of decisions.

Recording decisions in the source repository has the effect of making those
decisions more difficult to change. That effect might be positive in some
scenarios, but the effect might also be negative in other scenarios.

The criteria in the ["when to use the process"
section](#when-to-use-the-process) attempts to mitigate this drawback by scoping
the process to consequential situations but such scoping is bound to have false
positives and false negatives.

There are a large number of possible alternative strategies for solving the
underlying problem. For example, we could use a decision-making process that
centers around a synchronous meeting, but such a process will have difficulty
scaling to a global open-source project. We could also have selected a different
decision-making mechanism that balanced more towards consensus or more towards
authority.

## Prior art and references

There is a good deal of prior art about decision-making processes for
open-source projects. This proposal is strongly influenced by the following
existing processes:

* _IETF RFC process._ The IETF has run a successful, large-scale
   [decision-making process](https://ietf.org/standards/process/) for a long
   period of time. The process described in this document draws a number of
   ideas from the IETF process, including some of the terminology.

* _Rust RFC process._ The Rust community runs an [RFC
   process](https://github.com/rust-lang/rfcs/blob/HEAD/text/0002-rfc-process.md),
   which has been effective at making decisions for somewhat similar software
   engineering project. The process described in this document is fairly
   directly modelled after the Rust RFC process.

* _Blink Intent-to-implement process._ The Chromium project runs a
   [decision-making process](https://www.chromium.org/blink/launching-features)
   for behaviors that affect web pages. The process described in this document
   is informed by my (abarth) experience helping to design and run that process
   for a period of time.

* _FIDL Tuning Proposal._ The Fuchsia project has had direct experience using a
   similar process [to make decisions about the FIDL
   language](/docs/contribute/governance/deprecated-ftp-process.md). This
   proposal exists because of the success of that decision-making process.

[create]: /docs/contribute/governance/rfcs/create_rfc.md
