# second file

This file is referenced indirectly.

There is an [invalid link](http://{}.com/markdown)

Checks for links that are nested in paragraphs of list items

* A list item

   With a paragraph which contains an [invalid link](/docs/../../missing.md).
* A list item with a codeblock containing an invalid link
  
  This should be OK, since it is a code block, and the link is just an example.
  
  ```sh
  Some markdown [sample](https://<server>/<path>[?<query>])
  ```

* A valid mermaid diagram:

  ```mermaid
  graph TD
      A --> B
  ```

* An invalid mermaid diagram:

  ```mermaid
  invalid_type TD
      A --> B
  ```

