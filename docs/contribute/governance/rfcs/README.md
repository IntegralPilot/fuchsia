# Fuchsia RFCs

The [Fuchsia RFC process](rfc_process.md)
is intended to provide a consistent and transparent path
for making project-wide, technical decisions. For example, the RFC process can
be used to evolve the project roadmap and the system architecture.

The RFC process evolves over time, and can be read here in its [detailed current
form](rfc_process.md). It is also summarized below.

## Summary of the process

- Review [when to use the process](rfc_process.md#when-to-use-the-process).
- Socialize your proposal.
- [Draft](rfc_process.md#draft) your RFC using this [template](TEMPLATE.md)
  and share with stakeholders. See [creating an RFC](create_rfc.md) and
  [RFC best practices](best_practices.md).
- As conversations on your proposal converge, and stakeholders indicate their
  support, email <eng-council@fuchsia.dev> to ask the Eng Council
  to move your proposal to [Last Call](rfc_process.md#last-call).
- After a waiting period of at least 7 days, the Eng Council will accept or
  reject your proposal, or ask that you iterate with stakeholders further.

For detailed information, follow the [RFC process](rfc_process.md).

## Summary of the process (deck)

<!-- Wrap the iframe in a div to get fixed-aspect-ratio responsive behavior -->
<!-- mdlint off(WHITESPACE_LINE_LENGTH) -->
<div style="padding-top: 62%; position: relative; width: 100%">
  <iframe
    src="https://docs.google.com/presentation/d/e/2PACX-1vT8Sofn5v3d-PP7fcBw9YTH4vukwlvscjjqKsC4eItDVp79qYbENpAKer6ZoE_bQ3vD23dwHYrBn_aP/embed?start=false&loop=false&delayms=3000"
    frameborder="0" width="480" height="299"
    allowfullscreen="true" mozallowfullscreen="true" webkitallowfullscreen="true"
    style="position: absolute; top: 0; left: 0; width: 100%; height: 100%"></iframe>
</div>

## Stay informed

You can configure [Gerrit Notifications](https://fuchsia-review.googlesource.com/settings/#Notification)
to email you when new RFCs are uploaded.

Include the `docs/contribute/governance/rfcs` search expression
and select **Changes** to receive email notifications for
each new RFC proposal.

![Gerrit settings screenshot demonstrating
the above](resources/gerrit_notifications.png)

## List of all RFCs

For the full list of all RFCs, see
[List of all Fuchsia RFCs](/docs/contribute/governance/rfcs/all_rfcs.md).