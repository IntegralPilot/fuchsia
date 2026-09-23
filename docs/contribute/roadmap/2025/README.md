{% include "docs/contribute/roadmap/_common/_roadmap_header.md" %}

<!-- Add 2025 specific area list -->
{%- set areas | yamlloads %}
  - Platform Evolution
{%- endset %}

# Fuchsia 2025 roadmap overview

{% comment %}
The list of Fuchsia roadmap items for 2025 is generated from the information in
the following files:
/docs/contribute/roadmap/2025/_roadmap.yaml

Since this page is generated from a template, the full page is best viewed at
http://www.fuchsia.dev/fuchsia-src/contribute/roadmap/2025
{% endcomment %}

{% include "docs/contribute/roadmap/_common/_yaml_load.md" %}
{% include "docs/contribute/roadmap/_common/_roadmap_body_2025.md" %}