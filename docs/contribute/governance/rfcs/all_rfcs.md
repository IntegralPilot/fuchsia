{% include "docs/contribute/governance/rfcs/_common/_rfc_header.md" %}

# List of all Fuchsia RFCs

This page lists all of the Fuchsia RFCs. For more information on the RFC process,
see [Fuchsia RFCs](/docs/contribute/governance/rfcs/README.md)

## Active RFCs

[Gerrit link](https://fuchsia-review.googlesource.com/q/dir:docs/contribute/governance/rfcs+is:open)

## Finalized RFCs

<div class="form-checkbox">
<devsite-expandable id="rfc-area">
  <h4 class="showalways">RFC area</h4>
<form id="filter-checkboxes-reset">
  {%- for area in areas %}
    {%- set found=false %}
    {%- for rfc in rfcs %}
        {%- for rfca in rfc.area %}
          {%- if rfca == area %}
            {%- set found=true %}
          {%- endif %}
        {%- endfor %}
    {%- endfor %}
    {%- if found %}
      <div class="checkbox-div">
        <input type="checkbox" id="checkbox-reset-{{ area|lower|replace(' ','-')|replace('.','-')  }}" checked>
        <label for="checkbox-reset-{{ area|lower|replace(' ','-')|replace('.','-') }}">{{ area }}</label>
      </div>
    {%- endif %}
  {%- endfor %}
  <br>
  <br>
  <button class="select-all">Select all</button>
  <button class="clear-all">Clear all</button>
  <hr>
  <div class="see-rfcs">
    <div class="rfc-left">
      <p><a href="#accepted-rfc">Accepted RFCs</a></p>
    </div>
    <div class="rfc-right">
      <p><a href="#rejected-rfc">Rejected RFCs</a></p>
    </div>
  </div>
</form>
</devsite-expandable>

<a name="accepted-rfc"><h3 class="hide-from-toc">Accepted</h3></a>
{% include "docs/contribute/governance/rfcs/_common/_index_table_header.md" %}
{%- for rfc in rfcs | sort(attribute='name') %}
    {%- if rfc.status == "Accepted" %}
        {% include "docs/contribute/governance/rfcs/_common/_index_table_body.md" %}
    {%- endif %}
{%- endfor %}
{% include "docs/contribute/governance/rfcs/_common/_index_table_footer.md" %}

<a name="rejected-rfc"><h3 class="hide-from-toc">Rejected</h3></a>
{% include "docs/contribute/governance/rfcs/_common/_index_table_header.md" %}
{%- for rfc in rfcs | sort(attribute='name') %}
    {%- if rfc.status == "Rejected" %}
        {% include "docs/contribute/governance/rfcs/_common/_index_table_body.md" %}
    {%- endif %}
{%- endfor %}
{% include "docs/contribute/governance/rfcs/_common/_index_table_footer.md" %}

{# This div is used to close the filter that is initialized above #}
</div>