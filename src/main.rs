use anyhow::Result;
use clap::Parser;

mod cli;
mod commands;
mod config;
mod container;
mod docker;
mod error;
mod init;
mod state;
mod worktree;

use cli::Commands;

fn main() -> Result<()> {
    let cli = cli::Cli::parse();

    match cli.command {
        Commands::Run(args) => commands::execute_run(args),
        Commands::List(args) => commands::execute_list(args),
        Commands::Delete(args) => commands::execute_delete(args),
        Commands::Status(args) => commands::execute_status(args),
        Commands::Worktree(args) => match args {
            cli::WorktreeArgs::Init(_) => worktree::init(),
            cli::WorktreeArgs::Add(args) => worktree::add(&args),
            cli::WorktreeArgs::List(args) => worktree::list(&args),
            cli::WorktreeArgs::Rm(args) => worktree::rm(&args),
        },
    }
}
