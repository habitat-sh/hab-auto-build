use crate::core::{AutoBuildConfig, AutoBuildContext, ChangeDetectionMode, DepGraphData};

use axum::{
    body::{boxed, Full},
    extract::State,
    handler::HandlerWithoutStateExt,
    http::{header, StatusCode, Uri},
    response::{Html, IntoResponse, Response},
    routing::get,
    Json, Router,
};
use clap::Args;
use color_eyre::eyre::{eyre, Context, Result};
use petgraph::visit::EdgeRef;
use rust_embed::RustEmbed;
use serde_json::Value;
use std::{env, net::SocketAddr, path::PathBuf, sync::Arc};

#[derive(Debug, Args)]
pub(crate) struct Params {
    /// Path to hab auto build configuration
    #[arg(short, long)]
    config_path: Option<PathBuf>,
    /// Port to listen for HTTP requests
    #[arg(short, long)]
    port: u16,
}

pub(crate) fn execute(args: Params) -> Result<()> {
    let config_path = args.config_path.unwrap_or(
        env::current_dir()
            .context("Failed to determine current working directory")?
            .join("hab-auto-build.json"),
    );
    let config = AutoBuildConfig::new(&config_path)?;

    let run_context = AutoBuildContext::new(&config, &config_path, ChangeDetectionMode::Disk)
        .with_context(|| eyre!("Failed to initialize run"))?;

    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(start(run_context.dep_graph_data(), args.port));
    Ok(())
}

async fn start(graph: DepGraphData, port: u16) {
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

// basic handler that responds with a static string
async fn data(State(graph): State<Arc<DepGraphData>>) -> Json<Value> {
    Json(serde_json::to_value(&*graph).unwrap())
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
