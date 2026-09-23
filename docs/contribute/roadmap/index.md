{% set toc2021 | yamlloads %}
{% include "docs/contribute/roadmap/2021/_toc.yaml" %}
{% endset %}

# Fuchsia roadmap

The Fuchsia project values transparency with our community. We are sharing our
roadmap to give our community insight into the projects that are being actively
explored by Fuchsia teams.

The projects listed in this roadmap are subject to change and could be modified
based on a change in priorities.

While these lists of projects are not exhaustive, they provide high-level overviews
of active projects that inform the way that we're currently thinking about
Fuchsia.

## 2025

* [Fuchsia 2025 roadmap overview](/docs/contribute/roadmap/2025/README.md)

## 2024

* [Fuchsia 2024 roadmap overview](/docs/contribute/roadmap/2024/README.md)

## 2023

* [Fuchsia 2023 roadmap overview](/docs/contribute/roadmap/2023/README.md)

## 2022

* [Fuchsia 2022 roadmap overview](/docs/contribute/roadmap/2022/README.md)

## 2021

<ul>
{% for item in toc2021.toc %}
  {% if item.path and item.title %}
    <li><a href="{{ item.path }}">{{ item.title }}</a></li>
  {% endif %}
{% endfor %}
</ul>

## 2020

* [Fuchsia 2020 roadmap overview](/docs/contribute/roadmap/2020/overview.md)
