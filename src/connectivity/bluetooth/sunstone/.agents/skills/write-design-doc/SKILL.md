---
name: write-design-doc
description: Guide for writing design documents in the Sapphire project.
---

# Writing Design Documents for Sapphire

This skill guides you through creating a new design document for the Sapphire project.

## Instructions

When asked to write a design document, follow these steps:

1.  **Ask Clarifying Questions**: Before starting to write, identify areas of
    uncertainty, ambiguity, or missing requirements. Ensure you have a solid
    understanding of the goals before proceeding.
    *   **Do Your Homework First**: Search existing documentation and the
        codebase to try to find answers before asking the user.
    *   **Propose Options**: When asking, present concrete options or hypotheses
        rather than just open-ended questions (e.g., "Should we do A or B?
        I recommend A because...").
    *   **Categorize Questions**: Group your questions logically (e.g., Scope,
        Architecture, Security) to make them easy for the user to review.

2.  **Use the Template**: Use the template file
    `//src/connectivity/bluetooth/sunstone/design_docs/templates/default.md`
    as the starting point.

3.  **Determine the Filename**:
    *   The file must be named using the format: `YYYY_MM_DD_title.md`.
    *   Replace `YYYY_MM_DD` with the current date (e.g., `2026_05_04`).
    *   Replace `title` with a lowercase, underscore-separated version of the
        design doc title (e.g., `my_awesome_feature`).
    *   Example: `2026_05_04_my_awesome_feature.md`.

4.  **Destination Directory**:
    *   Save the new design document in
        `//src/connectivity/bluetooth/sunstone/design_docs/`.

5.  **Update Frontmatter**:
    *   Update the YAML frontmatter in the new file:
        *   `title`: Set to the actual title of the design doc.
        *   `description`: Provide a brief description.
        *   `status`: Set to `approved`.
        *   `authors`: Add the author's email (e.g., `bob@google.com`).
        *   `bugs`: Add relevant bug IDs if known, or remove if not applicable.

6.  **Fill in the Content**:
    *   Fill in all the sections defined in the template.
    *   Make sure to follow instructions in the comments of each section.

7. **Request a review from a subagent**:
   *   Use `invoke_subagent` to request a review of the doc. Use the
       design-doc-reviewer agent config.
   *   Address the review feedback.
