# Commit message style guide

The Git project provides [general guidelines][general-guidelines]{:.external}
which include how to compose commit messages. When writing a commit message,
follow these guidelines:

* [General guidelines](#general-guidelines)
* [Add a paragraph](#add-paragraph)
* [Add an associated bug](#add-bug)
* [Optionally indicate multiple steps](#indicate-multiple-steps)
* [Add tests and run them automatically multiple times](#add-tests)
* [Add a buffer line before the Change-Id](#add-buffer)
* [Use Change-Id to refer to related changes](#use-change-id)

## General guidelines {#general-guidelines}

* Add an empty line between the capitalized first line (summary of the commit)
  and the additional [detailed description](#add-paragraph).

* If possible, limit the first line to 50 characters and the
  [detailed description](#add-paragraph) to 72 characters per line.

  * The detailed description is recommended but can be omitted if the change
    is well explained by the first line.
  * The 50 characters limit is not a hard limit.

* Make use of [issue tracker integration](#add-bug), but not the in first line.
* Use the [imperative mood](https://en.wikipedia.org/wiki/Imperative_mood) to
  summarize the commit. For example, "Fix memory leak in file.cc", not "I fixed
  a memory leak the file.cc".

* Do not reference relative points in time, private URLs, individuals, private
  API keys, passwords, user names, etc.

## Add paragraph {#add-paragraph}

The paragraph underneath the header line describes the change in better detail.
Ensure the *reason and intention* of the change are clear:
[for example](https://fuchsia-review.googlesource.com/c/fuchsia/+/569681):

```none {:.devsite-disable-click-to-copy}
Adding Fuchsia Commit message style guide

This change centralizes all commit message style guide into one style
guide. It also removes duplicate content from existing pages and points
to the new style guide instead.

Change-Id: I307e5b24df4273661d22c52c81038de50600c76c
```

## Add bug {#add-bug}

If you want Fuchsia Gerrit to know what issue this change is associated with,
you need to add the `Bug: <issue-tracker-ID>` line. To associate multiple issues
with a change, list each bug in a separate line. For example:

```none {:.devsite-disable-click-to-copy}
[parent][component] Update component in Fuchsia

Write the details of a commit message here.

Bug: 82657
Bug: 82658

Test: Added test X.
```

The difference between `Bug:` and `Fixed:` is that `Fixed:` automatically closes
the issue for you once your change is submitted, whereas `Bug:` only comments on
your issue once submitted. If you have multiple changes attached to your issue,
use the `Bug:` tag for all the changes up until the final change. Use `Fixed:`
on the final change, so that the issue is closed.

## Indicate multiple steps {#indicate-multiple-steps}

When executing a change that requires multiple steps across various repositories
(for instance, to soft transition APIs defined in one repository and used in
others), indicate multiple steps by referencing the last step taken and the next
step taken.

This enables reviewers and those looking at the log to understand and navigate
the totality of the change. When possible, it is encouraged to provide all steps
to complete the migration in each commit log (but that may be impractical in
some cases).

Here's an example of a
[commit message](https://fuchsia-review.googlesource.com/c/fuchsia/+/423314)
with multiple steps:

```none {:.devsite-disable-click-to-copy}
Support for flexible enums (1/3)

Step 1 of 3. Adds support for flexible enums to fidlgen_go:

* For all enums, emit `IsUnknown()` method indicating whether the
  value is unknown (or known). While relevant only for flexible enums,
  it is possible to construct unknown strict enums using type casts.
* Emit an internal method `I_EnumIsStrict` indicating whether the enum
  is strict or flexible. This method is read by the runtime, when
  creating enum marshaler.
* For flexible enums, we generate a default unknown placeholder

Step 2: I1102f244aa5ab4545fab21218c1da90be08604ec
Step 3: If0a047a4db804a183e984676217b31e17b4af0ea

Test: fx test fidl_go_conformance at If0a047a4db804a183e984676217b31e17b4af0ea

Change-Id: Id71eb879e4d7dfabe228cc7b4e2fedb7f52db7b7
```

## Add test information {#add-tests}

### Deflake using `Multiply`

If you added new tests, you can get deflake runs automatically by adding the
`Multiply:` line with the test to run multiple times.

The following example shows `Multiply:` in the
[commit message](https://fuchsia-review.googlesource.com/c/fuchsia/+/537303):

```none {:.devsite-disable-click-to-copy}
Correct internal items' visibility Part II

This CL marks some internal items with pub(crate), pub(super) or leaves
as private. Related CL: fxr/535942

Bug: 72941
Multiply: setui_service_tests
Multiply: setui_client_interface_test

Change-Id: I67e061edee1e81a6875bf26b752ba5687c4ced71
```

Note: To specify that more than one test should be retried to deflake, you can add multiple
comma separated test names on a single line, or add multiple `Multiply:` lines as shown
in the above example. This is optional.

Developers are responsible for high-quality automated testing of their code.
Reviewers are responsible for pushing back on changes that do not include
sufficient tests. See
[Fuchsia testability rubrics](/docs/development/testing/testability_rubric.md)
for more information on how to introduce testable and tested code in the Fuchsia
project.

For more commit message options, see the
[Commit message options](/docs/development/source_code/commit_message_options.md)
guide.

### Describe testing procedures

If the testing instructions are complex, create an issue and provide a link to
that issue in the change description. If the change doesn't intend to change
behavior, indicate that fact in the commit message.

In some cases, certain behavior changes cannot be tested because Fuchsia lacks
some particular piece of infrastructure. If so, create an issue in the tracker
about the necessary infrastructure support and provide the bug number in the
change description, in addition to describing how the change is tested manually.

Sometimes, people will specify the `Test:` tag to indicate tests run on the change.
This line is only an indicator to human reviewers and has no effect on the behavior
of presubmit tests.

## Add a buffer line before Change-Id {#add-buffer}

The Change-Id gets added automatically when you commit your changes. You may
want to add a buffer line between your other lines and the Change-Id, in case
you use `git commit --amend`, so that Gerrit doesn't parse the Change-Id as a
regular line, and add a second ID to it.

For more specific details, see the
[Git interpret trailer rules](https://git-scm.com/docs/git-interpret-trailers).

## Use Change-Id to refer to related changes {#use-change-id}

To reference another Gerrit change in a commit message, always use the
Change-Id.

Using the Change-Id is preferred since:

The git SHA is only known after a change is merged, and while guidance could be
given to use the Change-Id in one case, and the git SHA in the other, prefer
uniform guidance. Furthermore, you cannot reference other repositories using the
git SHA.

The link to the change is assigned by Gerrit, and is not part of the persistent
history of the repository. Should the review mechanism change, the Change-Id
will continue to be part of the recorded history, whereas the change's number
will not. There are also rare occurrences where change numbers may be lost, for
example, due to re-indexing issues.

For instance, to refer to the change that added
[RFC-0042](/docs/contribute/governance/rfcs/0042_non_nullable_types.md), use
`I32b966810d21a249647887fa45b61720ad01714c`, and not the git SHA
`5d40ee8c42d1b0e4d8b690786da12a0a947c1aaa` or the link to the change,
<https://fuchsia-review.googlesource.com/c/fuchsia/+/284569>.

<!-- Reference links -->

[general-guidelines]:https://git-scm.com/book/en/v2/Distributed-Git-Contributing-to-a-Project
