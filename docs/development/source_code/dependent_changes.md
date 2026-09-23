# Dependent Changes in Gerrit

The Fuchsia project contains many separate repositories. To make
cross-repository changes easier, Fuchsia's Gerrit host supports declaring
dependencies between changes in different repositories. Dependencies are
respected during presubmit testing and change submission.

## Usage

To declare a CL's dependency on another CL, include a `Depends-on` footer in the
commit message. A CL can have multiple dependencies, spanning different
repositories and Gerrit hosts.

Format:

```none {:.devsite-disable-click-to-copy}
[foo] Do something

Depends-on: Idc82d1483b4be8480aaa87bb48af8d03cfa45858
Change-Id: Ibbf13ab7de7e4444a2dc1f52f3cad97e76c7721d
```

{% dynamic if user.is_googler %}

To make a change in turquoise-internal-review.googlesource.com depend on a
change in fuchsia-review.googlesource.com:

```none {:.devsite-disable-click-to-copy}
Depends-on: fuchsia:I9916ccaa4b95b6e9babdee33014fa6bd3d478f2e
```

To make a change in fuchsia-review.googlesource.com depend on a
change in turquoise-internal-review.googlesource.com:

```none {:.devsite-disable-click-to-copy}
Depends-on: turquoise-internal:I9916ccaa4b95b6e9babdee33014fa6bd3d478f2e
```

{% dynamic endif %}

The spelling `Depends-On` (with the "o" capitalized) is also accepted.

### Specification

* The value of each `Depends-on` footer must be a
  [Gerrit change ID](https://gerrit-review.googlesource.com/Documentation/user-changeid.html).
  Only change IDs are accepted; integer change numbers are not.
* If the dependency is on a different Gerrit host from the current CL, the host
  must be specified as a prefix (e.g.,
  `turquoise-internal:I9916ccaa4b95b6e9babdee33014fa6bd3d478f2e`).
* `Depends-on` footers *must* be in the last paragraph of the commit message,
  with no blank lines between `Depends-on` and the `Change-Id` footer.
* The `Depends-on` footer is *case-sensitive*, and will only be recognized
  if capitalized as `Depends-on` or `Depends-On`.
* If a `Depends-on` footer references a non-existent CL, a comment will be added
  to the CL.

## Behavior and Interaction with Infrastructure

* **Submission blocking:** A CL that has a `Depends-on` footer will not be
  submittable until all of its dependencies are submitted, at which point the
  "CL Deps Checker" robot will vote **Dependencies-Satisfied +1** which will
  make the CL submittable.
* **Presubmit Testing:** For easier testing, if CL A depends on CL B, then CL B
  will also be patched into the checkout in presubmit testing of CL A. The
  `Depends-on` footer is also handled recursively, so that transitive
  dependencies are respected.
* **Auto-submit:** This system will work best if the CL uploader uses Fuchsia's
  [auto-submit feature](auto_submit.md). If auto-submit is enabled on a chain of
  CLs with `Depends-on` footers, they will land in the correct order with no
  manual intervention necessary.

## Circular Dependencies (Atomic Changes) {#atomic}

Note: Circular dependencies are not supported for most repositories.

Note: Submission of circular dependencies has many edge cases, so circular
dependencies should only be used for changes that need to be landed atomically.
If the desired result can be accomplished in the same number of CLs using a
one-way dependency, please use a one-way dependency.

{% dynamic if user.is_googler %}

Circular dependencies are only allowed between
[fuchsia.git](https://fuchsia.googlesource.com/fuchsia) and
[vendor/google](https://turquoise-internal.googlesource.com/vendor/google).

{% dynamic endif %}

To use circular dependencies:

1.  Upload the two CLs with `Depends-on` footers referencing each other.
2.  Get the necessary approvals on both CLs.
3.  Set **Fuchsia-Auto-Submit +1** on both CLs once you're ready to land them.

The CL Deps Checker robot will take care of the rest, doing a presubmit dry run
on both CLs and approving them both simultaneously if the dry runs pass.

Then auto-submit will submit the CLs, and the roller into integration.git will
roll the CLs in a single commit.

{% dynamic if user.is_googler %}

### Using `fx atomic-commit`

The `fx atomic-commit` command automates the process of creating circular
dependencies. It ensures the top commits in your `fuchsia.git` and
`vendor/google` checkouts are amended to depend on each other.

To use the tool:

1.  Run `fx atomic-commit`.
2.  The script displays the commits and ask you to confirm before proceeding.
3.  After you confirm, the script amends both commits with the appropriate
    `Depends-on` footers and pushes them to create or update the CLs.
4.  Once the CLs are ready, set **Fuchsia-Auto-Submit +1** on both your CLs To
    land them atomically.

{% dynamic endif %}