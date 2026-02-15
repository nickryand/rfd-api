// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

mod kube;
mod meilisearch;

use anyhow::Result;
use clap::{Parser, Subcommand};

use crate::meilisearch::MeilisearchArgs;

#[derive(Parser)]
#[command(name = "rfd-kube-init")]
#[command(about = "Kubernetes initialization tool for RFD services")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Initialize Meilisearch secrets across target namespaces
    Meilisearch(MeilisearchArgs),
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Meilisearch(args) => {
            let kube_client = ::kube::Client::try_default().await?;
            meilisearch::init(&kube_client, &args).await
        }
    }
}
