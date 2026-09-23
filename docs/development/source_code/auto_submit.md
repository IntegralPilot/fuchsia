# Gerrit auto-submit

Fuchsia's Gerrit code review site supports an automatic change submission
feature. Any change that is opted in will automatically be submitted after being
approved and passing presubmit checks.

Note: Auto-submit is a Fuchsia-specific feature and its use and behavior do not
generalize to other Gerrit hosts, such as Chromium and Android, that use Commit
Queue or have their own auto-submit functionality.

## Usage

When adding reviewers in the Gerrit UI using the **REPLY** dialog, select the
**+1** for the **Fuchsia-Auto-Submit** label.

![demonstration of setting Fuchsia-Auto-Submit +1 in Gerrit](/docs/development/source_code/auto_submit_usage.gif)

After your change meets all the submit requirements (generally a **Code-Review
+2** vote and owner approval of all affected files), the auto-submit bot will
apply the **Commit-Queue +2** label. Once all presubmit checks pass, your change
will automatically be submitted.

If you want your change to land as soon as possible after approval, it's
recommended that you set **Commit-Queue +1** before (or at the same time as)
sending your change for review. When auto-submit applies the **Commit-Queue +2**
label, it will skip rerunning any checks that have already passed within the
last 24 hours, so submission often doesn't need to wait for checks to rerun.

## FAQs

### How long does it take for auto-submit to submit my change? {#latency}

Auto-submit will typically apply **Commit-Queue +2** to your change within 30
seconds of it being approved, but it may take up to 2 minutes.

Auto-submit is implemented as a job that polls Gerrit for submittable changes
every 30 seconds, but there may be occasional delays when the job restarts.

### How do I tell if a change has auto-submit enabled?

If the author of a change has opted into auto-submit, a **Fuchsia-Auto-Submit
+1** tile will appear under **Trigger Votes** in the left column of the Gerrit
UI.

![Fuchsia-Auto-Submit +1 tile](/docs/development/source_code/auto_submit_selected.png)

### My change is broken but auto-submit keeps retrying presubmit. Why? {#retries}

Auto-submit retries presubmit four times, using exponential backoff. It
intentionally assumes that any failures are false rejections due to latent
flakiness or transient breakages at HEAD. This makes auto-submit resilient to
false rejections, at the cost of occasionally retrying presubmit on CLs that are
legitimately broken and have no hope of passing presubmit checks.

If auto-submit's presubmit attempts fail four times in a row, auto-submit will
stop retrying and remove the **Fuchsia-Auto-Submit +1** label from the CL. To
continue retrying, you can manually add **Fuchsia-Auto-Submit +1** back to the
CL and it will retry presubmit four more times.

If incorrectly retrying is a concern, make sure a presubmit dry run passes
before sending your change for review, or don't use auto-submit. Alternatively,
use the
[`Multiply` directive](/docs/development/testing/testing_for_flakiness_in_cq.md)
if you're concerned about flakiness.

### I'm a reviewer on a change with auto-submit enabled. Can I approve it without submitting? {#unresolved-comments}

If you leave unresolved comments at the time you grant **Code-Review +2**, the
auto-submit bot will not submit the change until all comments are resolved.

### I need to look at FYI builder results too. Will a red FYI builder block my autosubmit?

FYI builders added through the **Choose Tryjobs** menu do not block auto-submit.
Exercise caution with FYI builders.
