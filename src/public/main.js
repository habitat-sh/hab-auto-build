const fn = (async() => {
    const data = await (async() => {
        return await d3.json("/data" + window.location.search);
    })();

    const step = 14;
    const margin = { top: 20, right: 20, bottom: 20, left: 200 };
    const height = (data.nodes.length - 1) * step + margin.top + margin.bottom;
    const arc = function arc(d) {
        const y1 = d.source.y;
        const y2 = d.target.y;
        const r = Math.abs(y2 - y1) / 2;
        return `M${margin.left},${y1}A${r},${r} 0,0,${y1 < y2 ? 1 : 0} ${margin.left},${y2}`;
    };

    const feedbackArc = function arc(d) {
        const y1 = d.source.y;
        const y2 = d.target.y;
        const r = Math.abs(y2 - y1) / 2;
        return `M${margin.left},${y1}A${r},${r} 0,0,${y1 < y2 ? 1 : 0} ${margin.left},${y2}`;
    };
    const graph = (() => {
        const nodes = data.nodes.map(({ ident }) => ({
            id: `${ident.origin}/${ident.name}/${ident.version}`,
            sourceLinks: [],
            targetLinks: [],
            feedbackSourceLinks: [],
            feedbackTargetLinks: [],
            degree: 0
        }));

        const nodeById = new Map();
        for (let index = 0; index < nodes.length; index++) {
            nodeById.set(index, nodes[index]);
        }

        const links = data.edges.map((edge) => ({
            source: nodeById.get(edge[0]),
            target: nodeById.get(edge[1]),
            dep_type: nodeById.get(edge[2])
        }));

        const feedbackLinks = data.feedback_edges.map((edge) => ({
            source: nodeById.get(edge[0]),
            target: nodeById.get(edge[1]),
            dep_type: nodeById.get(edge[2])
        }));

        for (const link of links) {
            const { source, target } = link;
            source.sourceLinks.push(link);
            target.targetLinks.push(link);
        }

        for (const feedbackLink of feedbackLinks) {
            const { source, target } = feedbackLink;
            source.feedbackSourceLinks.push(feedbackLink);
            target.feedbackTargetLinks.push(feedbackLink);
        }

        let roots = [];
        for (const index in nodes) {
            const node = nodes[index];
            if (node.sourceLinks.length == 0) {
                roots.push(node);
            }
        }
        while (roots.length > 0) {
            const currentNode = roots.shift();
            let degree = 0;
            for (const sourceLink of currentNode.sourceLinks) {
                degree = Math.max(sourceLink.target.degree + 1, degree);
            }
            currentNode.degree = degree;
            for (const targetLink of currentNode.targetLinks) {
                if (roots.indexOf(targetLink.source) < 0) {
                    roots.push(targetLink.source);
                }
            }
        }
        return { nodes, links, feedbackLinks };
    })();

    const color = d3.scaleOrdinal(graph.nodes.map(d => d.degree).sort(d3.ascending), d3.schemeCategory10);
    const y = d3.scalePoint(graph.nodes.map(d => d.id).sort(d3.ascending), [margin.top, height - margin.bottom]).domain(graph.nodes.sort((a, b) => a.degree - b.degree).map(d => d.id));
    const chart = (() => {
        const header = d3.select("#graph-header").text(`${data.nodes.length} Packages`);
        const svg = d3.select("#graph").attr("width", "100%").attr("height", height);

        svg.append("style").text(`
      
      .hover path {
        stroke: #ccc;
      }
      
      .hover text {
        fill: #ccc;
      }
      
      .hover g.primary text {
        fill: black;
        font-weight: bold;
      }
      
      .hover g.secondary text {
        fill: #333;
      }
      .hover path.primary-feedback-source path.primary-source {
        stroke: #933;
        stroke-opacity: 1;
      }
      .hover path.primary-feedback-target path.primary-target {
        stroke: #933;
        stroke-opacity: 1;
      }
      .hover path.primary-source {
        stroke: #393;
        stroke-opacity: 1;
      }
      .hover path.primary-target {
        stroke: #339;
        stroke-opacity: 1;
      }
      
      `);

        const label = svg.append("g")
            .attr("font-family", "sans-serif")
            .attr("font-size", 10)
            .attr("text-anchor", "end")
            .selectAll("g")
            .data(graph.nodes)
            .join("g")
            .attr("transform", d => `translate(${margin.left},${d.y = y(d.id)})`)
            .call(g => g.append("text")
                .attr("x", -6)
                .attr("dy", "0.35em")
                .attr("fill", d => d3.lab(color(d.degree)).darker(2))
                .text(d => `${d.id}-${d.degree}`))
            .call(g => g.append("circle")
                .attr("r", 3)
                .attr("fill", d => color(d.degree)));

        const path = svg.insert("g", "*")
            .attr("fill", "none")
            .attr("stroke-opacity", 0.6)
            .attr("stroke-width", 1.5)
            .selectAll("path")
            .data(graph.links)
            .join("path")
            .attr("stroke", d => color(d.target.degree))
            .attr("d", arc);

        const feedbackPath = svg.insert("g", "*")
            .attr("fill", "none")
            .attr("stroke-opacity", 0.6)
            .attr("stroke-width", 1.5)
            .selectAll("path")
            .data(graph.feedbackLinks)
            .join("path")
            .attr("stroke", "#933")
            .attr("d", feedbackArc);

        const overlay = svg.append("g")
            .attr("fill", "none")
            .attr("pointer-events", "all")
            .selectAll("rect")
            .data(graph.nodes)
            .join("rect")
            .attr("width", margin.left + 40)
            .attr("height", step)
            .attr("y", d => y(d.id) - step / 2)
            .on("mouseover", d => {
                svg.classed("hover", true);
                label.classed("primary", n => n === d);
                label.classed("secondary", n => n.sourceLinks.some(l => l.target === d) || n.targetLinks.some(l => l.source === d));
                path.classed("primary-source", l => l.source === d).filter(".primary-source").raise();
                path.classed("primary-target", l => l.target === d).filter(".primary-target").raise();
                feedbackPath.classed("primary-feedback-source", l => l.source === d).filter(".primary-feedback-source").raise();
                feedbackPath.classed("primary-feedback-target", l => l.target === d).filter(".primary-feedback-target").raise();
            })
            .on("mouseout", d => {
                svg.classed("hover", false);
                label.classed("primary", false);
                label.classed("secondary", false);
                path.classed("primary-source", false).order();
                path.classed("primary-target", false).order();
                feedbackPath.classed("primary-feedback-source", false).order();
                feedbackPath.classed("primary-feedback-target", false).order();
            });
        return svg.node();
    })();
})
fn();