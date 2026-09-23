# Command line interface (CLI) tools guidelines


## Overview

The command line is the primary interface for engineers to interact with Fuchsia
and its tools, drivers, and devices. Providing clear, consistent, and coherent
tooling is critical to ensuring better productivity and satisfaction for all the
engineers and teams developing Fuchsia. This document provides guidance for
developing and evolving CLI tools for Fuchsia.

The guidelines are divided into three main sections:

*   [Considerations](#considerations)
*   [UX guidelines](#ux-guidelines)
*   [Technical guidelines](#technical-guidelines)

<br/>

## Considerations {#considerations}

Fuchsia, and the devices running Fuchsia, make up a broad developer surface with
varying needs and requirements for users at all levels of the stack. When
developing Fuchsia tools and subtools, it’s important to ensure consistency,
architectural predictability, reliability, and ongoing support and maintenance
for end users.

Depending on the distribution scope, different tools will have different
expectations. The following guidelines and rubric will help
tool developers understand the UX considerations, technical requirements, and
documentation expectations for Fuchsia CLI tools.


Note: These guidelines focus on host-side CLI tools for Fuchsia development.
Shell scripts, test tools, target-side tools, and generic 1P or 3P tools, such
as C or Rust compilers, are out of scope.


## Audience

Fuchsia CLI tools are built, maintained, and used by many different people and
teams. A one-size-fits-all approach will not serve users who are working in
separate contexts, for example in-tree versus out-of-tree or SDK users.
Additionally, some users may have unique workflows for specific areas of
development, such as drivers, kernel, or product configuration.

When developing or updating Fuchsia tools, understanding the target audience,
their context, and their specific use cases is critical to building a better
developer experience.


## Fuchsia CLI tools

Ensuring that CLI tools are discoverable and well-documented will make it easier
for developers to find existing tools, understand workflows, and identify gaps.
In some cases, it may be more helpful to extend an existing tool to add new
features rather than creating a new subtool. For example, if you want to flash a
device over TCP, prefer adding a TCP option to `ffx target flash` rather than
creating a separate tool.


Tip: Refer to the [Tools overview](/reference/tools) to see what
Fuchsia CLI tools are currently available in the SDK and in the source code. You
can also run `ffx --help` in the command line to explore available ffx subtools,
commands, and options.


### Which tool to use?

Deciding which tool, script, or library to use will depend on your use case.
**ffx** is the CLI tool for interacting with target devices or developing
products that run Fuchsia, such as smart displays. In-tree tools like **fx** are
generally scripts that know about things outside of Fuchsia, for example, build
systems, repositories, and integrations.

> **Examples**
>
> *   Use ffx to interact with Fuchsia devices (e.g., `ffx target flash `to
> flash a Fuchsia image on a device, or `ffx component run `to interact with
> components on a device)
> *   Use fx to develop Fuchsia in tree (e.g., `fx set` to configure a build, or
> `fx test` to run tests on a host machine)


#### CLI tools to interact with or manage Fuchsia devices

*   Distributed as part of the SDK; also used by developers developing for
Fuchsia, including in-tree developers.
*   Emphasize discoverability, consistency, and stability by following UX
guidelines
*   Higher expectations for documentation, and `--help` content in order to
support developers
*   Some of these tools are used both directly by developers, and via
integration scripts or build systems.


#### In-tree tools for Fuchsia development in the platform source tree

*   For in-tree developers working in fuchsia.git
*   Emphasize low barrier of entry and allowing rapid evolution for tools,
commonly the main user is the tool author
*   Expected to change more often, may not be well-documented


#### Build system tools and scripts (e.g., cmc, fidlc)

*   Controlled exclusively through command line options and/or response files.
*   Emphasize stability in CLI to aid use in build systems.
*   Distributed as part of the SDK; the entire population of users is unknown,
and the integrations are not known/accessible.
*   Typically never run directly by developers, only run as part of a build
system.


#### Alternatives to tools

In addition to ffx, there are libraries for interacting with and scripting
interactions with Fuchsia targets from development hosts.

*   [Fuchsia Controller](/docs/contribute/governance/rfcs/0222_fuchsia_controller.md)
is a Python interface that allows developers to interact with targets via FIDL
*   [Lacewing](/docs/reference/testing/what-tests-to-write.md)
is a host-side system interaction test built on top of Fuchsia Controller for
scripting tests that cannot run purely on the target side—e.g., tests that
require rebooting the target.


## ffx overview

ffx is the top-level   tool that provides core developer functionality for
interacting with devices running Fuchsia. ffx has an extensible interface that
enables other tools which are useful for Fuchsia developers to be registered as
subtools, making it easier to discover them.

The goal of ffx is to create a cohesive tooling platform that provides a stable
command line argument surface and a standardized JSON output for commands.
Ideally, ffx subtools will be able to use common services such as configuration,
logging, error handling, and testing frameworks. This will provide a robust
collection of tools that are loosely coupled and can be adapted to specific
project needs.


### ffx subtool lifecycle

Before developing, updating, or deprecating an ffx subtool, there are several
considerations to take into account. Who is the tool for? Will it be available
in the SDK or in-tree only? Does a similar tool already exist?

Note: Internal Googlers see [Subtool lifecycle playbook](https://docs.google.com/presentation/d/1U1r6gHaBVhAYJe9Fu_tWs9SvrXyoijnGvWRRVMAiHFM/edit?usp=sharing&resourcekey=0-TOkvK6IUC8cq23l6WufjTw)
for detailed information on policy and process for developing or changing
Fuchsia tools, or making updates to the tooling platform.

When building a new tool, use the following prompts as a guide. You can also
consult the [CLI tool development rubric](#rubric) to determine whether your
tool meets Fuchsia requirements.


#### UX considerations

> Who are the intended users of the tool and how will they use it?
>
> * Where does the tool fit in the existing command structure?
>
> Can human users and scripts/other tools use the tool equally well?
>
> * Is it clearly defined which parts of the tool are intended for machine and
> automated workflows?
> * Is the machine interface well documented?
>
> Is the command interface in line with Fuchsia guidelines and best
> practices?
>
> *   Does the tool follow UX guidelines?
> *   Does the tool provide solid documentation and help for the user
> following the [CLI Help requirements](/docs/development/api/cli_help.md)?
> *   Does the tool provide clear and useful error messages?
> *   Have you considered [accessibility](/docs/concepts/accessibility/accessibility_framework.md)?
>
> Have other users tested the tool?
>
> * Ask someone outside of your team to run through the commands and outputs
>   to see if they are usable.


#### Technical requirements

> Is the tool compiled and independent of the runtime environment?
>
> * Does the tool minimize runtime link dependencies?
> * Is the tool built from the fuchsia.git source tree?
>
> Does the tool support machine interfaces?
>
> * Is there comprehensive documentation for machine interfaces?
> * Does the tool include manifest or JSON files for machine input?
>
> What are the tool’s compatibility guarantees?
>
> Does the tool collect metrics?
>
> * Is it compliant with privacy requirements for collecting metrics?


#### Product management

> How will you distribute the tool?
>
> *   Will it only be available in-tree or included in the SDK?
>
> Who will own the maintenance of the tool?
>
> *   Is there a plan in place to update, evolve, or deprecate the tool?
>
> Where can users go to find answers to common questions or ask new
> questions?
>
> How are you tracking user-reported issues?
>
> *   Does the help output and error output include a bug link?
>
> What events have been instrumented? Where can one find metrics for this
> tool?

<br/>


## CLI tool development rubric {#rubric}

Tip: Score each criterion on a scale of 1-5 (low to high) based on how well your
tool meets it. Identify areas for improvement, then prioritize enhancements
based on user needs and project goals.

<table>
  <tr>
   <td style="background-color: null">
<strong>UX Considerations</strong>
   </td>
   <td style="background-color: null"><strong>Description</strong>
   </td>
   <td style="background-color: null"><strong>Score</strong>
   </td>
  </tr>
  <tr>
   <td style="background-color: null">Target audience
   </td>
   <td style="background-color: null">Clearly defined intended users and their
   usage scenarios
   </td>
   <td style="background-color: null">(1-5)
   </td>
  </tr>
  <tr>
   <td style="background-color: null">Human & machine usability
   </td>
   <td style="background-color: null">Tool is equally usable for both human
   users and scripts/automation
   </td>
   <td style="background-color: null">
   </td>
  </tr>
  <tr>
   <td style="background-color: null">Fuchsia CLI guidelines
   </td>
   <td style="background-color: null">Aligns with established guidelines and UX
   design principles
   </td>
   <td style="background-color: null">
   </td>
  </tr>
  <tr>
   <td style="background-color: null">Help & documentation
   </td>
   <td style="background-color: null">Tool provides clear documentation and help
   output
   </td>
   <td style="background-color: null">
   </td>
  </tr>
  <tr>
   <td style="background-color: null">Error messages
   </td>
   <td style="background-color: null">Error messages are clear, actionable, and
   provide helpful context
   </td>
   <td style="background-color: null">
   </td>
  </tr>
  <tr>
   <td style="background-color: null">Accessibility
   </td>
   <td style="background-color: null">Tool design accounts for accessibility
   needs (e.g., color blindness, screen readers)
   </td>
   <td style="background-color: null">
   </td>
  </tr>
  <tr>
   <td style="background-color: null">Testing
   </td>
   <td style="background-color: null">Tool has been tested by users outside of
   the development team
   </td>
   <td style="background-color: null">
   </td>
  </tr>
  <tr>
   <td style="background-color: null">&nbsp
   </td>
  </tr>
  <tr>
   <td style="background-color: null"><strong>Technical Requirements</strong>
   </td>
   <td style="background-color: null"><strong>Description</strong>
   </td>
   <td style="background-color: null"><strong>Score</strong>
   </td>
  </tr>
  <tr>
   <td style="background-color: null">Programming language
   </td>
   <td style="background-color: null">Tool is written in C++, Rust, or Go; not
   Bash, Python, Perl, or JavaScript
   </td>
   <td style="background-color: null">(1-5)
   </td>
  </tr>
  <tr>
   <td style="background-color: null">Source tree
   </td>
   <td style="background-color: null">Tool uses the same build and dependency
   structure as the code in fuchsia.git
   </td>
   <td style="background-color: null">
   </td>
  </tr>
  <tr>
   <td style="background-color: null">Machine readability
   </td>
   <td style="background-color: null">Tool supports machine-readable interfaces
   </td>
   <td style="background-color: null">
   </td>
  </tr>
  <tr>
   <td style="background-color: null">Compatibility guarantees
   </td>
   <td style="background-color: null">Compatibility guarantees are well-defined
   </td>
   <td style="background-color: null">
   </td>
  </tr>
  <tr>
   <td style="background-color: null">Build system
   </td>
   <td style="background-color: null">Tool is unbiased towards any build system
   or environment
   </td>
   <td style="background-color: null">
   </td>
  </tr>
  <tr>
   <td style="background-color: null">Tests
   </td>
   <td style="background-color: null">Tool includes both unit tests and
   integration tests if distributed in the SDK or included in the build system
   </td>
   <td style="background-color: null">
   </td>
  </tr>
  <tr>
  <td style="background-color: null">&nbsp
  </td>
  </tr>
  <tr>
   <td style="background-color: null"><strong>Product Management</strong>
   </td>
   <td style="background-color: null"><strong>Description</strong>
   </td>
   <td style="background-color: null"><strong>Score</strong>
   </td>
  </tr>
  <tr>
   <td style="background-color: null">Maintenance
   </td>
   <td style="background-color: null">Clear ownership and maintenance plan for
   the tool
   </td>
   <td style="background-color: null">(1-5)
   </td>
  </tr>
  <tr>
   <td style="background-color: null">User support
   </td>
   <td style="background-color: null">Defined channels for user support (e.g.,
   forums, FAQs)
   </td>
   <td style="background-color: null">
   </td>
  </tr>
  <tr>
   <td style="background-color: null">Issue tracking
   </td>
   <td style="background-color: null">System in place for tracking and
   addressing user-reported issues
   </td>
   <td style="background-color: null">
   </td>
  </tr>
  <tr>
   <td style="background-color: null">Metrics
   </td>
   <td style="background-color: null">Relevant events are instrumented, and
   metrics are available
   </td>
   <td style="background-color: null">
   </td>
  </tr>
  <tr>
   <td style="background-color: null">Update/deprecation plan
   </td>
   <td style="background-color: null">Clear plan for the future of the tool
   (updates, evolution, or deprecation)
   </td>
   <td style="background-color: null">
   </td>
  </tr>
</table>


> **Key**
>
> 1. Needs significant improvement
> 2. Needs improvement
> 3. Meets minimum requirements
> 4. Exceeds expectations
> 5. Excellent

<br/>

* * *

<br/>

## UX guidelines {#ux-guidelines}

Providing clear, consistent, and coherent tooling is critical to ensuring better
productivity and satisfaction for all the engineers and teams developing
Fuchsia. These UX guidelines are designed to help people building and
integrating CLI tools create a consistent developer experience for users.
These guidelines use Fuchsia’s main developer tool, ffx, to illustrate best
practices.

Key Point: [ffx](/reference/tools/sdk/ffx) is a unified tooling platform and
command line interface (CLI) for Fuchsia development. The ffx tool is the
top-level interface for connecting to, flashing, and communicating with a device
running Fuchsia. ffx tools are used by humans and in scripts, and should be
designed for readability and ease of use.


### Design principles

Always aim to make the ffx interface as simple as possible.

*   **Put the user first:** Consider the needs of all potential users: tool
users, tool integrators, tool builders, and tool platform builders.
*   **Be clear:** Ensure the tool's purpose, usage, and feedback are easily
understandable. Provide helpful and actionable error messages and help text.
*   **Be consistent:** Maintain consistency in input/output patterns, language,
and Fuchsia core concepts.
*   **Take a holistic approach:** Ensure tools function effectively
independently and integrated with others, with consistent behavior across
contexts.
*   **Build for efficiency:** Design tools to be performant and responsive,
minimizing latency and enabling a rapid iteration cycle.
*   **Prioritize accessibility:** Use plain language and adhere to accessibility
guidelines for CLIs, ensuring usability for users with diverse abilities.
*   **Plan ahead:** Strive for flexibility, extensibility, and scalability to
accommodate future needs and growth.


### ffx user constituencies

*   **Tool users:** Direct users of ffx tools for Fuchsia workflows (either
platform or product)
*   **Tool integrators:** uses ffx tools by integrating them with other tools or
in specific environments. This includes authoring new higher level tools that
wrap ffx functionality.
*   **Tool builders:** author and maintain one or more tools hosted by ffx
*   **Tool platform builders:** evolve and maintain the underlying tooling
platform for running ffx tools


<br/>

## Command structure

ffx commands are structured as a tree under the root ffx command. This supports
a hierarchical organization which streamlines UX: instead of having to iterate
through a list of tools, users can traverse the tree of commands, eliminating
paths unrelated to their desired functionality.

Paths on the ffx command tree should follow a **noun** **noun**… **verb**
structure. Command paths consist of internal nodes that are nouns with
increasing specificity that culminate in a leaf node that is a verb, which is
the actual command.

For example, in `ffx component run <URL> `there is a root command noun (ffx), a
subtool noun (component), a subcommand verb  (run), and an argument (URL), which
is the value passed to the command when it is executed.


```
ffx component run /core/ffx-laboratory:hello-world fuchsia-pkg://fuchsia.com/hello-world-rust#meta/hello-world-rust.cm
```

<br/>

### Command structure design considerations

When developing a new CLI tool, it’s important to consider the developer
experience across the platform. Keep the number of commands under a tool
organized and reasonable. Avoid repetition, or adding unrelated commands to a
tool. Provide sensible organization of the commands in the help and
documentation.

In general, for tools that cover common workflows (such as host-to-target
interaction, system integration, and publishing), prefer extending an existing
tool rather than creating a new standalone tool. Add flags, options, or
subcommands to take advantage of shared code and functionality.

However, if the overall workflow enabled does not exist, consider a new command
or a higher level subgrouping. Review the existing command surface [reference
documentation](/reference/tools/sdk/ffx.md) to understand
where the new command or tool may fit.


### Audience

Tools may be used for different development tasks. On a large team, these roles
may be separate people. Consider which users may use a tool and cater the tool
to the audience.

> **Examples**
>
> *   Component development
> *   Driver development
> *   Fuchsia development (SDK)
> *   Build integration (GN, etc.)
> *   System integrators (e.g., on-device network tools)
> *   Publishing (from dev host to server)
> *   Deployment (from server to customers)


Tools may have different integration expectations. For example, a developer
doing component development may expect tools to integrate with their Integrated
Development Environment (IDE), while a build integration tool may be called from
a script.


### Group related commands

Related commands for Fuchsia workflows should be grouped under a common tool.
For example, ffx is organized into subtools and command groups that map to
high-level Fuchsia subsystems. This helps encourage the team toward a shared
workflow and provides a single point of discovery.


### Scope

Command line tools can vary in scope depending on user needs and goals. Create
tools that are ergonomic for their purpose. Sometimes, a simple, single-purpose
tool may be useful to automate a time-consuming process. Larger, more featureful
tools should encompass an entire task at the user (developer) level.

> **Examples**
>
> *   Avoid making a tool that accomplishes one small step of a task;
> instead design a tool that will perform a complete task.
>     *   For example, when developing a C++ application: run the preprocessor,
>     run the compiler, run the linker, start the built executable.
> *   Prefer tools that will accomplish all the steps needed by default, but
> allow for an advanced user to do a partial step.
>     *   For example, passing an argument to ask the C++ compiler to only run
> the preprocessor.


## ffx subtool design considerations

ffx subtools (previously called plugins) are organized into command groups that
map to high-level Fuchsia subsystems. In `ffx target`, ffx is the root command
and target is the subtool (subcommand). ffx subtools may have their own
arguments and options (e.g., `ffx target list --format [format] [nodename]`).

The ffx CLI should follow a standard structure and vocabulary so that users
experience ffx as an integrated whole rather than a patchwork of separate tools.
A user familiar with one ffx tool should be able to predict and understand the
command names in another tool.

When designing ffx subtools, there will be tradeoffs between complexity and
brevity. In general, ffx subtools should map to documented Fuchsia concepts or
subsystems (e.g., `target, package, bluetooth`). When possible, subtool command
groups should be limited to a primary feature or capability, with additional
options added as flags. Adding a secondary command group is discouraged, but
sometimes is unavoidable. Clarity is more important than brevity.


### Naming

Use well-known US English nouns for ffx commands, which are also known as ffx
subtools. Subcommands should be imperative verbs.

*   Well-known nouns are those that appear in documentation as common use for
Fuchsia concepts or subsystems. See the [ffx reference documentation](/reference/tools/sdk/ffx)
for a list of current ffx commands.
*   ffx subtool names should be three characters or longer.  (e.g., `ffx
bluetooth`, not `ffx bt`)
*   Commands and options should always be lowercase letters (a-z)
*   Subtool names should not contain hyphens. Options (flags) may use single
hyphens to separate words if necessary (e.g., `--log-level`).

<br/>

<table>
  <tr>
   <td>
<strong>tool</strong>
   </td>
   <td><strong>subtool</strong>
   </td>
   <td><strong>command group (1)</strong>
   </td>
   <td><strong>command group (2)</strong>
   </td>
   <td><strong>subcommand</strong>
   </td>
  </tr>
  <tr>
   <td>Top level, root, or parent command. (Noun)
   </td>
   <td>Also called subcommand (formerly plugin). Maps to a Fuchsia concept or
   subsystem. (Noun)
   </td>
   <td>Also called feature, capability, or subcommand. Primary feature related
   to a Fuchsia concept. (Noun)
   </td>
   <td>Secondary feature or capability related to the command group. (Noun)
   </td>
   <td>Direct action or command to execute. (Verb)
   </td>
  </tr>
  <tr>
   <td><code>ffx</code>
   </td>
   <td><code>emu</code>
   </td>
   <td style="background-color: #666666">
   </td>
   <td style="background-color: #666666">
   </td>
   <td><code>start</code>
   </td>
  </tr>
  <tr>
   <td><code>ffx</code>
   </td>
   <td><code>component</code>
   </td>
   <td><code>storage</code>
   </td>
   <td style="background-color: #666666">
   </td>
   <td><code>copy</code>
   </td>
  </tr>
  <tr>
   <td><code>ffx</code>
   </td>
   <td><code>target</code>
   </td>
   <td><code>update</code>
   </td>
   <td><code>channel</code>
   </td>
   <td><code>list</code>
   </td>
  </tr>
</table>

<br/>

### Structure

Follow the noun-noun-verb structure. Nest related commands as subcommands under
a parent tool.


<table>
  <tr>
   <td><code>ffx package build</code>
<p>
<code>ffx driver devices list</code>
<p>
<code>ffx component run</code>
   </td>
   <td><code>ffx-package package build </code>
<p>
<code>ffx driver list-devices</code>
<p>
<code>ffx run-component</code>
   </td>
  </tr>
  <tr>
   <td style="background-color: #d9ead3">Do
   </td>
   <td style="background-color: #f4cccc">Don’t
   </td>
  </tr>
</table>

<br/>

Don't create tools with hyphenated names, like `add-foo` and `remove-foo`.
Instead create a `foo` command that accepts `add` and `remove` subcommands.


<table>
  <tr>
   <td><code>ffx target add</code>
<p>
<code>ffx target remove</code>
<p>
<code>ffx bluetooth gap discovery start</code>
<p>
<code>ffx bluetooth gap discovery stop</code>
   </td>
   <td><code>ffx add-target</code>
<p>
<code>ffx remove-target</code>
<p>
<code>ffx bluetooth gap start-discovery</code>
<p>
<code>ffx bluetooth gap stop-discovery</code>
   </td>
  </tr>
  <tr>
   <td style="background-color: #d9ead3">Do
   </td>
   <td style="background-color: #f4cccc">Don’t
   </td>
  </tr>
</table>

<br/>

### Actions

Use clear, concise verbs that accurately reflect the command’s action.

Prefer subcommands to multiple tools that are hyphenated (e.g., avoid `foo-start,
foo-stop, foo-reset`; instead have `foo` that accepts commands
`start|stop|reset`)

<br/>

<table>
  <tr>
   <td><code>ffx emu start</code>
<p>
<code>ffx emu stop</code>
<p>
<code>ffx package archive add</code>
<p>
<code>ffx package archive remove</code>
   </td>
   <td><code>ffx emu launch</code>
<p>
<code>ffx emu halt</code>
<p>
<code>ffx package archive create-new</code>
<p>
<code>ffx package archive delete</code>
   </td>
  </tr>
  <tr>
   <td style="background-color: #d9ead3">Do
   </td>
   <td style="background-color: #f4cccc">Don’t
   </td>
  </tr>
</table>

<br/>

### Consistency

Align with established Fuchsia terminology and patterns to minimize cognitive
load. Use common verb pairings (e.g., `start/stop, add/remove, import/export`)
and follow existing ffx command patterns.

<br/>

<table>
  <tr>
   <td><strong>Common verb pairings</strong>
<p>
<code>add/remove</code>
<p>
<code>start/stop</code>
<p>
<code>get/set/unset</code>
<p>
<code>enable/disable</code>
<p>
<code>connect/disconnect</code>
<p>
<code>create/delete</code>
<p>
<code>register/deregister</code>
   </td>
   <td><strong>Standard ffx subcommands (verbs)</strong>
<p>
<code>list</code>
<p>
<code>show</code>
<p>
<code>listen</code>
<p>
<code>watch</code>
<p>
<code>test</code>
<p>
<code>run</code>
<p>
<code>log</code>
   </td>
  </tr>
</table>

<br/>

### Brevity

Aim for the shortest command and option names without sacrificing clarity or
discoverability. Command names should be at least 3 characters long.

Avoid acronyms or abbreviations with ambiguous definitions (e.g., `bt` could mean
bluetooth, Bigtable, or British Telecom on different Google products).

<br/>

<table>
  <tr>
   <td><code>ffx bluetooth</code>
   </td>
   <td><code>ffx bt</code>
   </td>
  </tr>
  <tr>
   <td style="background-color: #d9ead3">Do
   </td>
   <td style="background-color: #f4cccc">Don’t
   </td>
  </tr>
</table>

<br/>

## Command line arguments

An argument is a value that is passed to a command when it is executed.
Arguments in ffx can be exact text, ordered (also known as positional
arguments), or unordered options (also called flags).


### Options

Options (also called flags) are unordered, and can appear anywhere after the
group or command in which they're defined, including at the very end. See the
[top-level ffx options](/reference/tools/sdk/ffx).

*   Options should be at least 3 characters long and human readable
*   Options may use single hyphens to separate words if necessary. Use a single
dash between words in multi-word option names (e.g., `--log-level`)
*   Preface options with a double hyphen (--). Single hyphens (-) may be used
for single-character options, however short flags should be used sparingly to
avoid ambiguity. See [guidance on short aliases](#short-aliases)


Caution: Options should not overlap with top-level ffx commands (e.g., `ffx
target`)

<table>
  <tr>
   <td><code>--peer-target</code>
   </td>
   <td><code>--target</code>
   </td>
  </tr>
  <tr>
   <td style="background-color: #d9ead3">Do
   </td>
   <td style="background-color: #f4cccc">Don’t
   </td>
  </tr>
</table>

<br/>

Do not use uppercase letters for options. Do not use numeric options. ​If a
numeric value is needed, make a keyed option, like `--repeat <number>`.

<br/>

<table>
  <tr>
   <td><code>--timeout</code>
<p>
<code>-v, --verbose</code>
   </td>
   <td><code>-T, --timeout</code>
<p>
<code>-V, --verbose</code>
   </td>
  </tr>
  <tr>
   <td style="background-color: #d9ead3">Do
   </td>
   <td style="background-color: #f4cccc">Don’t
   </td>
  </tr>
</table>

<br/>

### Switches

The presence of a switch means the feature it represents is 'on' while its
absence means that it is 'off.' Switches default to 'off.'

*   All switches must be documented (hidden switches are not allowed)
*   Unlike keyed options, a switch does not accept a value. For example,` -v`
is a common switch meaning verbose; it doesn't take a value.

Use switches to evolve the functionality of a tool. Switches are more easily
controlled than feature flags.

<br/>

<table>
  <tr>
   <td><code>--use-new-feature</code>
<p>
<code>--no-use-new-feature</code>
   </td>
   <td><code>--config new-feature=true</code>
<p>
<code>--config new-feature=false</code>
   </td>
  </tr>
  <tr>
   <td style="background-color: #d9ead3">Do
   </td>
   <td style="background-color: #f4cccc">Don’t
   </td>
  </tr>
</table>

<br/>

Running switches together is not allowed, e.g., `-xzf` or `-vv`, each
must be separate: `-x -z -f` or `-v -v`.


### Short aliases {#short-aliases}

In general, UX recommends avoiding short flags. However, we recognize that it
can be helpful for experts to use short aliases for some frequently used
options. Overuse of short aliases can introduce confusion and ambiguity (e.g.,
does `-b` mean `--bootloader` or `--product-bundle` or `--build-dir`?). Short
aliases should be used sparingly.

*   Every option does not need an alias. When in doubt, spell it out.
*   Commands with serious irreversible consequences should have long names, so
that they are not invoked as the result of a typographical error
*   Only use lowercase letters; do not use numbers


Short aliases should be used in a consistent manner throughout the ffx CLI
(e.g., `-c` should not be shorthand for `--config` in the main ffx tool,
`--command` in one subtool, and `--font-color` in another).

<br/>

<table >
  <tr>
   <td><code>--capability</code>
<p>
<code>--console</code>
<p>
<code>--console-type</code>
   </td>
   <td><code>-c, --command</code>
<p>
<code>-c, --remote-component</code>
<p>
<code>-c, --font-color</code>
<p>
<code>-c, --cred</code>
   </td>
  </tr>
  <tr>
   <td style="background-color: #d9ead3">Do
   </td>
   <td style="background-color: #f4cccc">Don’t
   </td>
  </tr>
</table>

<br/>

### Positional arguments

Positional or ordered arguments must appear after the command name and are
identified by the order in which they appear.  Only use ordered arguments for
parameters where the sequence is crucial for understanding (e.g.,
`copy <source> <destination>`). In general, avoid positional arguments and
prefer exact text arguments with specific options.

Caution: Avoid using more than one positional argument in the same command.
Positional arguments should relate to the same type of elements (e.g., filenames)
that are processed together. Do not use positional arguments that refer to
different elements.

<table >
  <tr>
   <td><code>ffx product list --version <version></code>
   </td>
   <td><code>ffx product list <product> <version></code>
<p>
<code>ffx fuzz set <URL> <name> <value></code>
   </td>
  </tr>
  <tr>
   <td style="background-color: #d9ead3">Do
   </td>
   <td style="background-color: #f4cccc">Don’t
   </td>
  </tr>
</table>

<br/>

## Help output

The CLI help text, accessible via `--help`, is a vital communication tool for
users. It should be clear, concise, and provide essential information at a
glance as well as a path to more in-depth documentation where needed. See [CLI
tool help requirements](/docs/development/api/cli_help.md)
for more detail and examples.


### Writing help text

The terminal is a minimalistic text environment. Providing too much information
can make it difficult for users to get the help they need in the moment. Help
output should be standardized to provide users with actionable guidance and a
clear path to find more information when needed.

**Required elements**

*   Description - _A summary of the tool's function and purpose including key
information on usage._
*   Usage - _A clear outline of how to use the command, including syntax and
arguments, using < > to denote required elements and [ ] for optional ones._
*   Options - _A detailed breakdown of all options, their effects, and default
values_
*   Subcommands - _A list and summary of all available subcommands_

**Recommended elements**

*   Notes - _Important details and reminders_
*   Examples - _Illustrative examples of how to use the tool_
*   Error Codes - _A list of tool-specific errors and their meanings_

<br/>

Caution: Avoid recursive help explanations. That is, do not repurpose a
command's name to explain what it does.

<table >
  <tr>
   <td><code>doctor - Run common checks for the ffx tool and host environment</code>
   </td>
   <td><code>target - Interact with the target</code>
   </td>
  </tr>
  <tr>
   <td style="background-color: #d9ead3">Do
   </td>
   <td style="background-color: #f4cccc">Don’t
   </td>
  </tr>
</table>

<br/>

### Formatting help text

The help text should adhere to a clear structure and style for optimal
readability, including consistent indentation and line wrapping at 80
characters. The text should be written in clear, grammatically correct US
English and follow Fuchsia’s [documentation standards](/docs/contribute/docs/documentation-standards.md).

<br/>

## Error messages

Errors provide critical information to developers when things are not working as
expected. Error messages and Warnings are an opportunity to educate developers
with an accurate mental model of how a system or tool works, and its intended
use. Fuchsia error messages should help a reasonably technical user understand
and resolve an issue quickly and easily.

These guidelines apply to errors created as part of the Fuchsia platform. Errors
created by a particular runtime or language outside of Fuchsia may not follow
these guidelines.


### Writing errors

*   **State the problem** - Identify what went wrong, why it happened, and where
to fix it.
*   **Help the user fix the problem** - Offer solutions that are clear,
reasonable, and actionable. Provide links to get additional help.
*   **Write for humans** - Avoid jargon, keep the tone positive, and be concise
and consistent.

### Identify cause and suggest solutions

Users should know exactly what went wrong, where, and why. The error message
should offer a solution, next steps, or explain how the specific error can be
corrected.

<br/>

<table >
  <tr>
   <td><code>Failed to save network with SsidEmptyError. Add SSID and retry.</code>
   </td>
   <td><code>Failed to save network</code>
   </td>
  </tr>
  <tr>
   <td style="background-color: #d9ead3">Do
   </td>
   <td style="background-color: #f4cccc">Don’t
   </td>
  </tr>
</table>

<br/>

### Include shortlink

Use a shortlink to redirect users to correct documentation and assist with
troubleshooting by further explaining the problem. Follow [these guidelines](/docs/contribute/docs/shortlinks/README.md)
to create shortlinks.

<br/>

<table >
  <tr>
   <td><code>Broken pipe (os error 32) fuchsia.dev/go/<keyword> </code>
<p>
(link to an error catalog to define os error 32)
   </td>
   <td><code>Broken pipe (os error 32) </code>
   </td>
  </tr>
  <tr>
   <td style="background-color: #d9ead3">Do
   </td>
   <td style="background-color: #f4cccc">Don’t
   </td>
  </tr>
</table>

<br/>

### State requirements

Indicate if specific constraints or pre-conditions were not met. Users should
understand if an error occurred because of input validation (e.g., file not
found), processing (e.g., syntax errors in the file), or other unexpected reasons
(e.g., file corrupt, cannot read from disk).

<br/>

<table >
  <tr>
   <td><code>manifest or product_bundle must be specified</code>
   </td>
   <td><code>NotFound</code>
   </td>
  </tr>
  <tr>
   <td style="background-color: #d9ead3">Do
   </td>
   <td style="background-color: #f4cccc">Don’t
   </td>
  </tr>
</table>

<br/>

### Be specific

If the error involves values that the user can modify (text, settings, command-
line parameters, etc.), then the error message should indicate the offending
values. This makes it easier to debug the issue. However, very long values
should only be disclosed progressively or truncated.

<br/>

<table >
  <tr>
   <td><code>Path provided is not a directory</code>
   </td>
   <td><code>InvalidArgs</code>
   </td>
  </tr>
  <tr>
   <td style="background-color: #d9ead3">Do
   </td>
   <td style="background-color: #f4cccc">Don’t
   </td>
  </tr>
</table>

<br/>

### Use consistent terms and structure

Logs data can help users learn more about how and why the error occurred. Use
canonical names, categories, and values, and include clear descriptions in
documentation so an error can be easily referenced.

<br/>

<table>
  <tr>
   <td><code>No default target value</code>
   </td>
   <td><code>NotFound</code>
   </td>
  </tr>
  <tr>
   <td style="background-color: #d9ead3">Do
   </td>
   <td style="background-color: #f4cccc">Don’t
   </td>
  </tr>
</table>

<br/>

### Recommended: Create error code catalog

Including a unique identifier or standardized error code in addition to the
message can help users identify the error easily and find more information in an
error index or error catalog. For example, error codes in FIDL are always
rendered with the prefix fi- followed by a four digit code, like fi-0123.

*   See example: [FIDL compiler error catalog](/docs/reference/fidl/language/errcat.md)

<br/>

<table >
  <tr>
   <td>
<code>fi-0046: Unknown library</code>
   </td>
   <td><code>Unknown library</code>
   </td>
  </tr>
  <tr>
   <td style="background-color: #d9ead3">Do
   </td>
   <td style="background-color: #f4cccc">Don’t
   </td>
  </tr>
</table>

<br/>

* * *

<br/>

## Technical guidelines {#technical-guidelines}

To deliver a consistent developer experience across Fuchsia, CLI tools should
use standard libraries, consistent configuration, common logging and error
handling, and continuous support for users.

Tip: See Fuchsia's [CLI tool development rubric](#rubric) for a quick
checklist of considerations and guidelines.


### Programming languages

Fuchsia CLI tools may be written in C++, Rust, and Go. Tools must be compiled
and be independent of the runtime environment. Programming languages like Bash,
Python, Perl, and JavaScript are not supported.


### Runtime link dependencies

To make compiled tools easier to distribute and maintain, minimize runtime link
dependencies. Prefer to statically link dependencies instead. On Linux, it is
acceptable to runtime link against the glibc suite of libraries (libm, etc.);
other runtime link dependencies are not allowed.


### Building from source

Fuchsia tools should be built from the fuchsia.git source tree. Use the same
build and dependency structure as the code in the platform source tree. Do not
make a separate system to build tools.


### Metrics

Metrics are important to drive quality and business decisions. The type and
content of the metrics collected must be carefully chosen.

> **Questions to answer with metrics**
>
> *   Which OS are our users using? - to prioritize work for various platforms
> *   Which tools are they using? - to prioritize investments, and to learn
> which workflows are currently being used so we can prioritize investments or
> identify weak spots
> *   How often do they use a tool? - so we know how to prioritize investments,
> and to learn which workflows are currently being used so we can prioritize
> investments or identify weak spots
> *   Do our tools crash in the wild? How often? - so we know how to prioritize
> maintenance of tools
> *   How do they use a tool? - assuming that a tool can do one or more things,
> we'd like to learn how to prioritize investments in particular workflows of a
> tool

Note: For internal Googlers, every tool must file a Privacy Design Document
(PDD) in order to collect usage metrics. All tools must go through the
Google-standard PDD review process to ensure they are compliant with Google's
practices and policies. Tools must get approval on which metrics are collected
before collection.


## Configuration and environment

Tools often need to know something about the environment and context in which
they are running. This section provides guidelines on how that information
should be gathered and/or stored.


### Reading information

Tools should not attempt to gather or read settings or state files directly from
the environment in which they are running. Information such as an attached
target's IP address, the out directory for build products, or a directory for
writing temporary files should be gathered from a platform-independent source.
Separating out the code that performs platform-specific work will allow tools to
remain portable between disparate platforms.

Where practical, configuration information should be stored in a way familiar to
the user of the host machine (e.g., on Windows, use the registry). Tools should
gather information from SDK files or platform-specific tools that encapsulate
the work of reading from the Windows registry or Linux environment.

Tools should be unbiased towards any build system or environment. Accessing a
common file such as a build input dependency file is allowed.


### Writing information

Tools should not modify configuration or environment settings, except when the
tool is clearly designed for the purpose of modifying an expected portion of the
environment.

If modifying the environment outside of the tool's normal scope may help the
user, the tool may do so with the express permission of the user.


### Tests

Tools that are distributed in the SDK or included in the build system must
include tests that guarantee correct behavior. Include both unit tests and
integration tests with each tool. Tests will run in Fuchsia continuous
integration.

Note: Tests for fx tools are preferred, but not required.


### Documentation

All Fuchsia tools require documentation on how to use the tool and a
troubleshooting guide. Standard `--help` output must include:

*   **Description:** A summary of the tool's function and purpose including key
information on usage.
*   **Usage:** A clear outline of how to use the command, including syntax and
arguments, using &lt;> to denote required elements and [ ] for optional ones.
*   **Options:** A detailed breakdown of all options, their effects, and default
values
*   **Subcommands:** A list and summary of all available subcommands

More verbose usage examples and explanations should be documented in markdown on
fuchsia.dev.


## User vs. programmatic interaction

A tool may be run interactively by a human user or programmatically via a script
(or other tool).

While each tool will default to interactive or non-interactive mode if it can
determine what is preferred, it must also accept explicit instruction to run in
a given mode (e.g., allow the user to execute the programmatic interface even if
the tool is running in an interactive shell).


### Stdin

For tools that are not normally interactive, avoid requesting user input (e.g.,
readline or linenoise). Don't add an unexpected prompt to ask the user a
question.

For interactive tools (e.g., zxdb) prompting the user for input is expected.


### Stdout

When sending output to the user on stdout, use proper spelling and grammar.
Avoid unusual abbreviations. If an unusual abbreviation or term is used, be sure
it has an entry in the [glossary](/docs/glossary/README.md).


### Stderr

Use stderr for reporting invalid operation (diagnostic output) i.e. when the
tool is misbehaving. If the tool's purpose is to report issues (like a linter,
where the tool is not failing) output those results to stdout instead of stderr.


### Exit code

Exit code 0 is always treated as "no error" and exit code 1 is always a "general
error." Don’t rely on specific non-zero values. Use machine output to return a
specific error code and message. [See FIDL error catalog for an example](/docs/reference/fidl/language/errcat.md).

*   For success, return an exit code of zero
*   For failure, return a non-zero exit code

Avoid producing unnecessary output on success, unless the output includes
specific, critical information for a user to complete the next step in a
workflow (e.g., a file path). Don't print "succeeded" unless the user is asking
for verbose output.


### Logging

Logging is distinct from normal output and is usually configured to be
redirected to a file, or should be written to stderr. The audience for
logging is typically the tool developer or a user trying to debug an issue.

*   Logging from multiple threads will not interlace words within a line. The
minimum unit of output is a full text line.
*   Each line will be prefixed with an indication of the severity: `detail,
info, warning, error, fatal`


### Automation

Include a programmatic interface where reasonable to allow for automation. If
there is an existing protocol for that domain, try to follow suit (or have a
good reason not to). MachineWriter (`--machine`) can be used to support
structured output in JSON.


## Style guidelines

To promote a consistent developer experience across Fuchsia, CLI tools should
follow existing style guidelines for [programming languages](/docs/development/languages/README.md)
and [Fuchsia documentation](/docs/contribute/docs/documentation-style-guide.md). For
example, if the tool is included with Zircon and written in C++, use the style
guide for [C++ in Zircon](/docs/development/languages/c-cpp/cxx.md). Avoid creating
separate style guides for CLI tools.

All CLI tools, output, and documentation should follow the guidelines set forth
in Fuchsia’s [Respectful Code](/docs/contribute/respectful_code.md) policy. [Read more
about documentation standards on Fuchsia.](/docs/contribute/docs/documentation-standards.md)


### Case sensitivity in file paths

Don't rely on case sensitivity in file paths. Different platforms handle
uppercase and lowercase differently. Windows is case insensitive and Linux is
case sensitive. Be explicit about specific filenames. Don’t expect that
`src/BUILD` and `src/build` are different files.


### Color

You may use ANSI color in the command line interface to make text easier to read
or to highlight important information. When using color, be sure to use colors
that are distinct for readers who may not be able to see a full range of color
(e.g., color-blindness)

*   Use standard [8/16 colors](https://gist.github.com/fnky/458719343aabd01cfb17a3a4f7296797#8-16-colors),
which are easier for users to remap than [256 colors](https://gist.github.com/fnky/458719343aabd01cfb17a3a4f7296797#256-colors)
*   When possible, check whether the terminal supports color, and suppress
color output if not.
*   Always allow the user to manually suppress color output, e.g., with a
--no-color flag and/or by setting the NO\_COLOR environment variable
([no-color.org](http://no-color.org/))

Never rely solely on color to convey information. Only use color as an
enhancement. Seeing the color must not be required to correctly interpret
output. [Read more about accessibility on Fuchsia](/docs/concepts/accessibility/accessibility_framework.md).


### ASCII art

All Fuchsia tools should use standard output formatting with a consistent look
and feel. Don’t use ASCII art to format tables or otherwise enhance the output.
ASCII art can make your interface difficult to read and is not compatible with
screen readers for accessibility.
