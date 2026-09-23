# BEADS: Build Evolution with AI Directed Synthesis

## Introduction

BEADS is a backronym for Build Evolution with AI Directed Synthesis. This is the
home directory to collect and organize tools and documentation related to
AI-assisted GN to Bazel migration.

Before more information is available, please refer to the following:

- [Fuchsia AI-Assisted GN-to-Bazel Migration](https://docs.google.com/document/d/1gs24goUKSoA_TzMFDFF_WsJk_7qY2fg_Ue4DkjQAuX8)

## Bazel migration skills

Migration skills are available under [.agent/skills](.agent/skills).

To make these skills discoverable by Gemini, follow [Skill discovery and
configuration](http://go/fuchsia-skills-guide#skill-discovery-and-configuration)
from Fuchsia skills guide. One common approach is to have the following entry
in your `~/.gemini/config/skills.json`:

```json
{
  "entries": [
    {
      "path": "<FUCHSIA>/build/beads/.agent/skills/"
    }
  ]
}
```

where `<FUCHSIA>` is the root of your Fuchsia checkout directory. Note
that this path should be an absolute path.
