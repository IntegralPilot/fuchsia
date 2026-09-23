<li>
  {%- if item.workstream|length %}
    <p>{{ item.workstream }}</p>
  {%- endif %}
  {%- if item.bug|length > 0 and item.bug is defined %}
    <span class ="bug">Bugs:</span>
    <ul class="types comma-list">
      {% for bug in item.bug %}
      <li><a href="{{ url_qualifier }}{{ bug }}">{{ bug }}</a></li>
      {% endfor %}
    </ul>
  {%- endif %}
<hr class="item-divider">
</li>