{% include "fuchsia-src/contribute/roadmap/_common/_roadmap_header.md" %}

<!-- Add 2024 specific area list -->
{%- set areas | yamlloads %}
  - Platform Evolution
  - Developer Experience
{%- endset %}

# Fuchsia 2024 roadmap overview

{% comment %}
The list of Fuchsia roadmap items for 2024 is generated from the information in
the following files:
/docs/contribute/roadmap/2024/_roadmap.yaml

Since this page is generated from a template, the full page is best viewed at
http://www.fuchsia.dev/fuchsia-src/contribute/roadmap/2024
{% endcomment %}

{% include "fuchsia-src/contribute/roadmap/_common/_yaml_load.md" %}
{% include "fuchsia-src/contribute/roadmap/_common/_roadmap_body_2024.md" %}
