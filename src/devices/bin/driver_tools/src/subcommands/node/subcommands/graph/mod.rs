// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub mod args;
use crate::subcommands::node::common;
use crate::subcommands::node::subcommands::graph::args::GraphOrientation;

use anyhow::{Result, format_err};
use args::GraphNodeCommand;
use flex_fuchsia_driver_development as fdd;
#[cfg(feature = "fdomain")]
use fuchsia_driver_dev_fdomain as fuchsia_driver_dev;
use itertools::Itertools;
use safe_string::DotSafe;
use std::collections::{BTreeMap, HashMap};

pub struct ClusterInfo {
    pub koid: u64,
    pub driver_url: String,
}

const TAB: &str = "    ";

const DIGRAPH_PREFIX: &str = r#"digraph {
    forcelabels = true; splines="ortho"; ranksep = 5; nodesep = 1;
    node [ shape = "box" color = " #0a7965" penwidth = 2.25 fontname = "prompt medium" fontsize = 10 margin = 0.22 ];
    edge [ color = " #283238" penwidth = 1 style = solid fontname = "roboto mono" fontsize = 10 ];"#;

pub async fn graph_node(
    cmd: GraphNodeCommand,
    writer: &mut dyn std::io::Write,
    driver_development_proxy: fdd::ManagerProxy,
) -> Result<()> {
    if cmd.generate_html {
        return generate_html(writer, cmd.svg_path);
    }

    let nodes = fuchsia_driver_dev::get_device_info(&driver_development_proxy, &[], false).await?;
    let nodes = common::filter_nodes(nodes, cmd.filter)?;
    let node_map = common::create_node_map(&nodes)?;

    writeln!(writer, "{}", DIGRAPH_PREFIX)?;
    match cmd.orientation {
        GraphOrientation::TopToBottom => writeln!(writer, r#"{TAB}rankdir = "TB""#).unwrap(),
        GraphOrientation::LeftToRight => writeln!(writer, r#"{TAB}rankdir = "LR""#).unwrap(),
    };

    let mut grouped_nodes: HashMap<u64, HashMap<String, Vec<&fdd::NodeInfo>>> = HashMap::new();
    let mut unbound_nodes: Vec<&fdd::NodeInfo> = vec![];
    for node in &nodes {
        let cluster_info = get_cluster_info(node, &node_map);
        match cluster_info {
            Some(info) => {
                grouped_nodes
                    .entry(info.koid)
                    .or_default()
                    .entry(info.driver_url)
                    .or_default()
                    .push(node);
            }
            None => {
                unbound_nodes.push(node);
            }
        };
    }

    let mut edge_ids = vec![];

    let mut service_edges: Vec<(&fdd::NodeInfo, &fdd::NodeInfo, Vec<String>)> = vec![];
    if cmd.services {
        for node in nodes.iter() {
            let offers_converted = node.offer_list.as_ref().map(|offers| {
                offers
                    .iter()
                    .filter_map(|offer| {
                        if let fidl_fuchsia_component_decl::Offer::Service(svc) = offer {
                            let Some(fidl_fuchsia_component_decl::Ref::Child(ref child_ref)) =
                                svc.source
                            else {
                                return None;
                            };

                            let source_moniker = &child_ref.name;
                            return nodes
                                .iter()
                                .find(|node| node.moniker.as_ref() == Some(source_moniker))
                                .map(|node| (svc.clone(), node));
                        } else {
                            None
                        }
                    })
                    .collect::<Vec<_>>()
            });

            if let Some(offers) = offers_converted {
                for (offer_svc, source_node) in offers {
                    let edge_string = get_labeled_graph_edge(
                        &mut edge_ids,
                        source_node,
                        node,
                        format!(
                            "{}({})",
                            offer_svc.source_name.as_deref().expect("name"),
                            offer_svc
                                .renamed_instances
                                .unwrap_or_default()
                                .iter()
                                .map(|e| format!(
                                    "{}-->{}",
                                    e.source_name.clone(),
                                    e.target_name.clone()
                                ))
                                .collect::<Vec<_>>()
                                .join(",")
                        )
                        .as_str(),
                    )?;

                    service_edges.push((source_node, node, edge_string));
                }
            }
        }
    }

    for (koid, cluster_drivers) in grouped_nodes {
        writeln!(writer, "{TAB}subgraph \"cluster_{}\" {{", koid)?;
        writeln!(writer, "{TAB}{TAB}label = \"Host {}\";", koid)?;
        writeln!(writer, "{TAB}{TAB}style = \"filled,rounded\";")?;
        writeln!(writer, r#"{TAB}{TAB}fillcolor = " #b1b9be";"#)?;

        for (driver, cluster_nodes) in &cluster_drivers {
            // Get just the last bit of the url.
            let driver_name = driver.rsplit_once('/').unwrap_or(("", &driver)).1;
            let safe_driver_name = DotSafe::from_str_lossy(driver_name);
            writeln!(writer, "{TAB}{TAB}subgraph \"cluster_{}_{}\" {{", koid, safe_driver_name)?;
            writeln!(writer, "{TAB}{TAB}{TAB}label = \"{}\";", safe_driver_name)?;
            writeln!(writer, "{TAB}{TAB}{TAB}style = \"filled,rounded\";")?;
            writeln!(writer, r#"{TAB}{TAB}{TAB}fillcolor = " #dce0e3";"#)?;

            for node in cluster_nodes {
                print_graph_node(node, writer, format!("{TAB}{TAB}{TAB}").as_str())?;
            }

            service_edges.retain(|(service_edge_src, service_edge_target, edge_string)| {
                if cluster_nodes.iter().any(|n| n == service_edge_src)
                    && cluster_nodes.iter().any(|n| n == service_edge_target)
                {
                    for entry in edge_string {
                        writeln!(writer, "{TAB}{TAB}{}", entry).unwrap();
                    }

                    false
                } else {
                    true
                }
            });

            writeln!(writer, "{TAB}{TAB}}}")?;
        }

        service_edges.retain(|(service_edge_src, service_edge_target, edge_string)| {
            if cluster_drivers
                .iter()
                .any(|(_, cluster_nodes)| cluster_nodes.iter().any(|n| n == service_edge_src))
                && cluster_drivers.iter().any(|(_, cluster_nodes)| {
                    cluster_nodes.iter().any(|n| n == service_edge_target)
                })
            {
                for entry in edge_string {
                    writeln!(writer, "{TAB}{}", entry).unwrap();
                }
                false
            } else {
                true
            }
        });

        writeln!(writer, "{TAB}}}")?;
    }

    for unbound_node in unbound_nodes {
        print_graph_node(unbound_node, writer, format!("{TAB}").as_str())?;
    }

    for node in nodes.iter() {
        if let Some(child_ids) = &node.child_ids {
            for id in child_ids.iter().rev() {
                if let Some(child) = node_map.get(&id) {
                    print_graph_edge(&mut edge_ids, node, writer, child)?;
                }
            }
        }
    }

    for (_, _, edge_string) in &service_edges {
        for entry in edge_string {
            writeln!(writer, "{}", entry).unwrap();
        }
    }

    writeln!(writer, "}}")?;

    Ok(())
}

fn generate_html(writer: &mut dyn std::io::Write, svg: Option<String>) -> Result<()> {
    let mut svg_content = match svg {
        Some(svg) => std::fs::read_to_string(svg)?,
        None => std::io::read_to_string(std::io::stdin())?,
    };

    let re_edges = regex_lite::Regex::new(r#"<g id="(\d+_\d+)" class=""#)?;
    let edge_caps = re_edges
        .captures_iter(svg_content.as_str())
        .map(|caps| caps.get(1).map(|the_match| the_match.as_str().to_string()).expect("match1"))
        .collect::<Vec<_>>();

    let re_services = regex_lite::Regex::new(r#"<g id="(\d+)_(\d+)_(.*)_([xyz])" class=""#)?;
    let svc_caps = re_services
        .captures_iter(svg_content.as_str())
        .map(|caps| {
            (
                caps.get(1).map(|the_match| the_match.as_str().to_string()).expect("match1"),
                caps.get(2).map(|the_match| the_match.as_str().to_string()).expect("match2"),
                caps.get(3).map(|the_match| the_match.as_str().to_string()).expect("match3"),
                caps.get(4).map(|the_match| the_match.as_str().to_string()).expect("match4"),
            )
        })
        .collect::<Vec<_>>();

    let mut ids_map = HashMap::new();
    for (start, end, proto, suffix) in svc_caps {
        ids_map.insert(
            format!("\"{}_{}_{}_{}\"", start, end, proto, suffix),
            vec![
                format!("\"{}_{}_{}_x\"", start, end, proto), // intermediate node
                format!("\"{}_{}_{}_y\"", start, end, proto), // source to intermediate path
                format!("\"{}_{}_{}_z\"", start, end, proto), // intermediate to target path
            ],
        );
    }

    let i = svg_content.find("viewBox=").expect("viewbox");
    svg_content.insert_str(i, "id=\"svgImage\" ");

    writeln!(
        writer,
        r#"<!DOCTYPE html>
<html>
<style>
body {{
  overflow: hidden;
}}
</style>
<head>
    <title>Fuchsia Driver Node Graph</title>
</head>

<body>
    <h1>Node Graph</h1>
    <h2>Instructions:</h2>
    <p>
There are three layers of boxes represented. The driver host process, the driver component, and individual nodes. The graph edges represent parent-child relationships with the edges that end in boxes. The graph edges represent service routes with edges that end with arrows. You can hover over service edges to highlight them temporarily, or click them to select them to help when panning. The graph can be panned and zoomed with a mouse or trackpad.<br>
    </p>
    <span id="zoomValue">Zoom scale: 1</span>
    <div id="svgContainer">
    {}
    </div>
    <script>
        "#,
        svg_content
    )?;

    let simple_ids = edge_caps.iter().map(|id| format!("\"{}\"", id)).join(",\n    ");

    let ids = ids_map
        .iter()
        .map(|(k, v)| (k, v.join(",")))
        .map(|(k, v)| format!("[{}, [{}]]", k, v))
        .join(",\n    ");

    writeln!(
        writer,
        r#"
const svgImage = document.getElementById("svgImage");
const svgContainer = document.getElementById("svgContainer");

var viewBox = {{x:0,y:0,w:svgImage.clientWidth,h:svgImage.clientHeight}};
svgImage.setAttribute('viewBox', `${{viewBox.x}} ${{viewBox.y}} ${{viewBox.w}} ${{viewBox.h}}`);
const svgSize = {{w:svgImage.clientWidth,h:svgImage.clientHeight}};
var isPanning = false;
var startPoint = {{x:0,y:0}};
var endPoint = {{x:0,y:0}};;
var scale = 1;

function isTrackPad(e) {{
    var isTrackpad = false;
    if (e.wheelDeltaY) {{
        if (e.wheelDeltaY === (e.deltaY * -3)) {{
            isTrackpad = true;
        }}
    }}
    else if (e.deltaMode === 0) {{
        isTrackpad = true;
    }}

    return isTrackpad;
}}


svgContainer.onwheel = function(e) {{
    e.preventDefault();
    var w = viewBox.w;
    var h = viewBox.h;
    var mx = e.offsetX;//mouse x
    var my = e.offsetY;
    var dw = w*Math.sign(e.deltaY)*0.05;
    var dh = h*Math.sign(e.deltaY)*0.05;
    if (!isTrackPad(e)) {{
        dw = -dw;
        dh = -dh;
    }}
    var dx = dw*mx/svgSize.w;
    var dy = dh*my/svgSize.h;
    viewBox = {{x:viewBox.x+dx,y:viewBox.y+dy,w:viewBox.w-dw,h:viewBox.h-dh}};
    scale = svgSize.w/viewBox.w;
    zoomValue.innerText = `Zoom scale: ${{Math.round(scale*100)/100}}`;
    svgImage.setAttribute('viewBox', `${{viewBox.x}} ${{viewBox.y}} ${{viewBox.w}} ${{viewBox.h}}`);
}}


svgContainer.onmousedown = function(e){{
    isPanning = true;
    startPoint = {{x:e.x,y:e.y}};
}}

svgContainer.onmousemove = function(e){{
    if (isPanning) {{
        endPoint = {{x:e.x,y:e.y}};
        var dx = (startPoint.x - endPoint.x)/scale;
        var dy = (startPoint.y - endPoint.y)/scale;
        var movedViewBox = {{x:viewBox.x+dx,y:viewBox.y+dy,w:viewBox.w,h:viewBox.h}};
        svgImage.setAttribute('viewBox', `${{movedViewBox.x}} ${{movedViewBox.y}} ${{movedViewBox.w}} ${{movedViewBox.h}}`);
   }}
}}

svgContainer.onmouseup = function(e){{
    if (isPanning) {{
        endPoint = {{x:e.x,y:e.y}};
        var dx = (startPoint.x - endPoint.x)/scale;
        var dy = (startPoint.y - endPoint.y)/scale;
        viewBox = {{x:viewBox.x+dx,y:viewBox.y+dy,w:viewBox.w,h:viewBox.h}};
        svgImage.setAttribute('viewBox', `${{viewBox.x}} ${{viewBox.y}} ${{viewBox.w}} ${{viewBox.h}}`);
        isPanning = false;
   }}
}}

svgContainer.onmouseleave = function(e){{
 isPanning = false;
}}

// Define all colors. Indexes are:
// 0 = intermediate_node
// 1 = source_to_intermediate path
// 2 = intermediate_to_target path and arrowhead
// 3 = simple edges (parent-child)

const originalStrokes = ['#0a7965', '#566168', '#566168', '#283238'];
const originalFills = ['none', '#566168', '#566168', '#283238'];
const originalText = 'none';

const hoverStrokes = ['#faa500', '#faa500', '#faa500', '#faa500'];
const hoverFills = ['#faa500', '#faa500', '#faa500', '#faa500'];
const hoverText = 'none';

const highlightStrokes = ['#c7241f', '#c7241f', '#c7241f', '#c7241f'];
const highlightFills = ['#c7241f', '#c7241f', '#c7241f', '#c7241f'];
const highlightText = '#ffffff';

const simple_ids = [
    {simple_ids}
]

// Map the edge to all the elements that should be highlighted.
const ids = [
    {ids}
];

function normalizeColor(colorString) {{
  const ctx = document.createElement('canvas').getContext('2d');
  ctx.fillStyle = colorString;
  return ctx.fillStyle;
}}

simple_ids.forEach(id => {{
    const edgeTrigger = document.getElementById(id);

    edgeTrigger.addEventListener('click', function () {{
        let isHighlighted = normalizeColor(edgeTrigger.querySelector('path').style.stroke) == normalizeColor(highlightStrokes[3]);
        if (!isHighlighted) {{
            edgeTrigger.querySelectorAll('polygon').forEach(p => {{
                p.style.stroke = highlightStrokes[3];
                p.style.fill = highlightFills[3];
            }});

            edgeTrigger.querySelectorAll('polyline').forEach(p => {{
                p.style.stroke = highlightStrokes[3];
                p.style.fill = highlightFills[3];
            }});

            edgeTrigger.querySelector('path').style.stroke = highlightStrokes[3];
        }} else {{
             edgeTrigger.querySelectorAll('polygon').forEach(p => {{
                p.style.stroke = originalStrokes[3];
                p.style.fill = originalFills[3];
            }});

            edgeTrigger.querySelectorAll('polyline').forEach(p => {{
                p.style.stroke = originalStrokes[3];
                p.style.fill = originalFills[3];
            }});

            edgeTrigger.querySelector('path').style.stroke = originalStrokes[3];
        }}
    }});

    edgeTrigger.addEventListener('mouseenter', function () {{
        let isHighlighted = normalizeColor(edgeTrigger.querySelector('path').style.stroke) == normalizeColor(highlightStrokes[3]);
        if (isHighlighted) {{ return; }}

        edgeTrigger.querySelectorAll('polygon').forEach(p => {{
            p.style.stroke = hoverStrokes[3];
            p.style.fill = hoverFills[3];
        }});

        edgeTrigger.querySelectorAll('polyline').forEach(p => {{
            p.style.stroke = hoverStrokes[3];
            p.style.fill = hoverFills[3];
        }});

        edgeTrigger.querySelector('path').style.stroke = hoverStrokes[3];
    }});

    edgeTrigger.addEventListener('mouseleave', function () {{
        let isHighlighted = normalizeColor(edgeTrigger.querySelector('path').style.stroke) == normalizeColor(highlightStrokes[3]);
        if (isHighlighted) {{ return; }}

        edgeTrigger.querySelectorAll('polygon').forEach(p => {{
            p.style.stroke = originalStrokes[3];
            p.style.fill = originalFills[3];
        }});

        edgeTrigger.querySelectorAll('polyline').forEach(p => {{
            p.style.stroke = originalStrokes[3];
            p.style.fill = originalFills[3];
        }});

        edgeTrigger.querySelector('path').style.stroke = originalStrokes[3];
    }});
}});


ids.forEach(id => {{
    const edgeTrigger = document.getElementById(id[0]);

    edgeTrigger.addEventListener('click', function () {{
        let intermediate_node = id[1][0];
        let source_to_intermediate = id[1][1];
        let intermediate_to_target = id[1][2];

        let isHighlighted = normalizeColor(document.getElementById(intermediate_node).querySelector('ellipse').style.stroke) == normalizeColor(highlightStrokes[0]);
        if (!isHighlighted) {{
            document.getElementById(intermediate_node).querySelector('ellipse').style.stroke = highlightStrokes[0];
            document.getElementById(intermediate_node).querySelector('ellipse').style.fill = highlightFills[0];
            document.getElementById(intermediate_node).querySelector('text').style.stroke = highlightText;

            document.getElementById(source_to_intermediate).querySelector('polygon').style.stroke = highlightStrokes[1];
            document.getElementById(source_to_intermediate).querySelector('polygon').style.fill = highlightFills[1];
            document.getElementById(source_to_intermediate).querySelector('path').style.stroke = highlightStrokes[1];

            document.getElementById(intermediate_to_target).querySelector('polygon').style.stroke = highlightStrokes[2];
            document.getElementById(intermediate_to_target).querySelector('polygon').style.fill = highlightFills[2];
            document.getElementById(intermediate_to_target).querySelector('path').style.stroke = highlightStrokes[2];
        }} else {{
            document.getElementById(intermediate_node).querySelector('ellipse').style.stroke = originalStrokes[0];
            document.getElementById(intermediate_node).querySelector('ellipse').style.fill = originalFills[0];
            document.getElementById(intermediate_node).querySelector('text').style.stroke = originalText;

            document.getElementById(source_to_intermediate).querySelector('polygon').style.stroke = originalStrokes[1];
            document.getElementById(source_to_intermediate).querySelector('polygon').style.fill = originalFills[1];
            document.getElementById(source_to_intermediate).querySelector('path').style.stroke = originalStrokes[1];

            document.getElementById(intermediate_to_target).querySelector('polygon').style.stroke = originalStrokes[2];
            document.getElementById(intermediate_to_target).querySelector('polygon').style.fill = originalFills[2];
            document.getElementById(intermediate_to_target).querySelector('path').style.stroke = originalStrokes[2];
        }}
    }});

    edgeTrigger.addEventListener('mouseenter', function () {{
        let intermediate_node = id[1][0];
        let source_to_intermediate = id[1][1];
        let intermediate_to_target = id[1][2];

        let isHighlighted = normalizeColor(document.getElementById(intermediate_node).querySelector('ellipse').style.stroke) == normalizeColor(highlightStrokes[0]);
        if (isHighlighted) {{ return; }}

        document.getElementById(intermediate_node).querySelector('ellipse').style.stroke = hoverStrokes[0];
        document.getElementById(intermediate_node).querySelector('ellipse').style.fill = hoverFills[0];
        document.getElementById(intermediate_node).querySelector('text').style.stroke = hoverText;

        document.getElementById(source_to_intermediate).querySelector('polygon').style.stroke = hoverStrokes[1];
        document.getElementById(source_to_intermediate).querySelector('polygon').style.fill = hoverFills[1];
        document.getElementById(source_to_intermediate).querySelector('path').style.stroke = hoverStrokes[1];

        document.getElementById(intermediate_to_target).querySelector('polygon').style.stroke = hoverStrokes[2];
        document.getElementById(intermediate_to_target).querySelector('polygon').style.fill = hoverFills[2];
        document.getElementById(intermediate_to_target).querySelector('path').style.stroke = hoverStrokes[2];
    }});

    edgeTrigger.addEventListener('mouseleave', function () {{
        let intermediate_node = id[1][0];
        let source_to_intermediate = id[1][1];
        let intermediate_to_target = id[1][2];

        let isHighlighted = normalizeColor(document.getElementById(intermediate_node).querySelector('ellipse').style.stroke) == normalizeColor(highlightStrokes[0]);
        if (isHighlighted) {{ return; }}

        document.getElementById(intermediate_node).querySelector('ellipse').style.stroke = originalStrokes[0];
        document.getElementById(intermediate_node).querySelector('ellipse').style.fill = originalFills[0];
        document.getElementById(intermediate_node).querySelector('text').style.stroke = originalText;

        document.getElementById(source_to_intermediate).querySelector('polygon').style.stroke = originalStrokes[1];
        document.getElementById(source_to_intermediate).querySelector('polygon').style.fill = originalFills[1];
        document.getElementById(source_to_intermediate).querySelector('path').style.stroke = originalStrokes[1];

        document.getElementById(intermediate_to_target).querySelector('polygon').style.stroke = originalStrokes[2];
        document.getElementById(intermediate_to_target).querySelector('polygon').style.fill = originalFills[2];
        document.getElementById(intermediate_to_target).querySelector('path').style.stroke = originalStrokes[2];
    }});
}});
    "#
    )?;

    writeln!(
        writer,
        r#"    </script>

</body>
</html>
"#
    )?;

    Ok(())
}

fn print_graph_node(
    node: &fdd::NodeInfo,
    writer: &mut dyn std::io::Write,
    prefix: &str,
) -> Result<()> {
    let moniker = node.moniker.as_ref().ok_or_else(|| format_err!("Node missing moniker"))?;
    let (_, name) = moniker.rsplit_once('.').unwrap_or(("", &moniker));
    let node_id = node.id.as_ref().ok_or_else(|| format_err!("Node missing id"))?;

    writeln!(
        writer,
        "{}\"{}\" [label=\"{}\", id = \"{}\"]",
        prefix,
        node_id,
        DotSafe::from_str_lossy(name),
        node_id
    )?;
    Ok(())
}

fn print_graph_edge(
    edge_ids: &mut Vec<String>,
    node: &fdd::NodeInfo,
    writer: &mut dyn std::io::Write,
    child: &fdd::NodeInfo,
) -> Result<()> {
    let start_id = node.id.as_ref().ok_or_else(|| format_err!("Node missing id"))?;
    let end_id = child.id.as_ref().ok_or_else(|| format_err!("Child node missing id"))?;

    edge_ids.push(format!("{}_{}", start_id, end_id));

    writeln!(
        writer,
        "{TAB}\"{}\" -> \"{}\" [arrowhead = box id = \"{}_{}\"]",
        start_id, end_id, start_id, end_id
    )?;
    Ok(())
}

fn get_labeled_graph_edge(
    edge_ids: &mut Vec<String>,
    node: &fdd::NodeInfo,
    child: &fdd::NodeInfo,
    label: &str,
) -> Result<Vec<String>> {
    let start_id = node.id.as_ref().ok_or_else(|| format_err!("Node missing id"))?;
    let end_id = child.id.as_ref().ok_or_else(|| format_err!("Child node missing id"))?;

    let sanitized_label: String =
        label.chars().map(|c| if c.is_alphanumeric() { c } else { '_' }).collect();
    let intermediate_node_id = format!("intermediate_{}_{}_{}", start_id, end_id, sanitized_label);

    let id_group = format!("{}_{}_{}", start_id, end_id, sanitized_label);
    edge_ids.push(format!("{}_x", id_group));
    edge_ids.push(format!("{}_y", id_group));
    edge_ids.push(format!("{}_z", id_group));

    Ok(vec![
        format!(
            "{TAB}\"{}\" [shape=oval, style=\"dotted\", label=\"{}\", id = \"{}_x\"];",
            intermediate_node_id,
            DotSafe::from_str_lossy(label),
            id_group
        ),
        format!(
            "{TAB}\"{}\" -> \"{}\" [dir=back arrowtail = inv, color = \" #566168\" penwidth = 1 style = solid, id = \"{}_y\"];",
            start_id, intermediate_node_id, id_group
        ),
        format!(
            "{TAB}\"{}\" -> \"{}\" [arrowhead = normal color = \" #566168\" penwidth = 1 style = solid, id = \"{}_z\"]",
            intermediate_node_id, end_id, id_group
        ),
    ])
}

/// For a given node, traverse up the tree to find the driver host koid and the driver URL
/// that owns the node.
fn get_cluster_info(
    node: &fdd::NodeInfo,
    node_map: &BTreeMap<u64, fdd::NodeInfo>,
) -> Option<ClusterInfo> {
    let mut koid = node.driver_host_koid;
    let (_, mut url) = common::get_state_and_owner(node.quarantined, &node.bound_driver_url, false);

    if url == "none" {
        return None;
    }

    let mut curr = node;
    while koid.is_none() || url == "parent" || url == "composite(s)" {
        let primary_parent_id = find_primary_parent(curr, node_map)?;
        curr = &node_map[primary_parent_id];

        let (_, owner) =
            common::get_state_and_owner(curr.quarantined, &curr.bound_driver_url, false);

        if koid.is_none() {
            koid = curr.driver_host_koid;
        }
        if url == "parent" || url == "composite(s)" {
            url = owner;
        }
    }

    Some(ClusterInfo { koid: koid.expect("koid should not be none"), driver_url: url })
}

/// Find the primary parent of a node. The primary parent is the parent that is a prefix of the
/// node's moniker.
fn find_primary_parent<'a>(
    node: &'a fdd::NodeInfo,
    node_map: &'a BTreeMap<u64, fdd::NodeInfo>,
) -> Option<&'a u64> {
    let moniker = node.moniker.as_ref()?;
    node.parent_ids.as_ref()?.iter().find(|parent_id: &&u64| {
        if let Some(parent_node) = node_map.get(parent_id) {
            if let Some(parent_moniker) = &parent_node.moniker {
                return moniker.starts_with(parent_moniker);
            }
        }
        false
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Context;
    use argh::FromArgs;
    use fidl_fuchsia_component_decl as fcd;
    use flex_client::fidl::ServerEnd;
    use fuchsia_async as fasync;
    use futures::future::{Future, FutureExt};
    use futures::stream::StreamExt;
    #[cfg(feature = "fdomain")]
    use std::sync::Arc;

    async fn test_graph_node<F, Fut>(
        #[cfg(feature = "fdomain")] client: Arc<flex_client::Client>,
        cmd: GraphNodeCommand,
        on_driver_development_request: F,
    ) -> Result<String>
    where
        F: Fn(fdd::ManagerRequest) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<()>> + Send + Sync,
    {
        #[cfg(not(feature = "fdomain"))]
        let client = flex_client::fidl::ZirconClient;
        let (driver_development_proxy, mut driver_development_requests) =
            client.create_proxy_and_stream::<fdd::ManagerMarker>();

        let mut writer = Vec::new();
        let request_handler_task = fasync::Task::spawn(async move {
            while let Some(res) = driver_development_requests.next().await {
                let request = res.context("Failed to get next request")?;
                on_driver_development_request(request).await.context("Failed to handle request")?;
            }
            anyhow::bail!("Driver development request stream unexpectedly closed");
        });
        futures::select! {
            res = request_handler_task.fuse() => {
                res?;
                anyhow::bail!("Request handler task unexpectedly finished");
            }
            res = graph_node(cmd, &mut writer, driver_development_proxy).fuse() => {
                res.context("Graph node command failed")?;
            }
        }

        String::from_utf8(writer).context("Failed to convert graph node output to a string")
    }

    async fn run_device_info_iterator_server(
        mut device_infos: Vec<fdd::NodeInfo>,
        iterator: ServerEnd<fdd::NodeInfoIteratorMarker>,
    ) -> Result<()> {
        let mut iterator = iterator.into_stream();
        while let Some(res) = iterator.next().await {
            let request = res.context("Failed to get request")?;
            match request {
                fdd::NodeInfoIteratorRequest::GetNext { responder } => {
                    responder
                        .send(&device_infos)
                        .context("Failed to send device infos to responder")?;
                    device_infos.clear();
                }
            }
        }
        Ok(())
    }

    #[fuchsia::test]
    async fn test_graph_node_simple() {
        #[cfg(feature = "fdomain")]
        let client = fdomain_local::local_client_empty();
        let cmd = GraphNodeCommand::from_args(&["graph"], &[]).unwrap();

        let output = test_graph_node(
            #[cfg(feature = "fdomain")]
            Arc::clone(&client),
            cmd,
            |request: fdd::ManagerRequest| async move {
                match request {
                    fdd::ManagerRequest::GetNodeInfo { iterator, .. } => {
                        run_device_info_iterator_server(
                            vec![
                                fdd::NodeInfo {
                                    id: Some(0),
                                    parent_ids: Some(vec![]),
                                    child_ids: Some(vec![1]),
                                    driver_host_koid: Some(100),
                                    bound_driver_url: Some(
                                        "fuchsia-pkg://fuchsia.com/root#meta/root.cm".to_string(),
                                    ),
                                    moniker: Some("root".to_string()),
                                    ..Default::default()
                                },
                                fdd::NodeInfo {
                                    id: Some(1),
                                    parent_ids: Some(vec![0]),
                                    child_ids: Some(vec![]),
                                    driver_host_koid: Some(100),
                                    bound_driver_url: Some(
                                        "fuchsia-pkg://fuchsia.com/child-pkg#meta/child_driver.cm"
                                            .to_string(),
                                    ),
                                    moniker: Some("root.child_node".to_string()),
                                    ..Default::default()
                                },
                            ],
                            iterator,
                        )
                        .await
                        .context("Failed to run device info iterator server")?;
                    }
                    _ => {}
                }
                Ok(())
            },
        )
        .await
        .unwrap();

        assert!(output.starts_with("digraph {"));
        assert!(output.contains(r#"rankdir = "TB""#));
        assert!(output.contains(r#"subgraph "cluster_100" {"#));
        assert!(output.contains(r#"subgraph "cluster_100_root.cm" {"#));
        assert!(output.contains(r#"label = "root.cm";"#));
        assert!(output.contains(r#""0" [label="root", id = "0"]"#));
        assert!(output.contains(r#"subgraph "cluster_100_child_driver.cm" {"#));
        assert!(output.contains(r#"label = "child_driver.cm";"#));
        assert!(output.contains(r#""1" [label="child_node", id = "1"]"#));
        assert!(output.contains(r#""0" -> "1" [arrowhead = box id = "0_1"]"#));
        assert!(output.ends_with("}\n"));
    }

    #[fuchsia::test]
    async fn test_graph_node_orientation_lr() {
        #[cfg(feature = "fdomain")]
        let client = fdomain_local::local_client_empty();
        let cmd = GraphNodeCommand::from_args(&["graph"], &["--orientation", "lr"]).unwrap();

        let output = test_graph_node(
            #[cfg(feature = "fdomain")]
            Arc::clone(&client),
            cmd,
            |request: fdd::ManagerRequest| async move {
                if let fdd::ManagerRequest::GetNodeInfo { iterator, .. } = request {
                    run_device_info_iterator_server(vec![], iterator).await.unwrap();
                }
                Ok(())
            },
        )
        .await
        .unwrap();

        assert!(output.contains(r#"rankdir = "LR""#));
    }

    #[fuchsia::test]
    async fn test_graph_unbound_nodes() {
        #[cfg(feature = "fdomain")]
        let client = fdomain_local::local_client_empty();
        let cmd = GraphNodeCommand::from_args(&["graph"], &[]).unwrap();

        let output = test_graph_node(
            #[cfg(feature = "fdomain")]
            Arc::clone(&client),
            cmd,
            |request: fdd::ManagerRequest| async move {
                if let fdd::ManagerRequest::GetNodeInfo { iterator, .. } = request {
                    run_device_info_iterator_server(
                        vec![fdd::NodeInfo {
                            id: Some(5),
                            parent_ids: Some(vec![]),
                            child_ids: Some(vec![]),
                            driver_host_koid: None,
                            bound_driver_url: Some("unbound".to_string()),
                            moniker: Some("unbound_node".to_string()),
                            ..Default::default()
                        }],
                        iterator,
                    )
                    .await
                    .unwrap();
                }
                Ok(())
            },
        )
        .await
        .unwrap();

        assert!(!output.contains("subgraph"));
        assert!(output.contains(r#"    "5" [label="unbound_node", id = "5"]"#));
    }

    #[fuchsia::test]
    async fn test_injection_driver_url_subgraph_and_label() {
        #[cfg(feature = "fdomain")]
        let client = fdomain_local::local_client_empty();
        let cmd = GraphNodeCommand::from_args(&["graph"], &[]).unwrap();

        let output = test_graph_node(
            #[cfg(feature = "fdomain")]
            Arc::clone(&client),
            cmd,
            |request: fdd::ManagerRequest| async move {
                if let fdd::ManagerRequest::GetNodeInfo { iterator, .. } = request {
                    run_device_info_iterator_server(
                        vec![fdd::NodeInfo {
                            id: Some(0),
                            parent_ids: Some(vec![]),
                            child_ids: Some(vec![]),
                            driver_host_koid: Some(200),
                            bound_driver_url: Some(
                                "fuchsia-pkg://fuchsia.com/pkg#meta/evil.cm\"; evil_subgraph [label=\"hacked\"];".to_string(),
                            ),
                            moniker: Some("root".to_string()),
                            ..Default::default()
                        }],
                        iterator,
                    )
                    .await
                    .unwrap();
                }
                Ok(())
            },
        )
        .await
        .unwrap();

        // Check that quotes in driver_name are safely escaped with \"
        assert!(
            output
                .contains(r#"subgraph "cluster_200_evil.cm\"; evil_subgraph [label=\"hacked\"];""#)
        );
        assert!(output.contains(r#"label = "evil.cm\"; evil_subgraph [label=\"hacked\"];";"#));
        // Verify raw unescaped breakout does not exist
        assert!(!output.contains(r#"subgraph "cluster_200_evil.cm"; evil_subgraph"#));
    }

    #[fuchsia::test]
    async fn test_injection_node_moniker_quotes_and_backslashes() {
        #[cfg(feature = "fdomain")]
        let client = fdomain_local::local_client_empty();
        let cmd = GraphNodeCommand::from_args(&["graph"], &[]).unwrap();

        let output = test_graph_node(
            #[cfg(feature = "fdomain")]
            Arc::clone(&client),
            cmd,
            |request: fdd::ManagerRequest| async move {
                if let fdd::ManagerRequest::GetNodeInfo { iterator, .. } = request {
                    run_device_info_iterator_server(
                        vec![fdd::NodeInfo {
                            id: Some(42),
                            parent_ids: Some(vec![]),
                            child_ids: Some(vec![]),
                            driver_host_koid: None,
                            bound_driver_url: Some("unbound".to_string()),
                            moniker: Some(
                                r#"sys.sensor" [color=red, label="injected"]; "evil"#.to_string(),
                            ),
                            ..Default::default()
                        }],
                        iterator,
                    )
                    .await
                    .unwrap();
                }
                Ok(())
            },
        )
        .await
        .unwrap();

        assert!(output.contains(r#"label="sensor\" [color=red, label=\"injected\"]; \"evil""#));
        assert!(!output.contains(r#"label="sensor" [color=red"#));
    }

    #[fuchsia::test]
    async fn test_injection_node_moniker_newlines_and_control_chars() {
        #[cfg(feature = "fdomain")]
        let client = fdomain_local::local_client_empty();
        let cmd = GraphNodeCommand::from_args(&["graph"], &[]).unwrap();

        let output = test_graph_node(
            #[cfg(feature = "fdomain")]
            Arc::clone(&client),
            cmd,
            |request: fdd::ManagerRequest| async move {
                if let fdd::ManagerRequest::GetNodeInfo { iterator, .. } = request {
                    run_device_info_iterator_server(
                        vec![fdd::NodeInfo {
                            id: Some(10),
                            parent_ids: Some(vec![]),
                            child_ids: Some(vec![]),
                            driver_host_koid: None,
                            bound_driver_url: Some("unbound".to_string()),
                            moniker: Some(
                                "root.line1\nline2\r\nline3\x1b[31m\x07alert\x7fdel\0null"
                                    .to_string(),
                            ),
                            ..Default::default()
                        }],
                        iterator,
                    )
                    .await
                    .unwrap();
                }
                Ok(())
            },
        )
        .await
        .unwrap();

        let rep = char::REPLACEMENT_CHARACTER;
        let expected_label = format!("line1\\nline2\\nline3{rep}[31m{rep}alert{rep}del{rep}null");
        assert!(output.contains(&format!("label=\"{}\"", expected_label)));
    }

    #[fuchsia::test]
    async fn test_injection_service_offers_and_renamed_instances() {
        #[cfg(feature = "fdomain")]
        let client = fdomain_local::local_client_empty();
        let cmd = GraphNodeCommand::from_args(&["graph"], &["--services"]).unwrap();

        let output = test_graph_node(
            #[cfg(feature = "fdomain")]
            Arc::clone(&client),
            cmd,
            |request: fdd::ManagerRequest| async move {
                if let fdd::ManagerRequest::GetNodeInfo { iterator, .. } = request {
                    run_device_info_iterator_server(
                        vec![
                            fdd::NodeInfo {
                                id: Some(0),
                                parent_ids: Some(vec![]),
                                child_ids: Some(vec![1]),
                                driver_host_koid: Some(100),
                                bound_driver_url: Some(
                                    "fuchsia-pkg://fuchsia.com/root#meta/root.cm".to_string(),
                                ),
                                moniker: Some("root".to_string()),
                                ..Default::default()
                            },
                            fdd::NodeInfo {
                                id: Some(1),
                                parent_ids: Some(vec![0]),
                                child_ids: Some(vec![]),
                                driver_host_koid: Some(100),
                                bound_driver_url: Some(
                                    "fuchsia-pkg://fuchsia.com/child#meta/child.cm".to_string(),
                                ),
                                moniker: Some("root.child".to_string()),
                                offer_list: Some(vec![fcd::Offer::Service(fcd::OfferService {
                                    source_name: Some(
                                        "fuchsia.evil.Service\"; evil [label=\"pwned\"]; //"
                                            .to_string(),
                                    ),
                                    target_name: Some("fuchsia.evil.Service".to_string()),
                                    source: Some(fcd::Ref::Child(fcd::ChildRef {
                                        name: "root".to_string(),
                                        collection: None,
                                    })),
                                    target: None,
                                    source_instance_filter: None,
                                    renamed_instances: Some(vec![fcd::NameMapping {
                                        source_name: "src\" [style=bold]; \"".to_string(),
                                        target_name: "dst\nbreakout".to_string(),
                                    }]),
                                    availability: None,
                                    ..Default::default()
                                })]),
                                ..Default::default()
                            },
                        ],
                        iterator,
                    )
                    .await
                    .unwrap();
                }
                Ok(())
            },
        )
        .await
        .unwrap();

        // Verify the label has escaped quotes and newlines
        assert!(output.contains(r#"label="fuchsia.evil.Service\"; evil [label=\"pwned\"]; //(src\" [style=bold]; \"-->dst\nbreakout)""#));
        // Verify edge connections to and from intermediate node
        assert!(output.contains(r#"-> "intermediate_0_1_"#));
        assert!(output.contains(r#""intermediate_0_1_"#));
    }

    #[fuchsia::test]
    async fn test_stress_graphviz_dot_syntax_invariants_on_complex_hierarchies() {
        #[cfg(feature = "fdomain")]
        let client = fdomain_local::local_client_empty();
        let cmd = GraphNodeCommand::from_args(&["graph"], &["--services", "--orientation", "lr"])
            .unwrap();

        let attack_payloads = [
            r#"payload" [color=red]; evil_node [label="injected"]; "#,
            r#"subgraph" { label="hacked"; a -> b; } "#,
            "multi\nline\r\nstring\twith\x1b[31mcontrol\x07codes",
            r#"path\with\trailing\backslash\"#,
            r#"" -> "evil_target" [label="hijack"]; "node"#,
            "Unicode_🦀_日本語_Café_100%",
            "/* comment */ // line\nnode [shape=box];",
        ];

        let mut test_nodes = Vec::new();
        // Create 20 nodes with various parent-child edges and driver hosts
        for i in 0u64..20u64 {
            let host_koid = (i % 4) + 1000;
            let driver_idx = (i as usize) % attack_payloads.len();
            let moniker_idx = ((i + 1) as usize) % attack_payloads.len();

            let parent_ids: Vec<u64> = if i > 0 { vec![(i - 1) / 2] } else { vec![] };
            let child_ids: Vec<u64> = vec![];

            let offers = if i % 2 == 1 {
                Some(vec![fcd::Offer::Service(fcd::OfferService {
                    source_name: Some(format!(
                        "fuchsia.service.{}",
                        attack_payloads[(i as usize) % attack_payloads.len()]
                    )),
                    target_name: Some("fuchsia.service.target".to_string()),
                    source: Some(fcd::Ref::Child(fcd::ChildRef {
                        name: format!("node_{}", (i - 1) / 2),
                        collection: None,
                    })),
                    target: None,
                    source_instance_filter: None,
                    renamed_instances: Some(vec![fcd::NameMapping {
                        source_name: format!(
                            "src_{}",
                            attack_payloads[((i + 2) as usize) % attack_payloads.len()]
                        ),
                        target_name: format!(
                            "dst_{}",
                            attack_payloads[((i + 3) as usize) % attack_payloads.len()]
                        ),
                    }]),
                    availability: None,
                    ..Default::default()
                })])
            } else {
                None
            };

            test_nodes.push(fdd::NodeInfo {
                id: Some(i),
                parent_ids: Some(parent_ids),
                child_ids: Some(child_ids),
                driver_host_koid: Some(host_koid),
                bound_driver_url: Some(format!(
                    "fuchsia-pkg://fuchsia.com/pkg#meta/driver_{}.cm",
                    attack_payloads[driver_idx]
                )),
                moniker: Some(format!("root.node_{}.{}", i, attack_payloads[moniker_idx])),
                offer_list: offers,
                ..Default::default()
            });
        }

        // Set child_ids reciprocally
        for i in 1u64..20u64 {
            let parent_idx = ((i - 1) / 2) as usize;
            if let Some(ref mut children) = test_nodes[parent_idx].child_ids {
                children.push(i);
            }
        }

        let output = test_graph_node(
            #[cfg(feature = "fdomain")]
            Arc::clone(&client),
            cmd,
            move |request: fdd::ManagerRequest| {
                let nodes_clone = test_nodes.clone();
                async move {
                    if let fdd::ManagerRequest::GetNodeInfo { iterator, .. } = request {
                        run_device_info_iterator_server(nodes_clone, iterator).await.unwrap();
                    }
                    Ok(())
                }
            },
        )
        .await
        .unwrap();

        // Validate Graphviz AST Invariants on the entire output
        assert!(output.starts_with("digraph {"));
        assert!(output.trim_end().ends_with('}'));
        assert!(output.contains(r#"rankdir = "LR""#));

        // Invariant 1: No raw C0 or C1 control characters in output
        for c in output.chars() {
            if c != '\n' && c != '\r' && c != '\t' {
                assert!(
                    !c.is_control(),
                    "Raw control character U+{:04X} found in DOT output",
                    c as u32
                );
            }
        }

        // Invariant 2: Verify brace nesting outside string literals
        let mut brace_depth = 0;
        let mut in_string = false;
        let mut string_escaped = false;

        for c in output.chars() {
            if in_string {
                if string_escaped {
                    string_escaped = false;
                } else if c == '\\' {
                    string_escaped = true;
                } else if c == '"' {
                    in_string = false;
                }
            } else if c == '"' {
                in_string = true;
                string_escaped = false;
            } else if c == '{' {
                brace_depth += 1;
            } else if c == '}' {
                brace_depth -= 1;
                assert!(brace_depth >= 0, "Mismatched closing brace in DOT output");
            }
        }

        assert_eq!(brace_depth, 0, "Unclosed braces in DOT output");
        assert!(!in_string, "Unclosed string literal in DOT output");

        // Invariant 3: Validate all label="..." and subgraph "..." attributes are safe
        for line in output.lines() {
            let mut cursor = line;
            while let Some(start_idx) = cursor.find('"') {
                let after_start = &cursor[start_idx + 1..];
                // Find closing unescaped quote
                let mut end_idx = None;
                let mut esc = false;
                for (byte_offset, ch) in after_start.char_indices() {
                    if esc {
                        esc = false;
                    } else if ch == '\\' {
                        esc = true;
                    } else if ch == '"' {
                        end_idx = Some(byte_offset);
                        break;
                    }
                }

                let quote_end = end_idx.unwrap_or_else(|| panic!("Unclosed quote in line: {line}"));
                let inner = &after_start[..quote_end];
                // Verify inner does not have illegal escapes
                let mut check_esc = false;
                for ch in inner.chars() {
                    if check_esc {
                        assert!(
                            ch == '\\' || ch == '"' || ch == 'n',
                            "Invalid escape sequence \\{ch} in line: {line}"
                        );
                        check_esc = false;
                    } else if ch == '\\' {
                        check_esc = true;
                    }
                }
                assert!(!check_esc, "Trailing unescaped backslash in literal in line: {line}");

                cursor = &after_start[quote_end + 1..];
            }
        }
    }
}
