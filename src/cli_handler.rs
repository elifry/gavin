use anyhow::Result;
use tokio::fs;
use crate::database::Database;
use crate::gitversion::GitVersionState;
use clap::{ValueEnum, CommandFactory};
use crate::report::generate_markdown_report;
use crate::{
    SupportedTask, TaskValidState, find_pipeline_files, 
    search_in_pipelines_concurrent, search_gitversion_tasks,
    check_all_task_implementations, collect_task_usage,
    ensure_all_repos_exist,
};

pub async fn handle_cli_args(cli: &crate::Cli, db: &Database) -> Result<()> {
    // Check if any meaningful argument is provided
    let has_args = cli.search_string.is_some() 
        || cli.search_task.is_some()
        || cli.list_repos
        || cli.list_pipelines
        || cli.add_repo.is_some()
        || cli.delete_repo.is_some()
        || cli.add_task_state.is_some()
        || cli.delete_task_state.is_some()
        || cli.list_task_states.is_some()
        || cli.list_all_task_states
        || cli.analyze_tasks
        || cli.check_tasks
        || cli.set_git_credentials.is_some();

    if !has_args {
        crate::Cli::command().print_help()?;
        std::process::exit(0);
    }

    // Validate state_value is only used with appropriate commands
    if cli.state_value.is_some() && cli.add_task_state.is_none() && cli.delete_task_state.is_none() {
        println!("--state-value can only be used with --add-task-state or --delete-task-state");
        std::process::exit(1);
    }

    match (&cli.search_string, &cli.search_task, cli.list_repos, cli.list_pipelines) {
        (Some(query), _, _, _) => {
            let repos = db.list_repositories()?;
            search_in_pipelines_concurrent(&repos, query).await?;
        },
        (_, Some(task), _, _) => {
            let repos = db.list_repositories()?;
            match task {
                SupportedTask::Gitversion => search_gitversion_tasks(&repos, cli.verbose).await?,
            }
        },
        (_, _, true, _) => {
            for repo in db.list_repositories()? {
                println!("{}", repo);
            }
        },
        (_, _, _, true) => {
            for repo in db.list_repositories()? {
                println!("{}", repo);
                let repo_path = db.get_local_path(&repo);
                let pipeline_files = find_pipeline_files(&repo_path).await?;
                for file in pipeline_files {
                    if let Ok(rel_path) = file.strip_prefix(&repo_path) {
                        println!("    - {}", rel_path.display());
                    }
                }
            }
        },
        _ => handle_other_cli_args(cli, &db).await?,
    }

    Ok(())
}

async fn handle_other_cli_args(cli: &crate::Cli, db: &Database) -> Result<()> {
    if cli.list_repos {
        let repos = db.list_repositories()?;
        if repos.is_empty() {
            println!("No repositories found.");
        } else {
            for url in repos {
                println!("{}", url);
            }
        }
    } else if cli.list_pipelines {
        ensure_all_repos_exist(db).await?;
        for repo_url in db.list_repositories()? {
            println!("\n{}", repo_url);
            let repo_path = db.get_local_path(&repo_url);
            let pipeline_files = find_pipeline_files(&repo_path).await?;
            for file in pipeline_files {
                if let Ok(rel_path) = file.strip_prefix(&repo_path) {
                    println!("  {}", rel_path.display());
                }
            }
        }
    } else if let Some(credentials) = &cli.set_git_credentials {
        db.set_git_credentials(credentials)?;
        println!("Git credentials updated successfully");
    } else if let Some(repo_url) = &cli.add_repo {
        db.add_repository(repo_url).await?;
        println!("Added repository: {}", repo_url);
    } else if let Some(path) = &cli.delete_repo {
        db.delete_repository(path)?;
        println!("Deleted repository: {}", path);
    } else if let (Some(task), Some(state_str)) = (cli.add_task_state, &cli.state_value) {
        match task {
            SupportedTask::Gitversion => {
                let state = GitVersionState::from_string(state_str)
                    .map_err(|e| anyhow::anyhow!("Invalid state format: {}", e))?;
                db.add_valid_state(&task, &TaskValidState::Gitversion(state))?;
                println!("Added valid state for GitVersion");
            }
        }
    } else if let Some(task) = cli.list_task_states {
        handle_list_task_states(task, db).await?;
    } else if cli.list_all_task_states {
        handle_list_all_task_states(db).await?;
    } else if cli.analyze_tasks {
        let repos = db.list_repositories()?;
        // Ensure repos exist before analyzing
        ensure_all_repos_exist(db).await?;
        collect_task_usage(&repos).await?;
    } else if cli.check_tasks {
        let repos = db.list_repositories()?;
        if cli.output_markdown {
            let report = generate_markdown_report(&repos, db).await?;
            fs::write("report.md", report).await?;
            println!("Generated markdown report: report.md");
        } else {
            check_all_task_implementations(&repos, None).await?;
        }
    } else if let (Some(task), Some(state_str)) = (cli.delete_task_state, &cli.state_value) {
        match task {
            SupportedTask::Gitversion => {
                let state = GitVersionState::from_string(state_str)
                    .map_err(|e| anyhow::anyhow!("Invalid state format: {}", e))?;
                db.delete_valid_state(&task, &TaskValidState::Gitversion(state))?;
                println!("Deleted valid state for GitVersion");
            }
        }
    } else {
        println!("Invalid combination of arguments. Use --help for usage information.");
        std::process::exit(1);
    }

    Ok(())
}

async fn handle_list_task_states(task: SupportedTask, db: &Database) -> Result<()> {
    let states = db.list_valid_states(&task)?;
    println!("\nValid states for {}:", task);
    println!("{}", "-".repeat(60));
    if states.is_empty() {
        println!("  No valid states defined yet.");
        println!("  To add a state, use:");
        println!("    --add-task-state {} --state-value \"setup:VERSION,execute:VERSION,spec:VERSION\"", task);
        println!("  Example:");
        println!("    --add-task-state {} --state-value \"setup:3,execute:3,spec:6.0.3\"", task);
    } else {
        for state in states {
            match state {
                TaskValidState::Gitversion(gv) => {
                    println!("  setup: v{}", gv.setup_version);
                    println!("  execute: v{}", gv.execute_version);
                    println!("  spec: v{}", gv.spec_version);
                    println!();
                }
            }
        }
    }
    Ok(())
}

async fn handle_list_all_task_states(db: &Database) -> Result<()> {
    println!("Valid states for all supported tasks:");
    println!("------------------------------------------------------------");

    for task in SupportedTask::value_variants() {
        let states = db.list_valid_states(task)?;
        println!("{}:", task);
        println!("{}", crate::format_task_states(task, states));
    }
    Ok(())
}