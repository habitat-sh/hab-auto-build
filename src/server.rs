use crate::{DependencyType, PackageDependencyGraph, PlanMetadata};

use axum::{
    body::{boxed, Full},
    extract::{Query, State},
    handler::HandlerWithoutStateExt,
    http::{header, StatusCode, Uri},
    response::{Html, IntoResponse, Response},
    routing::get,
    Json, Router,
};
use petgraph::{
    algo::{self, greedy_feedback_arc_set},
    stable_graph::EdgeIndex,
    visit::EdgeRef,
};
use rust_embed::RustEmbed;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{net::SocketAddr, sync::Arc};

pub async fn start(graph: PackageDependencyGraph, port: u16) {
    let graph = Arc::new(graph);
    // build our application with a route
    let app = Router::new()
        .route("/", get(index_handler))
        .route("/index.html", get(index_handler))
        .route_service("/static/*file", static_handler.into_service())
        .route("/data", get(data))
        .with_state(graph);

    // run our app with hyper
    // `axum::Server` is a re-export of `hyper::Server`
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    tracing::info!("Server started on {}", addr);
    axum::Server::bind(&addr)
        .serve(app.into_make_service())
        .await
        .unwrap();
}

#[derive(Debug, Serialize)]
struct GraphData {
    nodes: Vec<PlanMetadata>,
    edges: Vec<(usize, usize, DependencyType)>,
    feedback_edges: Vec<(usize, usize)>,
}

#[derive(Deserialize, Debug)]
struct Package {
    origin: Option<String>,
    name: Option<String>,
    include_studios: Option<bool>,
}
// basic handler that responds with a static string
async fn data(
    State(graph): State<Arc<PackageDependencyGraph>>,
    package: Query<Package>,
) -> Json<Value> {
    let graph = (**graph).clone();

    let selected_package_nodes = if let (Some(origin), Some(name)) =
        (&package.origin, &package.name)
    {
        graph
            .node_indices()
            .filter(|index| {
                graph[*index].plan.ident.origin == *origin && graph[*index].plan.ident.name == *name
            })
            .collect::<Vec<_>>()
    } else {
        vec![]
    };
    let graph = graph.filter_map(
        |_, node| Some(node),
        |_, edge| match edge {
            DependencyType::Runtime | DependencyType::Build => Some(*edge),
            DependencyType::Studio => {
                if package.include_studios.unwrap_or_default() {
                    Some(*edge)
                } else {
                    None
                }
            }
        },
    );
    let mut graph = graph.filter_map(
        |node_index, node| {
            if selected_package_nodes.is_empty() {
                Some((*node).clone())
            } else {
                for package_node in selected_package_nodes.iter() {
                    if algo::has_path_connecting(&graph, *package_node, node_index, None) {
                        return Some((*node).clone());
                    }
                }
                None
            }
        },
        |_, edge| Some(*edge),
    );
    let mut feedback_edges = vec![];
    let edges_to_remove: Vec<EdgeIndex> = greedy_feedback_arc_set(&graph).map(|e| e.id()).collect();
    for edge_index in edges_to_remove.iter() {
        if let Some((source, target)) = graph.edge_endpoints(*edge_index) {
            feedback_edges.push((source.index(), target.index()));
        }
    }

    for edge_index in edges_to_remove.iter() {
        graph.remove_edge(*edge_index);
    }

    let (nodes, edges) = graph.into_nodes_edges();
    let nodes = nodes
        .iter()
        .map(|node| node.weight.plan.clone())
        .collect::<Vec<_>>();
    let edges = edges
        .iter()
        .map(|edge| (edge.source().index(), edge.target().index(), edge.weight))
        .collect::<Vec<_>>();
    Json(
        serde_json::to_value(&GraphData {
            nodes,
            edges,
            feedback_edges,
        })
        .unwrap(),
    )
}

// We use static route matchers ("/" and "/index.html") to serve our home
// page.
async fn index_handler() -> impl IntoResponse {
    static_handler("/index.html".parse::<Uri>().unwrap()).await
}

// We use a wildcard matcher ("/static/*file") to match against everything
// within our defined assets directory. This is the directory on our Asset
// struct below, where folder = "examples/public/".
async fn static_handler(uri: Uri) -> impl IntoResponse {
    let mut path = uri.path().trim_start_matches('/').to_string();

    if path.starts_with("static/") {
        path = path.replace("static/", "");
    }
    StaticFile(path)
}

// Finally, we use a fallback route for anything that didn't match.
async fn not_found() -> Html<&'static str> {
    Html("<h1>404</h1><p>Not Found</p>")
}

#[derive(RustEmbed)]
#[folder = "src/public"]
struct Asset;

pub struct StaticFile<T>(pub T);

impl<T> IntoResponse for StaticFile<T>
where
    T: Into<String>,
{
    fn into_response(self) -> Response {
        let path = self.0.into();

        match Asset::get(path.as_str()) {
            Some(content) => {
                let body = boxed(Full::from(content.data));
                let mime = mime_guess::from_path(path).first_or_octet_stream();
                Response::builder()
                    .header(header::CONTENT_TYPE, mime.as_ref())
                    .body(body)
                    .unwrap()
            }
            None => Response::builder()
                .status(StatusCode::NOT_FOUND)
                .body(boxed(Full::from("404")))
                .unwrap(),
        }
    }
}
