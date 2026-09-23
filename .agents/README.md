# Agent configuration guide

This guide explains how to configure skills for agents in the Fuchsia project.

The Fuchsia project maintains agents' "global" skills in `//.agents/skills` in
`fuchsia.git`. However, you can also manage your own skill configurations using
`skills.json` files.

As you add a new skill to the Fuchsia codebase, follow these guidelines:

* **Stage 1: Sub-team**: Add a skill to a shared location in `fuchsia.git`
  (e.g. a team subdirectory) so that others can discover and use it.
* **Stage 2: Global**: Make your skill automatically available to all Fuchsia
  workspace (`fuchsia.git`) users by putting it in the global registry.

Note: If your skill is only useful to a small group of Fuchsia developers,
it does not need to be a global skill in the Fuchsia codebase.

## Stage 1: Sub-team skills

Use this stage when a skill works reliably and you want to share it with your
sub-team or group of teammates.

Add your skill to a directory or sub-directory in the `fuchsia.git` repository
outside of the top-level `//.agents/skills/` directory (for example,
`//src/devices/skills/`).

Keep in mind that agents do not automatically discover skills outside the
top-level `//.agents/skills` directory in `fuchsia.git`. Other Fuchsia
workspace users need to configure their `skills.json` files to discover and
use these skills. See
[Skill discovery and configuration](#skill-discovery-and-configuration).

## Stage 2: Global skills

Use this stage when a skill works reliably and is useful to most of the
Fuchsia team.

1. **Add the skill** to the `fuchsia.git` repository in `//.agents/skills/`.
2. **Request and receive approval** from an `OWNER` listed in the `//.agents/`
   `OWNERS` file.

## Skill discovery and configuration

Agents use JSON configuration files (`skills.json`) to discover custom skills.

### Location of skills.json

The agent reads `skills.json` from these paths (loaded in order):

1. `~/.gemini/config/skills.json` - Active globally across all workspaces
   on your machine.
2. `.agents/skills.json` - Active within this Fuchsia workspace.

### inherits vs entries

When loading large skill repositories or team configurations, avoid having
every single skill active at once. Create a `skills.json` that includes skills
using `inherits` and `entries`.

* **`inherits`**: Imports another entire JSON configuration file. Use this for
  importing a standard team config.
* **`entries`**: Points directly to a directory containing a single skill.

Both keys support `include_only` and `exclude` filters (string arrays) to
selectively parse skills.

#### Example configuration

```json
{
  "inherits": [
    {
      "path": "/path/to/fuchsia/docs/team_skills.json",
      "include_only": ["cpp-to-rust-porting"]
    }
  ],
  "entries": [
    {
      "path": "/path/to/fuchsia/src/devices/skills/custom_sensor_skill"
    }
  ]
}
```

## Managing skills with fx manage-skills

The `fx manage-skills` tool provides an interactive command-line interface
for creating and editing the `skills.json` file in your Fuchsia workspace
(specifically at `//.agents/skills.json`).

Run the following command in your shell:

```bash
fx manage-skills
```

This command scans the workspace, displays all discovered skills, and
prompts you to select the ones you want to enable. It automatically
generates or updates the `.agents/skills.json` file with your selections.

### Non-interactive mode

The `fx manage-skills` tool also supports a non-interactive mode for scripting
and quick updates using command-line flags:

*   **`--add=<path>`**: Adds a skill or container path to your configuration.
    If the path points to a single skill (containing `SKILL.md`), it is added
    without recursive inclusion. If it is a container directory, it is added
    with recursive inclusion.
*   **`--remove=<path>`**: Removes any entries in `.agents/skills.json` whose
    path matches or begins with the specified path.
*   **`--list`**: Displays the current contents of `.agents/skills.json`.
*   **`--clear`**: Removes all entries from `.agents/skills.json`.

For example, to add multiple skill paths and list the resulting configuration:

```bash
fx manage-skills --add=//src/devices/skills --add=//docs/skills
fx manage-skills --list
```
