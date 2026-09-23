# Gemini CLI Prompts

This directory contains a collection of shared "prompts" for use with AI
applications such as the Gemini CLI. These files are instructions designed to be
consumed by a Large Language Model (LLM) to perform specific, recurring tasks.

### Directory organization

Prompts are organized into subdirectories based on either the **target role**
(e.g., `pgm` for Program Managers) or the **task type** (e.g., `work_order` for
a generic task).

Structure:

```
tools/prompts/
├── OWNERS
├── README.md
├── pgm/
│   └── create_milestone_review.md
│   └── OWNERS
└── work_order/
│   └── flesh_out_task_description.md
│   └── OWNERS
```

### Contribution guidelines

To maintain the quality and utility of this directory, please adhere to the
following guidelines when contributing.

#### Criteria for new prompts

Avoid having this directory become a dumping ground for one-off or personal
prompts. Before adding a prompt, ensure it meets these criteria:

* **Broadly useful:** Is this prompt something that will be useful to other
  members of your team or job role?
* **Reliable:** Has the prompt been tested to ensure it produces consistent and
  helpful results?
* **Clear and maintainable:** Is the prompt's purpose clear from its filename
  and content?

#### How to Contribute

Note: **Consider a script:** Is this task better suited as a bash script, or a
more complex program? If so, this is not the right place. This directory is
for prompts that are best suited for an LLM.

1. **Find the right location:** Browse the existing subdirectories to find a
   suitable category.
2. **Create a directory (if needed):** If no suitable category exists, create
   one with a concise, descriptive name.
3. **Add Your prompt file:** Create a new `.md` file with a filename that
   clearly describes its action (e.g., `generate_rfc_from_bug.md`).
4. **Assign ownership:** Add or update the `OWNERS` file in the subdirectory.
   The owners are responsible for reviewing and maintaining the prompts in their
   directory.

### Using prompts

To use a prompt, you typically start the Gemini CLI and then reference the
prompt file using the `@` syntax to add it to the conversation's context.

Start the Gemini CLI:

```
gemini
```

Once in the interactive session, reference the file:

```
@~/fuchsia/tools/prompts/work_order/flesh_out_task_description.md and apply it to my current CL.
```
