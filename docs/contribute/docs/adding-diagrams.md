# Adding diagrams to documentation

Diagrams are useful in documentation to help illustrate engineering concepts and
guides. However, it's important to ensure diagrams on fuchsia.dev are accessible
and updatable. This guide shows you how to add maintainable diagrams to Fuchsia
documentation.

## Types of diagrams

* Generate a simple flow diagram from text in the source code. Options:

  * [Mermaid][mermaid]: This is a simple way to maintain diagrams
    that are generated from text in the source code
  * [Sequence Diagram](https://bramp.github.io/js-sequence-diagrams/): Create
    basic flow diagrams using text in the source code
  * (Googlers only) [go/sequencediagram](http://goto.google.com/sequencediagram):
    Internal version of the Sequence Diagram tool

* Create a visual diagram in Google Drawings, Slides, or other tools

{% dynamic if user.is_googler %}

For more complex images, high-level concepts, or diagrams that require UX
design, partner with XD to create a diagram in Figma.
[File a bug](http://goto.google.com/xd-bug) to request assistance.

{% dynamic endif %}

## How to add visual diagrams

If you are creating a visual diagram for documentation on fuchsia.dev, follow
these steps:

1. Create a visual diagram in your tool of choice and either use a code block
   for tools like Mermaid or export the image as a `.svg` (suggested) or `.png`
   file.

   For images, ensure to do the following:

   * Optimize your image to ensure it loads quickly. Large images can increase
     page load times.
   * Use SVG (Scalable Vector Graphics) format whenever possible. SVGs are
     scalable, accessible, and have a smaller file size compared to other image
     formats.

2. Embed the image on fuchsia.dev using Markdown. For example:
   [see instructions](/docs/contribute/docs/markdown.md#images).

   * {mermaid}

      Note: Use the [Mermaid live editor][mermaid] to create your diagram. For
      more information about Mermaid syntax, see the
      [Mermaid documentation][mermaid-docs].

      * {markdown}

          <pre>
          {% verbatim %}
          ```mermaid
          graph LR
          A[Square Rect] -- Link text --> B((Circle))
          A --> C(Round Rect)
          B --> D{Rhombus}
          C --> D
          ```
          {% endverbatim %}
          </pre>

      * {html}

          ```none
          {% verbatim %}
          <pre class="mermaid">
            graph LR
            A[Square Rect] -- Link text --> B((Circle))
            A --> C(Round Rect)
            B --> D{Rhombus}
            C --> D
          </pre>
          {% endverbatim %}
          ```

     <p>Rendered:</p>

          <div>
            {% framebox %}
            <script src="https://cdn.jsdelivr.net/npm/mermaid@11.4.1/dist/mermaid.min.js">
            </script>

            <pre class="mermaid">
              graph LR
              A[Square Rect] -- Link text --> B((Circle))
              A --> C(Round Rect)
              B --> D{Rhombus}
              C --> D
            </pre>
            {% endframebox %}
          </div>

   * {.svg/.png}

     Note: If applicable, store your image in a `image/` directory near your Markdown
     file.

     Make sure to add alt text to the image. Alt text helps screen reader users
     understand the content of your image. For example:

      ```none
      ![Alt text](image_filename.svg)
      ```

      * Replace `Alt text` with a concise description of the image.
      * Replace `image_filename.svg` with the actual filename of your image.

      For example:

      ```none
      ![A flowchart showing the boot process of a Fuchsia device. The process starts
      with the bootloader, then moves to Zircon kernel, and finally to the user
      space.](component_framework.svg)
      ```

{% dynamic if user.is_googler %}

If you used a tool like Google Drawings or Slides to create a visual diagram,
follow these additional steps:

1. Create a bug to make the diagram maintainable on fuchsia.dev using the
   template at [go/fuchsia-diagram](http://goto.google.com/fuchsia-diagram). In
   the bug, include:

   * Author or owner (best point of contact for maintaining the diagram)
   * Link to the source file (Google drawings, Slides, Figma, etc - ensure files
     are shared with rvos)
   * Link to page on fuchsia.dev where the diagram can be found
   * Alt text description for the diagram

2. Add a comment in the markdown file with a link to the bug that contains the
   diagram source information. For example:

    Note: These bugs are intentionally internal-only, even though there
    may be instances when the diagram is visible to the public on an open source
    page. While this may not fall under the best practice, the bug links will be
    labeled “Diagram source” for transparency.

    ```none
    {% verbatim %}
    {# Diagram source: link to bug #}
    {% endverbatim %}
    ```

{% dynamic endif %}


## UX guidelines for diagrams

Diagrams are a useful tool for providing additional clarity or context that is
not easily conveyed through words alone. Effective diagrams should directly
support and enhance the accompanying text in documentation.

* **Focus:** Each diagram should have a clear focus and convey a specific
  concept or idea. Label all elements clearly and concisely.

* **Simplicity:** Strive for visual clarity. Avoid excessive complexity or
  visual clutter that might confuse the reader.

* **Consistency:** Maintain a consistent style and visual language across all
  diagrams within Fuchsia documentation.

* **Internationalization:** Keep text within diagrams to a minimum. If text is
  necessary, ensure it is translatable and localized. Use universally recognized
  symbols and icons whenever possible.

### Design considerations

* **Visual hierarchy:** Use clear and consistent visual hierarchy to guide the
  reader's eye through the diagram. Use whitespace to create visual separation
  and improve readability.

* **Color:** Use sufficient color contrast to ensure readability for users with
  visual impairments. Avoid using color as the sole means of conveying
  information.

* **Scalability:** Ensure diagrams are scalable and can be zoomed in without
  losing clarity.

### Alt text

Provide concise and informative alternative text descriptions for all diagrams. Alt text should:

* Accurately describe the content and purpose of the diagram
* Use plain language that is easy for everyone to understand
* Be concise and to the point, avoiding unnecessary detail
* If a diagram contains text, include that text in the alt text

Note: Avoid using phrases like "image of" or "diagram of" as screen readers
already identify them as images.

**Alt text examples**:

<table>
  <tr>
   <td>A flowchart showing the boot process of a Fuchsia device. The process
   starts with the bootloader, then moves to Zircon kernel, and finally to the
   user space.</td>
   <td>Image of a flowchart. (Too generic and not informative)</td>
  </tr>
  <tr>
   <td style="background-color: #d9ead3">Do
   </td>
   <td style="background-color: #f4cccc">Don’t
   </td>
  </tr>
</table>

<table>
  <tr>
   <td>A diagram illustrating the relationship between components in a Fuchsia
   component framework. The components are arranged hierarchically, with parent
   components at the top and child components at the bottom.</td>
   <td>Diagram with boxes and arrows. (Not descriptive enough)</td>
  </tr>
  <tr>
   <td style="background-color: #d9ead3">Do
   </td>
   <td style="background-color: #f4cccc">Don’t
   </td>
  </tr>
</table>

[mermaid]: https://mermaid.live/
[mermaid-docs]: https://mermaid.js.org/intro/syntax-reference.html
