use anyhow::Result;
use std::sync::Arc;
use tokio::sync::Semaphore;
use tokio::fs;
use clap::CommandFactory;
use crate::{
    Config,
    Database, SupportedTask, TaskValidState, GitVersionState,
    find_pipeline_files, search_in_pipelines_concurrent,
    search_gitversion_tasks, check_all_task_implementations,
    collect_task_usage, ensure_all_repos_exist, search_default_task,
    git_manager::GitManager,
    cli::Cli,
    report::generate_markdown_report,
    utils::sanitize_file_path,
};

pub async fn handle_cli_args(cli: &Cli, db: &Database) -> Result<()> {
    // Load config if needed
    if let Some(config) = load_config_if_needed(cli)? {
        // Merge config states into database
        db.merge_config_states(&config)?;
    }

    // Check if any meaningful argument is provided
    let has_args = cli.search_string.is_some() 
        || cli.search_task.is_some()
        || cli.list_repos
        || cli.list_pipelines
        || cli.add_repo.is_some()
        || cli.add_multiple_repos.is_some()
        || cli.delete_repo.is_some()
        || cli.add_task_state.is_some()
        || cli.delete_task_state.is_some()
        || cli.list_task_states.is_some()
        || cli.list_all_task_states
        || cli.analyze_tasks
        || cli.check_tasks
        || cli.set_git_credentials.is_some();

    if !has_args {
        Cli::command().print_help()?;
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
            match task.to_string().as_str() {
                "gitversion" => search_gitversion_tasks(&repos, cli.verbose).await?,
                task_name => search_default_task(&repos, task_name, cli.verbose).await?,
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

async fn handle_other_cli_args(cli: &Cli, db: &Database) -> Result<()> {
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
        ensure_all_repos_exist(db, cli.no_update).await?;
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
        db.add_repository(repo_url, cli.new).await?;
        println!("Added repository: {}", repo_url);
    } else if let Some(repos) = &cli.add_multiple_repos {
        let repo_urls: Vec<&str> = repos.split(',').map(str::trim).collect();
        let credentials = db.get_git_credentials()?
            .ok_or_else(|| anyhow::anyhow!("Git credentials not found. Please set them first with --set-git-credentials"))?;
        
        // First, test all connections sequentially
        println!("Testing connections to all repositories...");
        let mut valid_repos = Vec::new();
        let mut failed_repos = Vec::new();
        
        for repo_url in repo_urls {
            let git_manager = GitManager::new(credentials.0.clone(), credentials.1.clone(), repo_url);
            match git_manager.test_connection().await {
                Ok(_) => {
                    valid_repos.push(repo_url.to_string());
                }
                Err(e) => {
                    println!("✗ Failed to connect to repository {}: {}", repo_url, e);
                    failed_repos.push((repo_url.to_string(), e));
                }
            }
        }

        if valid_repos.is_empty() {
            println!("No valid repositories to process.");
            return Ok(());
        }

        // Then process valid repos in parallel
        println!("\nProcessing {} valid repositories...", valid_repos.len());
        let max_concurrent = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);
        let semaphore = Arc::new(Semaphore::new(max_concurrent));
        let is_new = cli.new; // Clone the flag value before moving into async blocks
        
        let mut handles = Vec::new();
        
        for repo_url in valid_repos {
            let permit = semaphore.clone().acquire_owned().await?;
            let creds = (credentials.0.clone(), credentials.1.clone());
            
            let handle = tokio::spawn(async move {
                let _permit = permit;
                let git_manager = GitManager::new(creds.0, creds.1, &repo_url);
                
                match if is_new {
                    git_manager.ensure_repo_exists_new().await
                } else {
                    git_manager.ensure_repo_exists().await
                } {
                    Ok(()) => Ok(repo_url),
                    Err(e) => Err((repo_url.clone(), e)),
                }
            });
            
            handles.push(handle);
        }

        let mut success_urls = Vec::new();
        
        for handle in handles {
            match handle.await? {
                Ok(url) => {
                    println!("✓ Successfully cloned repository: {}", url);
                    success_urls.push(url);
                }
                Err((url, error)) => {
                    println!("✗ Failed to clone repository {}: {}", url, error);
                    failed_repos.push((url, error));
                }
            }
        }

        // Add successful repos to database synchronously
        println!("\nAdding repositories to database...");
        for url in &success_urls {
            if let Err(e) = db.add_repository_sync(url) {
                let error_msg = e.to_string();
                println!("✗ Failed to add {} to database: {}", url, error_msg);
                failed_repos.push((url.clone(), anyhow::anyhow!(error_msg)));
            } else {
                println!("✓ Added to database: {}", url);
            }
        }

        println!("\nSummary:");
        println!("Successfully added {} repositories", success_urls.len());
        if !failed_repos.is_empty() {
            println!("Failed to process {} repositories:", failed_repos.len());
            for (url, error) in failed_repos {
                println!("✗ {}: {}", url, error);
            }
        }
    } else if let Some(path) = &cli.delete_repo {
        db.delete_repository(path)?;
        println!("Deleted repository: {}", path);
    } else if let (Some(task_name), Some(state_str)) = (cli.add_task_state.clone(), &cli.state_value) {
        match &task_name {
            SupportedTask::Gitversion => {
                let state = GitVersionState::from_string(state_str)
                    .map_err(|e| anyhow::anyhow!("Invalid state format: {}", e))?;
                db.add_valid_state(&task_name, &TaskValidState::Gitversion(state))?;
                println!("Added valid state for GitVersion");
            },
            SupportedTask::Default(name) => {
                db.add_valid_state(&task_name, &TaskValidState::Default(state_str.to_string()))?;
                println!("Added valid state for {}", name);
            }
        }
    } else if let Some(task) = &cli.list_task_states {
        list_task_states(db, task)?;
    } else if cli.list_all_task_states {
        handle_list_all_task_states(db).await?;
    } else if cli.analyze_tasks {
        let repos = db.list_repositories()?;
        // Ensure repos exist before analyzing
        ensure_all_repos_exist(db, cli.no_update).await?;
        collect_task_usage(&repos).await?;
    } else if cli.check_tasks {
        let repos = db.list_repositories()?;
        // Ensure repos exist before checking tasks
        ensure_all_repos_exist(db, cli.no_update).await?;
        
        if cli.output_markdown {
            let issues = check_all_task_implementations(&repos, None, cli.no_update).await?;
            let report = generate_markdown_report(&repos, &db, &issues).await?;
            let report_path = cli.report_path.as_deref().unwrap_or("report.md");
            
            // Sanitize the output path
            let safe_path = sanitize_file_path(report_path);
            fs::write(&safe_path, report).await?;
            println!("Generated markdown report: {}", safe_path.display());
        } else {
            let _issues = check_all_task_implementations(&repos, None, cli.no_update).await?;
        }
    } else if let Some(task) = &cli.delete_task_state {
        if let Some(state_value) = &cli.state_value {
            let state = match task {
                SupportedTask::Gitversion => {
                    TaskValidState::Gitversion(
                        GitVersionState::from_string(state_value)
                            .map_err(|e| anyhow::anyhow!("Invalid GitVersion state format: {}", e))?
                    )
                }
                SupportedTask::Default(_) => {
                    TaskValidState::Default(state_value.to_string())
                }
            };
            db.delete_valid_state(task, &state)?;
            println!("Deleted task state for {}: {}", task, state_value);
        } else {
            println!("Error: --state-value is required when using --delete-task-state");
            return Ok(());
        }
    } else {
        println!("Invalid combination of arguments. Use --help for usage information.");
        std::process::exit(1);
    }

    Ok(())
}

// async fn handle_list_task_states(task: SupportedTask, db: &Database) -> Result<()> {
//     let states = db.list_valid_states(&task)?;
//     println!("\nValid states for {}:", task);
//     println!("{}", "-".repeat(60));
//     if states.is_empty() {
//         println!("  No valid states defined yet.");
//         println!("  To add a state, use:");
//         println!("    --add-task-state {} --state-value \"setup:VERSION,execute:VERSION,spec:VERSION\"", task);
//         println!("  Example:");
//         println!("    --add-task-state {} --state-value \"setup:3,execute:3,spec:6.0.3\"", task);
//     } else {
//         for state in states {
//             match state {
//                 TaskValidState::Gitversion(gv) => {
//                     println!("  setup: v{}", gv.setup_version);
//                     println!("  execute: v{}", gv.execute_version);
//                     println!("  spec: v{}", gv.spec_version);
//                     println!();
//                 },
//                 TaskValidState::Default(version) => {
//                     println!("  version: v{}", version);
//                     println!();
//                 }
//             }
//         }
//     }
//     Ok(())
// }

async fn handle_list_all_task_states(db: &Database) -> Result<()> {
    let tasks = db.get_all_tasks()?;
    
    for task in tasks {
        let states = db.list_valid_states(&task)?;
        println!("\nValid states for {}:", task);
        println!("{}", crate::format_task_states(&task, states));
    }
    Ok(())
}

fn load_config_if_needed(cli: &Cli) -> Result<Option<Config>> {
    // Check if the command needs state configuration
    let needs_config = cli.search_task.is_some()
        || cli.delete_task_state.is_some()
        || cli.add_task_state.is_some()
        || cli.state_value.is_some()
        || cli.list_task_states.is_some()
        || cli.list_all_task_states
        || cli.analyze_tasks
        || cli.check_tasks;

    if needs_config {
        let config = Config::load(cli.config_path.as_deref())?;
        Ok(Some(config))
    } else {
        Ok(None)
    }
}

// fn list_supported_tasks() {
//     for task in SupportedTask::get_all_variants() {
//         println!("  {}", task);
//     }
// }

fn list_task_states(db: &Database, task: &SupportedTask) -> Result<()> {
    let states = db.list_valid_states(task)?;
    println!("Valid states for {}:", task);
    println!("{}", crate::format_task_states(task, states));
    Ok(())
}