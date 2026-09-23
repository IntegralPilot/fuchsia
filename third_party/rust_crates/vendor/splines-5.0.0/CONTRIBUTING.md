# How to contribute

<!--toc:start-->
- [How to contribute](#how-to-contribute)
  - [Prerequisites](#prerequisites)
  - [Setup](#setup)
    - [git send-email](#git-send-email)
  - [Contribution workflow](#contribution-workflow)
  - [Commit hygiene](#commit-hygiene)
<!--toc:end-->

Everyone is welcome to contribute. There is no small contributions. **Please take
the time to read this document before starting.**

## Prerequisites

Before contributing, some prerequisites:

- You must have [git] installed, as this project uses it as VCS.
- This project accepts contributions via _git patches_. A mail client that can
  send emails in plain-text mode is highly recommended — for instance, [aerc].
  More on that in the [Guidelines](#guidelines) section.
- Not mandatory but highly recommended; you should have a GPG key hosted on a
  third-party location — for instance, [keys.openpgp.org] — and sign your
  emails with it. More on that in the the [Guidelines](#guidelines) section.

## Find something to work on

Head over to <https://todo.sr.ht/~hadronized/splines> and find something to work
on. Do not hesitate to send a message to the thread(s) you want to try to notify
others you will be working there.

## Setup

Before starting up, you need to setup your tools.

### git send-email

You should follow [this link](https://git-send-email.io/) as a first source of
information on how to configure `git send-email`. Additionally, you want to
setup the per-project part.

Contributions must be sent to <~hadronized/splines@lists.sr.ht>.
Instead of using the `--to` flag everytime you use `git send-email`, you should
edit the local configuration of your repository with:

```sh
git config --local sendemail.to "~hadronized/splines@lists.sr.ht"
```

You also must set the subject prefix — that helps reviewing and it is also
mandatory for the CI to run:

```sh
git config --local format.subjectprefix "PATCH splines"
```

Once this is done, all you have to do is to use `git send-email` normally.

> Note: if you would rather go your webmail instead, **ensure it does plain
> text**, and use `git format-patch` accordingly.

## Contribution workflow

You have found something to work on and want to start contributing. Follow these
simple steps:

1. Ensure you have followed the steps in the [Setup](#setup) section.
2. Clone the repository.
3. Create a branch; it will help when sending patches upstream.
4. Make your changes and make some commits!
5. Once ready to get your changes reviewed, send them with
  `git send-email master`.
6. Wait for the review and check your inbox.

If your change was accepted, you should have an email telling you it was
applied. If not, you should repeat the process.

> Note: please use the `--annotate -v2` flag of `git send-enail` if pushing a
> new version. `-v3` for the next one, etc. It will help track progress.

## Commit hygiene

Please refrain from creating gigantic commits. I reserve the right to refuse
your patch if it’s not atomic enough: I engage my spare-time to review and
understand your code so **please** keep that in mind.

There is no limit on the number of commits per patch, but keep in mind that
individual commits should still remain small enough to be easily reviewable. Try
to scope a patch down to a single topic, or even subpart of an issue if you
think it makes sense.

Also, remember to include the issue link in your commit, and to write concise
but acute commit messages. Those are used for writing changelog, so please keep
that in mind. Keep the line width to 80-char if possible.

To include a references to an issue, just visit the issue, take its URL, and
put it in the trailing section of your commit message as `References`, such as:

```
Fix mutex bug.

References: <url>
```

Finally, **merging `master` into your branch is not appreciated**, and will end
up with your patch refused. If you want to “synchronize” your work with the
recent changes, please use `git rebase origin/master`.

[git]: https://git-scm.com/
[rustup]: https://rustup.rs/
[aerc]: https://aerc-mail.org/
[keys.openpgp.org]: https://keys.openpgp.org/
