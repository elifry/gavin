use anyhow::Result;
use gavin::{Database, handle_cli_args};
use gavin::cli::Cli;
use clap::Parser;

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let db = Database::new()?;
    handle_cli_args(&cli, &db).await
}