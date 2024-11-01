use anyhow::Result;
use clap::Parser;
use gavin::cli::Cli;
use gavin::{handle_cli_args, Database};

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let db = Database::new()?;
    handle_cli_args(&cli, &db).await
}
