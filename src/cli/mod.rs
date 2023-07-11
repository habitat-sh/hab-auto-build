mod add;
mod analyze;
mod build;
mod check;
mod changes;
mod compare;
mod download;
mod git_sync;
mod output;
mod remove;
mod server;

use clap::{command, Parser, Subcommand};
use color_eyre::eyre::Result;

// Habitat Auto Build allows you to automatically build multiple packages
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Analyze various types of dependencies of a set of packages
    Analyze(analyze::Params),
    /// Build a set of packages
    Build(build::Params),
    /// Check a set of packages
    Check(check::Params),
    /// Check the current list of changes across all repos
    Changes(changes::Params),
    /// Compare plans across two sets of repos
    Compare(compare::Params),
    /// Download source archives for specified plans
    Download(download::Params),
    /// Add a plan from the list of changed plans
    Add(add::Params),
    /// Remove a plan from the list of changed plans
    Remove(remove::Params),
    /// Sync plan file timestamps with git commit timestamps
    GitSync(git_sync::Params),
    /// Start a server to visualize the package build graph
    Server(server::Params)
}

impl Cli {
    pub fn run() -> Result<()> {
        let cli = Cli::parse();
        match cli.command {
            Commands::Add(args) => add::execute(args),
            Commands::Changes(args) => changes::execute(args),
            Commands::Check(args) => check::execute(args),
            Commands::Compare(args) => compare::execute(args),
            Commands::Download(args) => download::execute(args),
            Commands::GitSync(args) => git_sync::execute(args),
            Commands::Remove(args) => remove::execute(args),
            Commands::Build(args) => build::execute(args),
            Commands::Analyze(args) => analyze::execute(args),
            Commands::Server(args) => server::execute(args),
        }
    }
}
