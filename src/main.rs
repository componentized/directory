//! The componentized directory: indexes the components and WIT packages published under
//! ghcr.io/componentized and renders them as a static site.

mod config;
mod data;
mod decode;
mod http;
mod resolve;
mod scan;
mod site;
mod version;
mod wit_html;

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(version, about)]
struct Cli {
    /// The root of the index, `{namespace}/{name}/{version}/`
    #[arg(long, global = true, default_value = "index")]
    data: PathBuf,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Index the new releases of the org's packages, and the versions they depend on
    Scan {
        /// The GitHub org whose ghcr.io packages are scanned, also their WIT namespace
        #[arg(long, default_value = "componentized")]
        org: String,
        /// A GitHub token, `packages: read` lets discovery use the GitHub Packages API
        #[arg(long, env = "GITHUB_TOKEN", hide_env_values = true)]
        github_token: Option<String>,
        /// A package to scan in addition to the discovered ones, e.g. `logging/levels`
        #[arg(long = "package")]
        packages: Vec<String>,
        /// Only scan the packages of these repositories, e.g. `logging`
        #[arg(long)]
        only: Vec<String>,
        /// The wkg config that maps WIT namespaces to registries
        #[arg(long, default_value = ".config/wasm-pkg/config.toml")]
        config: PathBuf,
    },
    /// Render the index as a static site
    Build {
        /// The output directory
        #[arg(long, default_value = "_site")]
        out: PathBuf,
        /// The URL the site is served from, for canonical links
        #[arg(long, default_value = "https://directory.componentized.dev")]
        base_url: String,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Scan {
            org,
            github_token,
            packages,
            only,
            config,
        } => scan::run(&scan::Options {
            root: cli.data,
            org,
            github_token: github_token.filter(|token| !token.is_empty()),
            packages,
            only,
            registries: config::Config::load(&config)?,
        }),
        Command::Build { out, base_url } => site::build(&cli.data, &out, &base_url),
    }
}
