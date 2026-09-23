# mise on Fuchsia

This mise config is an alternative to modifying your global shell config to
be aware of Fuchsia. Instead you can add mise to your global shell config and
it will automatically set up Fuchsia development tools when your shell's current
directory is inside a Fuchsia checkout.

See https://mise.jdx.dev/about.html for more information about mise.

## Setup

See https://mise.jdx.dev/getting-started.html.

Once you can use `fx` & `ffx` without [setting Fuchsia environment variables][fdev]
then you're good to go.

## Antigravity integration

If you've opened Fuchsia using `fuchsia.code-workspace`, the agent's terminal
should also be able to run `fx`/`ffx`/etc commands without additional
modifications.

### Alternative: user-global `BASH_ENV`

On some platforms Antigravity runs its shells without going through the
workspace's terminal integration and it won't have see the mise-provided
`fx`, `ffx`, etc.

In these situations, you can copy `//.mise/agent_hook.sh` to a path in your
home directory and point future shells to run it even for non-login shells
with `BASH_ENV`:

```sh
mkdir $HOME/.mise
cp $FUCHSIA_DIR/.mise/agent_hook.sh $HOME/.mise/agent_hook.sh
echo 'export BASH_ENV="$HOME/.mise/agent_hook.sh"' >> $HOME/.bashrc
```

The agent hook script should be a no-op unless it's run in a shell created by
Antigravity's agent.

You may need to log out of your shell or your desktop session and log back in
to see the effects of this change.

## Personal Settings

Some users may have personal dotfiles or other settings they previously used
`direnv` (or another tool) to manage. If you have settings, tools or env vars
you need to set up but do not want to be public in `./.mise/config.toml`, go
create a `${FUCHSIA_DIR}/mise.local.toml`. This is ignored by our `.gitignore`
and will let you have project-specific settings that are not committed.

For example: `${FUCHSIA_DIR}/mise.local.toml`


```toml
[env]
SIMPLE_GIT_PROMPT = 1
FOO = "totally-not-an-api-key"
FILE_I_EDIT_ALL_THE_TIME = "{{ config_root }}/src/BUILD.gn"

[tasks.work]
description = "Work hard"
run = "edit $FILE_I_EDIT_ALL_THE_TIME"
```

Would allow you to run `edit FILE_I_EDIT_ALL_THE_TIME` or even just `mise work`
to edit the file you edit all the time without modifying anyone else's setup.

## Security

By default, once `mise` trusts a given configuration file, it is trusted
forever, even if it is modified. From a security posture standpoint, this is
a weak posture as `mise` can run arbitrary code in it's configuration file,
which would allow an attacker to potentially compromise a system. `mise` has
a ["paranoid" mode][paranoid], which stores the SHA of trusted configuration
files and will not load them if they are changed.

You can enable paranoid mode by running `mise settings paranoid=1`.

[paranoid]: https://mise.jdx.dev/paranoid.html
[fdev]: https://fuchsia.dev/fuchsia-src/get-started/get_fuchsia_source?hl=en#set-up-environment-variables
