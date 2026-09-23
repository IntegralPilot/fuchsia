# Processing the virtio specification for AI agents

Our experiments show that AI agents working on this driver benefit from having
access to a Markdown version of the virtio specification, broken down into
subsections.

Our experiments also show that the prompt below consistently works with fronteer
models. The prompt assumes that [Python](https://www.python.org/) and
[Pandoc](https://pandoc.org/) are installed on the local machine.

```
## Goal

Obtain a copy of the virtio specification optimized for consumption by AI
agents.

## Source

The latest virtio specification is at
https://docs.oasis-open.org/virtio/virtio/v1.4/virtio-v1.4.html

## Process

Follow the steps below to obtain a cleaned up specification.

1. Create a `local/virtio-spec/` directory, if it does not already exist.

2. Download the source specification to the `local/virtio-spec/` directory, as
   `all.html`.

3. Use the `pandoc` invocation pattern below to convert the specification to
   Markdown, and save it as `local/virtio-spec/all.md`.

    ```bash
    pandoc --from=html --to=markdown-smart --wrap=auto --markdown-headings=atx \
        --columns=80 all.html -o all.md
    ```

4. Read the specification's table of contents, which is roughly at the beginning
   of the file.

5. Write a shell command or Python script that extracts the table of contents to
   `local/virtio-spec/toc.md`.

   * Validation: Run the script and read the output file.
   * Iteration: If necessary, fix the script and iterate.

6. Write a shell command or Python script that extracts each top-level section
   to `local/virtio-spec/section{section_number_or_letter}.md`.

   * Example: section 3 goes to `local/virtio-spec/section3.md`.
   * Validation: run the script and read the first, second, next-to-last and
     last output file.
   * Iteration: If necessary, fix the script and iterate.

7. Write a shell command or Python script that extracts each transport subsection
   to `local/virtio-spec/transport-{transport_type}-section{section_and_subsection}.md`.

   * Examples: `local/virtio-spec/transport-pci-section4_1.md`,
     `local/virtio-spec/transport-mmio-section4_2.md`.
   * Validation: run the script and read the first, second, next-to-last and
     last output file.
   * Iteration: If necessary, fix the script and iterate.

8. Write a shell command or Python script that extracts each device subsection
   to `local/virtio-spec/{device_type}-section{section_and_subsection}.md`.

   * Examples: `local/virtio-spec/net-section5_1.md`,
     `local/virtio-spec/block-section5_2.md`.
   * Validation: run the script and read the first, second, next-to-last and
     last output file.
   * Iteration: If necessary, fix the script and iterate.

9. Write a summary of the directory's contents in
   `local/virtio-spec/overview.md`. Your primary audience is AI agents.
```
