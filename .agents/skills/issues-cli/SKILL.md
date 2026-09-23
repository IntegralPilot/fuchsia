---
name: issues-cli
description: >
  Interacts with Google's issue tracker system. Provides capabilities to
  search bugs and components, render issue details, add comments, create new
  issues, and update issue metadata via the CLI.
---

# Issues CLI Skill

This skill allows agents to manage the lifecycle of issues and components in
Google's issue tracker. It provides a suite of commands to search for
information, communicate via comments, initialize new work items, and maintain
existing ones.

> [!CAUTION] **Write actions (create, update, comment, attachments, hotlists,
> star, vote, subscribe) MUST be explicitly authorized.** Never invoke them
> autonomously. Read-only commands (`render`, `search`, `list-*`, `get-*`) are
> safe to auto-run.

> [!IMPORTANT]
> Accessing public bug tickets using the `$ISSUES` command is not yet supported.
> Error messages mentioning `render_issue_with_external` may reflect this issue.
> However, the `render_issue_with_external` option is not yet available.

## When to use this skill

- Use this when you need to find existing bugs or feature requests related to a
  specific topic or component.
- Use this when you need to identify the correct Component ID for filing a new
  issue.
- Use this to read the details, status, and history of a specific issue.
- Use this to provide updates or feedback on an issue by adding a comment.
- Use this to file a new bug or feature request with specific metadata like
  priority, severity, and assignee.
- Use this to update existing issue fields like title, status, priority, or
  custom fields.
- Use this to manage issue relationships such as duplicates, blocking issues,
  and parent/child links.

## How to use it

By default, use the pre-built binary (available on all gLinux machines):

```bash
ISSUES=/google/bin/releases/issues-cli/issues
```

All interactions are then performed using the `$ISSUES` command.

**Policy note:** When running commands, prefer to use the full binary path
directly (e.g. `/google/bin/releases/issues-cli/issues search ...`) rather than
shell variable expansion, as this increases the chance that the command will be
accepted by the user.

### 1. Search for Issues

Search for bugs matching a query. `$ISSUES search --query "<SEARCH_QUERY>"
[--limit <MAX_RESULTS>] [--verbose]`

- **query**: The standard search syntax.
- **limit**: Default is 10.

See the [Appendix] Issue Search section for more details on how use this
command.

### [Appendix] Issue Search

This section provides comprehensive guidance on constructing advanced issue
tracker search queries. It covers the full range of search tokens, boolean
operators, and date-based filtering.

How to use it:

1.  Constructing a Query:

Use the `issues search --query "<QUERY>"` command. A query consists of one or
more tokens.

- **Keywords**: Search in title and comments (e.g., `performance regression`).
- **Atoms**: Key-value pairs (e.g., `status:open`).
- **Operators**: Use space for AND, `|` for OR, and `-` for NOT.

2.  Common Scenarios:

Scenario                       | Query Pattern
:----------------------------- | :-------------------------------------
**Bugs assigned to me**        | `assignee:me status:open`
**Recent bugs in a component** | `componentid:12345 created:7d`
**High priority bugs (P0/P1)** | `priority:p0
**Bugs I'm CC'd on**           | `cc:me status:assigned`
**Specific phrase**            | `"status:open performance regression"`

3.  Advanced Syntax Reference:

For a complete list of all supported tokens (like `blockedbyid`, `hotlistid`,
`staffinguser`, `cl`, `incident`, and `customfield<id>`), see the [Appendix]
Issue Tracker Search Query Syntax Reference section below.

Best Practices:

- **Use Component IDs**: Precise searching in a specific component is faster
  than keyword searches. Use `componentid:12345+` to include sub-components.
- **Date Filtering**: Use relative dates like `created:30d` for fresh issues.
- **Boolean Logic**: Use parentheses for complex logic to ensure correct
  operator precedence.
- **Verbose Mode**: Add `--verbose` to the CLI command to see more metadata like
  vote counts and custom fields in the results. IMPORTANT: Only use verbose if
  you truly need the field metadata.

### 2. Search for Components

Find components to ensure work is filed in the correct location. `$ISSUES
search-components --query "<COMPONENT_NAME>" [--include_description]`

- **include_description**: Includes the component's description in the output.

### 3. View Issue Details (Render)

Render the full details of a specific issue. `$ISSUES render --issue_id <ID>
[--verbose]`

- **issue_id**: The numeric issue tracker ID. The `issue_id` flag cannot be
  repeated.

### 4. Add a Comment

Post a new comment to an existing issue. Use either `--comment` or
`--comment_file`:

```bash
$ISSUES comment --issue_id <ID> --comment "<MESSAGE>"
# OR
$ISSUES comment --issue_id <ID> --comment_file <PATH>
```

- **comment**: The text or markdown to add as a comment.
- **comment_file**: The file to read a comment from. Preferred for multi-line
  payloads.

### 5. Create a New Issue

File a new issue with complete metadata. `$ISSUES create --title "<TITLE>"
--description "<DESC>" --component_id <ID> \ [--assignee <LDAP>] [--priority
<P0-P4>] [--severity <S0-S4>] \ [--type <TYPE>] [--cc <LDAP1,LDAP2>] \ [--status
<STATUS>] [--hotlists <ID1,ID2>]`

- **Required**: `title`, `description`, and `component_id`.
- **Priority/Severity**: Use standard enums (e.g., `P1`, `S2`).
- **Status**: Use standard enums (e.g., `NEW`, `ASSIGNED`, `ACCEPTED`).
- **Hotlists**: Comma-separated list of hotlist IDs.

### 6. Update Existing Issues

Modify existing issues using specialized sub-commands. All `update` commands
follow the pattern: `$ISSUES update <SUBCOMMAND> --issue_id <ID> [FLAGS]`

- **title**: `$ISSUES update title --issue_id <ID> --title "<NEW_TITLE>"`
- **status**: `$ISSUES update status --issue_id <ID> --status "<STATUS>"` (e.g.,
  `ASSIGNED`, `FIXED`, `WON'T_FIX`)
- **priority**: `$ISSUES update priority --issue_id <ID> --priority "<P0-P4>"`
- **assign-to-self**: `$ISSUES update assign-to-self --issue_id <ID>`
- **safe-reassign**: `$ISSUES update safe-reassign --issue_id <ID> --assignee
  <LDAP_OR_EMAIL>`
- **safe-change-verifier**: `$ISSUES update safe-change-verifier --issue_id <ID>
  --verifier <LDAP>`
- **safe-move**: `$ISSUES update safe-move --issue_id <ID> --target_component_id
  <ID>`
- **safe-cc**: `$ISSUES update safe-cc --issue_id <ID> --cc_user
  <LDAP_OR_EMAIL>`
- **safe-un-cc**: `$ISSUES update safe-un-cc --issue_id <ID> --cc_user
  <LDAP_OR_EMAIL>`
- **verify-as-self**: `$ISSUES update verify-as-self --issue_id <ID>`
- **duplicate**: `$ISSUES update duplicate --issue_id <ID> --target_id <ID>` or
  `unmark-duplicate`
- **hotlist**: `$ISSUES update add-hotlist --issue_id <ID> --hotlist_id <ID>` or
  `remove-hotlist`
- **blocking**: `$ISSUES update blocked-by --issue_id <ID> --target_ids
  <ID1,ID2>` (also `unmark-blocked-by`, `blocking`, `unmark-blocking`)
- **parent**: `$ISSUES update parent --issue_id <ID> --parent_id <ID>` (also
  `remove-parent`)
- **custom-field**: `$ISSUES update custom-field --issue_id <ID> --field_id <ID>
  --value "<VAL>"`
- **edit-comment**: `$ISSUES update edit-comment --issue_id <ID> --comment_num
  <N> --text "<STR>"`
- **description**: `$ISSUES update description --issue_id <ID> --text "<STR>"`
- **status-update**: `$ISSUES update status-update --issue_id <ID> --text
  "<STR>"`
- **severity**: `$ISSUES update severity --issue_id <ID> --severity "<S0-S4>"`
- **type**: `$ISSUES update type --issue_id <ID> --type "<TYPE>"` (e.g., `BUG`,
  `FEATURE_REQUEST`, `CUSTOMER_ISSUE`)
- **effort**: `$ISSUES update effort --issue_id <ID> --effort "<EFFORT>"` (e.g.
  Story points (1, 2, etc.) or T-shirt sizes (`XS`, `S`, `M`, `L`, `XL`))
- **add-changelist**: `$ISSUES update add-changelist --issue_id <ID>
  --changelist "<CHANGELIST>"`

### 7. Look Up Metadata

Retrieve details about components, hotlists, templates, and custom fields by ID.

- **component**: `$ISSUES get-component <COMPONENT_ID>`
- **hotlist**: `$ISSUES get-hotlist <HOTLIST_ID>`
- **template**: `$ISSUES get-template <TEMPLATE_ID> --component_id
  <COMPONENT_ID>`
- **search hotlists**: `$ISSUES list-hotlists --query "<QUERY>"`
- **list custom fields**: `$ISSUES list-custom-fields --component_id <ID>`

### 8. Hotlist Management

Create and browse hotlists.

- **create hotlist**: `$ISSUES create-hotlist --title "<TITLE>" [--description
  "<DESC>"]`
- **list hotlist entries**: `$ISSUES list-hotlist-entries --hotlist_id <ID>
  [--limit <N>]`

### 9. Issue History & Relationships

View an issue's change history, relationships, and batch-fetch multiple issues.

- **list updates**: `$ISSUES list-updates --issue_id <ID> [--limit <N>]`
- **list relationships**: `$ISSUES list-relationships --issue_id <ID>`
- **batch get**: `$ISSUES batch-get --issue_ids <ID1,ID2,...> [--verbose]`

### 10. Bookmark groups

Browse and access bookmark groups.

- **list bookmark groups**: `$ISSUES list-bookmark-groups --query "<QUERY>"`
- **get bookmark group**: `$ISSUES get-bookmark-group <GROUP_ID>`

### 11. View Attachments and Enrichments

Retrieve attachments and AI-generated enrichments for an issue.

- **attachments**: `$ISSUES list-attachments --issue_id <ID>`
- **download-attachment**: `$ISSUES download-attachment --issue_id <ID>
  --attachment_id <ID> [--output <FILE>] [--allowed_severity <SEVERITY>]`
- **enrichments**: `$ISSUES list-enrichments --issue_id <ID>`
- **list attachment enrichments**: `$ISSUES list-attachment-enrichments
  <ISSUE_ID>`

### 12. Star, Vote, and Subscribe

Manage user engagement with issues.

- **star**: `$ISSUES star-issue --issue_id <ID> --starred true` (or `false` to
  unstar)
- **vote**: `$ISSUES vote-issue --issue_id <ID> --voted true` (or `false` to
  unvote)
- **subscribe**: `$ISSUES subscribe-issue --issue_id <ID> --subscription
  ALL_UPDATES` (or `NO_UPDATES`)

### 13. Issue Hierarchy

Manage parent-child relationships.

- **add child**: `$ISSUES update child --issue_id <PARENT_ID> --child_id
  <CHILD_ID>`

### 14. SLO and Hotlist Management

Update SLO deadlines and rename hotlists.

- **update SLO end time**: `$ISSUES update slo-end-time --issue_id <ID>
  --slo_end_time <RFC3339>` (e.g., `2025-06-15T00:00:00Z`)
- **rename hotlist**: `$ISSUES update hotlist-name --hotlist_id <ID> --name
  "<NEW_NAME>"`

### 15. Attachments

Create and retrieve attachment metadata. Use `--file` to upload file data.

- **create attachment**: `$ISSUES create-attachment --issue_id <ID> --file
  <PATH>` (auto-detects filename and content type; override with `--filename`
  and `--content_type`)
- **get attachment**: `$ISSUES get-attachment --issue_id <ID> --attachment_id
  <AID>`

### 16. Saved Searches and User Access

Retrieve saved searches and check user permissions.

- **get saved search**: `$ISSUES get-saved-search --saved_search_id <ID>`
- **get user access**: `$ISSUES get-user-access --issue_id <ID>`

## Best Practices

- **Validate Component IDs**: Always use `search-components` before creating an
  issue to ensure the `component_id` is valid.
- **Specific Metadata**: When creating issues, provide as much metadata as
  possible (assignee, priority, type) to ensure prompt triage.
- **Markdown Comments**: Use markdown in comments for better readability of logs
  or code snippets.

## [Appendix] Issue Tracker Search Query Syntax Reference

This document provides a detailed list of search tokens and operators available
for constructing queries in Google's issue tracker.

### Operators

- **AND**: Use space between tokens (e.g., `reporter:me status:open`).
- **OR**: Use `|` (e.g., `priority:p0 | priority:p1`).
- **NOT**: Use `-` (e.g., `-status:closed`).
- **Grouping**: Use parentheses (e.g., `(assignee:me | cc:me) status:open`).

### Search Tokens

| Token | Description |
| :--- | :--- |
| `id` | The issue ID. |
| `blockingid` | ID of an issue that is blocked by the issue. |
| `blockedbyid` | ID of an issue blocking the issue. |
| `parentid` | ID of the parent issue. Appending `+` includes transitive children. |
| `canonicalid` | ID of the canonical issue for a duplicate issue. |
| `hotlistid` | ID of a hotlist containing the issue. |
| `componentid` | ID of the component the issue is in. Appending `+` includes child components. |
| `trackerid` | ID of the tracker the issue is in. |
| `teamid` | ID of a team that has a role on the issue. Appending `+` includes child teams. |
| `reporter` | The user who reported the issue. |
| `assignee` | The user assigned to the issue. |
| `collaborator` | A user who is a collaborator on the issue. |
| `cc` | A user CC'd on the issue. |
| `verifier` | The user who verified the issue. |
| `escalationowner` | The user who owns the escalation. Appending `+` includes subordinates. |
| `mention` | A user mentioned in a comment. |
| `modifier` | A user who has modified the issue. |
| `lastmodifier` | The last user to modify the issue. |
| `commenter` | A user who has commented on the issue. |
| `lastcommenter` | The last user to comment on the issue. |
| `staffinguser` | Staffing user on the issue. |
| `priority` | The priority of the issue (P0-P4). |
| `severity` | The severity of the issue (S0-S4). |
| `type` | The type of the issue (BUG, FEATURE_REQUEST, etc.). |
| `status` | The status of the issue (NEW, ASSIGNED, ACCEPTED, etc.). |
| `deletionreason` | The reason the issue was deleted. |
| `staffingplaceholder` | Staffing placeholder on the issue. |
| `accesslevel` | The access level of the issue. |
| `title` | Text in the issue's title. |
| `comment` | Text in the issue's comments. |
| `attachment` | The filename of an attachment. |
| `insight` | Text in the issue's insights. |
| `attachmentanalysis` | Text in the attachment analysis. |
| `foundin` | A build or version where the issue was found. |
| `targetedto` | A build or version where the issue is targeted to be fixed. |
| `verifiedin` | A build or version where the issue was verified. |
| `effortlabel` | The effort label for the issue. |
| `cl` | A changelist associated with the issue. |
| `postmortem` | A postmortem associated with the issue. |
| `incident` | Associated incident ID. |
| `created` | The creation date of the issue (supports relative dates like `created:14d`). |
| `modified` | The last modified date of the issue. |
| `resolved` | The date the issue was resolved. |
| `verified` | The date the issue was verified. |
| `nearestslo` | The nearest SLO date for the issue. |
| `deletiontime` | The deletion time of the issue. |
| `duplicatecount` | The number of duplicates of the issue. |
| `votecount` | The number of votes for the issue. |
| `commentcount` | The number of comments on the issue. |
| `collaboratorcount` | The number of collaborators on the issue. |
| `cccount` | The number of users CC'd on the issue. |
| `descendantcount` | The number of descendants of the issue. |
| `opendescendantcount` | The number of open descendants of the issue. |
| `attachmentcount` | The number of attachments on the issue. |
| `onedayviewcount` | The number of views in the last day. |
| `sevendayviewcount` | The number of views in the last seven days. |
| `thirtydayviewcount` | The number of views in the last thirty days. |
| `staffingusercount` | The number of staffing users. |
| `staffingplaceholdercount` | The number of staffing placeholders. |
| `efforttotalopen` | The total open effort for the issue. |
| `efforttotalcomplete` | The total completed effort for the issue. |
| `inprod` | Whether the issue is in production. |
| `star` | Whether the issue is starred (`star:true`). |
| `archived` | Whether the issue is archived. |
| `mute` | Whether the issue is muted. |
| `deleted` | Whether the issue is deleted. |
| `vote` | Whether the current user has voted for the issue. |
| `customfield<id>` | The value of a custom field. |
| `savedsearchid` | The ID of a saved search. |

### Date and Time Formats

- **Absolute**: `YYYY-MM-DD` (e.g., `created:2024-01-01`)
- **Relative**: `7d` (last 7 days)
- **Range**: `created:2023-01-01..2023-12-31`
- **Today-based**: `created:today`, `modified<today-10`,
  `resolved:today..today+3`
