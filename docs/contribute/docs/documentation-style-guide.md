# Documentation style guide

This document gives writing style guidance for Fuchsia.dev. These
guidelines build on the general guidance in the [Google Developers Style
Guide][google-dev-doc-style-guide].

Note: This guide highlights best practices for Fuchsia documentation. For
general documentation standards and tone, see
[Documentation standards][doc-standard]. For complete Markdown syntax, see the
[Markdown reference guide][markdown-guide].

In this guide:

* [Tone, voice, and grammar](#tone-voice-and-grammar)
* [Outlines and navigation lists](#outlines-and-navigation-lists)
* [Text and links](#text-and-links)
* [Headers](#headers)
* [Lists](#lists)
* [Callouts (notes, warnings, and tips)](#callouts-notes-warnings-and-tips)
* [Horizontal rules](#horizontal-rules)
* [Code samples](#code-samples)

## Tone, voice, and grammar {#tone-voice-and-grammar}

When writing Fuchsia documentation, enforce the following style, tone, and
grammar guidelines to ensure content is clear, consistent, and accessible.

### Use plain U.S. English {#use-plain-us-english}

Write clear, direct U.S. English using simple words and concise sentences.
Use standard contractions (for example, _it's_, _don't_, _you'll_) to create
a natural, approachable tone. Avoid idioms, regional colloquialisms, or slang
that may be difficult for non-native English speakers to translate.

### Address the reader in second person ("you") {#address-the-reader-in-second-person}

Write directly to the reader in the second person ("you").

<span class="compare-better">Recommended</span>: Use second person to speak
directly to the developer:

```none
You can install Fuchsia by running the following command:
```

<span class="compare-worse">Not recommended</span>: Avoid third-person
phrasing ("users can") or first-person plural ("we can"):

```none
Fuchsia users can install the OS, and then we can run the test.
```

### Use present tense and active voice {#use-present-tense-and-active-voice}

State facts and system behavior in the present tense. Avoid future tense
("will"), which can introduce ambiguity about when an action occurs.
In addition, use active voice over passive voice so the subject performing the
action is clear.

<span class="compare-better">Recommended</span>: Active voice and present
tense:

```none
The command creates a configuration file in the working directory.
```

<span class="compare-worse">Not recommended</span>: Passive voice and future
tense:

```none
A configuration file will be created by the command.
```

### Spell out acronyms and use consistent terminology {#spell-out-acronyms-and-use-consistent-terminology}

Spell out acronyms and abbreviations on first mention in a document, followed by
the acronym in parentheses (for example, "Looks Good To Me (LGTM)"). Ensure
technical terminology matches the official Fuchsia [glossary][glossary].

## Outlines and navigation lists {#outlines-and-navigation-lists}

### Include top-level outline and sub-section lists {#include-top-level-outline-and-sub-section-lists}

Long or multi-section documents should provide navigation lists to help readers
orient themselves:

* Include a **top-level outline navigation list** near the top of the document
  (after the introduction and before the first `##` section heading).

* Include a **sub-section step list** at the top of major sections (`##`
  headings) that contain multiple sub-sections (`###`).

### Keep top-level outlines non-redundant {#keep-top-level-outlines-non-redundant}

The top-level outline navigation list should list only major sections
(`##` headings). Do not nest sub-section (`###`) links under major section items
in the top-level outline if those sub-sections are already listed at the top of
their respective major section. Keeping the top outline clean avoids redundancy.

### Choose between numbered and bulleted navigation lists {#choose-between-numbered-and-bulleted-navigation-lists}

Use **numbered list items** (`1. [Section title](#anchor)`) only when sections
are meant to be executed in sequence, such as in a tutorial or how-to guide.
For sequential procedures, precede the navigation list with `"The steps are:"`.

<span class="compare-better">Recommended</span>: Top-level outline for a
sequential procedure:

```none
The steps are:

1. [Prerequisites](#prerequisites)
2. [Build Fuchsia](#build-fuchsia)
3. [Set up Device Cloud](#set-up-device-cloud)
4. [Troubleshooting](#troubleshooting)
```

Use **bulleted list items** (`* [Section title](#anchor)`) for non-sequential
topics, such as an overview, concept page, index page, or reference document.
Do not include `"The steps are:"` when the sections are non-sequential.

<span class="compare-better">Recommended</span>: Top-level outline for a
non-sequential reference or overview document:

```none
* [Overview](#overview)
* [Available skills](#available-skills)
```

For major sections containing multiple sub-sections, provide a sub-section
list at the top of the section:

```none
## Set up Device Cloud {#set-up-device-cloud .numbered}

1. [Set up environment](#set-up-environment)
2. [Check out device](#check-out-device)
3. [Recover and flash](#recover-and-flash)
4. [Serve packages](#serve-packages)
```

## Text and links {#text-and-links}

### Follow the 80 character limit {#follow-the-80-character-limit}

In the Fuchsia project, the maximum line length for code is 100 characters,
while the maximum line length for documentation is 80 characters. Wrap all
prose lines at a maximum length of 80 characters.

Notable exceptions to this rule are URLs, reference link definitions (such as
`[reference-id]: https://...`), and top-level YAML frontmatter, which remain on
a single line without wrapping.

Code tends to be indented (blank space on the left of the page), while English
prose (documentation) tends to form paragraphs of text. This difference leads to
different width specification.

### Mark external links {#mark-external-links}

Use `{:.external}` to mark any links that are not within `fuchsia.dev`,
`fuchsia.googlesource.com`, or `fuchsia-review.googlesource.com`:

```none
This is an [external](http://example.com){:.external} link.
```

Notice the external link icon: This is an
[external][external-link-example]{:.external} link.

### Use reference-style links {#use-reference-style-links}

In general, Fuchsia recommends using reference-style links in Markdown files.
Reference style links use a reference identifier associated with the link, and
then refers to that identifier whenever you use the link in the doc. This makes
links easy to update in the document.

<span class="compare-better">Recommended</span>: Create an identifier where you
want the link.

In this example, the link identifier is called `fuchsia-home`:

```none
Welcome to the [Fuchsia home page][fuchsia-home].
```

And then define it at the bottom of the document:

<pre><code>&#91;fuchsia-home]: https://fuchsia.dev/</code></pre>


<span class="compare-worse">Not recommended</span>: Writing an in-line link
like the following:

```none
Welcome to the [Fuchsia home page](www.fuchsia.dev).
```

You can read more about reference style links in the external
[Markdown Guide][markdown-reference-links].

### Use correct links to different Fuchsia content {#use-correct-links-to-different-fuchsia-content}

In the Fuchsia documentation you can link to three types of contents:

* `/docs/` - Link to documents that are in the `/docs/` directory of the Fuchsia
  source tree. These links must link to a file with an `.md` extension. For
  example, `/docs/concepts/README.md`.

* Source code - Link to source code files that exist within the Fuchsia source
  tree. These links can link to any file extension, but these files must exist
  in the source tree. For example, `/sdk/lib/fdio/fdio.cc`.

  Note: If linking to a specific line number or making use of a search query,
  use the full link to the file. For example,
  [https://cs.opensource.google/fuchsia/fuchsia/+/main:docs/README.md;l=17](https://cs.opensource.google/fuchsia/fuchsia/+/main:docs/README.md;l=17).

* Reference documentation - Links to auto-generated Fuchsia reference
  documentation.
  * Most of the Fuchsia reference documentation doesn't exist in
    the source tree, but is published on [fuchsia.dev][fuchsia-dev]. These links
    must be used as fully qualified URLs. For example,
    `https://fuchsia.dev/reference/fidl/fuchsia.io`.
  * However, some Fuchsia reference documentation exists in the source
    tree. These documents exist in `/docs/reference/` and are published in the
    `https://fuchsia.dev/fuchsia-src/reference/` section. These links must link
    to a file with an `.md` extension. For example,
    `/docs/reference/fidl/bindings/overview.md`.

### Test your links before submitting a change {#test-your-links-before-submitting-a-change}

Once you have created a valid markdown document, run `doc-checker` to ensure
that your document uses valid links. When you try to submit a change that
includes an `.md` file, Gerrit runs `doc-checker` and blocks submission if you
have broken links.

To run `doc-checker` locally, use the `fx format-code` tool:

```posix-terminal
fx format-code
```

## Headers {#headers}

### Use sentence case for page and section titles {#use-sentence-case-for-page-and-section-titles}

All titles and section headers (`#`, `##`, `###`) must use sentence case.

<span class="compare-better">Recommended</span>: Using sentence case.

```none
# This title is an example of sentence case
```

<span class="compare-worse">Not recommended</span>: Using title case:

```none
# This Title is an Example of Title Case
```

### Use dashes, not underscores, for anchors {#use-dashes-not-underscores-for-anchors}

By default, `fuchsia.dev` creates anchors using underscores (`_`) in place of
spaces. When creating a custom anchor for a section heading, use dashes (`-`)
instead of underscores, using `{#section-title}` (or `{:#section-title}`). Also,
use dashes for file names.

<span class="compare-better">Recommended</span>: Using dashes for anchors:

```none
## This is a section header {#this-is-a-section-header}
```

### Do not add custom anchors to page titles {#do-not-add-custom-anchors-to-page-titles}

The main page title (level 1 heading `#`) does not need a custom anchor.
Custom anchors should only be applied to sub-section headings (`##`, `###`).
Remove any custom anchor (such as `{#anchor-name}`) from the `# Title` line.

## Lists {#lists}

### Include empty lines between list items {#include-empty-lines-between-list-items}

In bulleted lists and numbered lists, include an empty line between items for
readability and proper rendering on fuchsia.dev. In addition, include an empty
line between a parent list item and the start of a nested sub-list.

<span class="compare-better">Recommended</span>: Add empty lines between items
and before sub-lists:

```none
* First list item.

* Second list item.

  * First nested sub-list item.

  * Second nested sub-list item.
```

<span class="compare-worse">Not recommended</span>: Consecutive list items
without empty lines:

```none
* First list item.
* Second list item.
  * First nested sub-list item.
  * Second nested sub-list item.
```

## Callouts (notes, warnings, and tips) {#callouts-notes-warnings-and-tips}

Use supported DevSite callout syntax to highlight important information on
fuchsia.dev. To create a callout box, start a paragraph with one of the
supported DevSite callout keywords followed by a colon (`:`):

* `Note: ...`
* `Caution: ...`
* `Warning: ...`
* `Important: ...`
* `Tip: ...`

Keep callout boxes concise. Notes and other callouts should typically consist of
a single paragraph or short sentences. Do not put bulleted or numbered lists
inside a callout box.

<span class="compare-better">Recommended</span>: Use DevSite callout syntax and
keep callout content concise:

```none
Note: This is an example of a concise DevSite note callout box.
```

```none
Warning: Running this command overwrites existing configuration files.
```

<span class="compare-worse">Not recommended</span>: Do not include lists inside
callouts or use unsupported alert formatting:

* Do not put bulleted or numbered lists inside a callout box.

* Do not use GitHub-style blockquote alerts (for example, `> [!NOTE]`,
  `> [!TIP]`, or `> [!WARNING]`). DevSite does not support them.

* Do not use bolded or italicized lead-ins (for example, `**Note:**` or
  `_Note:_`). These render as normal body text rather than a callout box.

  ```none
  **Note:** Do not use bolded lead-ins.
  ```

## Horizontal rules {#horizontal-rules}

Do not use `---` horizontal rules or separators in Markdown documents.
Section headings (`##`, `###`) provide sufficient visual separation and
structure on fuchsia.dev without needing decorative horizontal lines.

## Code samples {#code-samples}

### Use posix-terminal for shell command examples {#use-posix-terminal-for-shell-command-examples}

<span class="compare-better">Recommended</span>: Allow readers to easily copy
the content in a code block by adding `posix-terminal` after <code>```</code>
for a shell command.

<pre><code>
```posix-terminal
fx ota
```
</code></pre>


This code block is rendered with `$` in the front of the command:

```posix-terminal
fx ota
```

<span class="compare-worse">Not recommended</span>: Don't hardcode a `$`
character in the command.

```sh
$ fx ota
```

### Disable the copy feature {#disable-the-copy-feature}

<span class="compare-better">Recommended</span>: Add `none` or `none
{:.devsite-disable-click-to-copy}` after <code>```</code> for code or output
log examples that should not be copied.

<pre><code>
```none {:.devsite-disable-click-to-copy}
$ my_command
It won't be necessary to copy and paste this code block.
```
</code></pre>


This code block is rendered without the copy icon in the top right corner:

```none {:.devsite-disable-click-to-copy}
$ my_command
It won't be necessary to copy and paste this code block.
```

<span class="compare-worse">Not recommended</span>: Enable the copy feature for
view-only content. If you don't specify anything after <code>```</code>, the
copy feature is enabled by default.

<pre><code>
```
$ my_command
It won't be necessary to copy and paste this code block.
```
</code></pre>


This code block is rendered as below:

```
$ my_command
It won't be necessary to copy and paste this code block.
```

### Use paths instead of URLs when referring to source code {#use-paths-instead-of-urls-when-referring-to-source-code}

<span class="compare-better">Recommended</span>: Any links that refer to source
code should be referred to by path only. You will get a static error check
otherwise.

<pre>
Update the [state header][sh]
&#91;sh]: /zircon/system/ulib/inspect/include/lib/inspect/cpp/vmo/state.h
</pre>


<!-- Reference links -->

[doc-standard]: /docs/contribute/docs/documentation-standards.md
[style-guide]: /docs/contribute/docs/documentation-style-guide.md
[markdown-guide]: /docs/contribute/docs/markdown.md
[google-dev-doc-style-guide]: https://developers.google.com/style
[markdown-reference-links]: /docs/contribute/docs/markdown.md
[external-link-example]: http://example.com
[fuchsia-dev]: https://fuchsia.dev
[glossary]: /docs/glossary/README.md
