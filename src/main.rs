mod cli;
mod core;
mod store;
mod check;

use cli::Cli;
use color_eyre::eyre::Result;
use tracing::Level;
use tracing_subscriber::{
    filter, fmt, prelude::__tracing_subscriber_SubscriberExt, util::SubscriberInitExt, EnvFilter,
    Layer,
};

fn main() -> Result<()> {
    let app_log_layer = fmt::layer()
        .with_filter(EnvFilter::from_env("HAB_AUTO_BUILD_DEBUG"))
        .with_filter(filter::filter_fn(|metadata| {
            metadata.target() != "user-ui" && metadata.target() != "user-log"
        }));
    let user_ui_layer = fmt::layer()
        .with_target(false)
        .with_level(false)
        .without_time()
        .with_filter(filter::filter_fn(|metadata| {
            metadata.target() == "user-ui" && *metadata.level() == Level::INFO
        }));
    let user_log_layer = fmt::layer()
        .with_target(false)
        .with_level(true)
        .without_time()
        .with_filter(filter::filter_fn(|metadata| {
            metadata.target() == "user-log"
        }));

    tracing_subscriber::registry()
        .with(app_log_layer)
        .with(user_ui_layer)
        .with(user_log_layer)
        .init();

    color_eyre::install()?;

    Cli::run()
}
