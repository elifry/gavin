use anyhow::{Context, Result};
use clap::Parser;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;
use serde::{Serialize, Deserialize};
use clap::ValueEnum;
use std::collections::HashMap;
use regex::Regex;
use std::collections::HashSet;
use itertools::Itertools;
use tokio::sync::Semaphore;
use tokio::fs;
use std::sync::Arc;

mod gitversion;
use crate::gitversion::{GitVersionState, GitVersionImplementation};

mod database;
use crate::database::Database;

mod cli_handler;
use crate::cli_handler::handle_cli_args;

mod report;

mod git_manager;
use crate::git_manager::GitManager;

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Cli {
    /// Search for a string in pipeline files
    #[arg(short = 's', long = "search")]
    search_string: Option<String>,

    /// List all repositories
    #[arg(long = "list-repos")]
    list_repos: bool,

    /// List all repositories with their pipeline files
    #[arg(long = "list-pipelines")]
    list_pipelines: bool,

    /// Add a repository to the database
    #[arg(long = "add-repo")]
    add_repo: Option<String>,

    #[arg(long = "add-multiple-repos")]
    add_multiple_repos: Option<String>,

    #[arg(long = "new")]
    new: bool,

    /// Delete a repository from the database
    #[arg(long = "delete-repo")]
    delete_repo: Option<String>,

    /// Search for a specific task in pipelines (e.g., gitversion)
    #[arg(long = "search-task", value_enum)]
    search_task: Option<SupportedTask>,

    /// Add a valid state for a task
    #[arg(long = "add-task-state")]
    add_task_state: Option<SupportedTask>,

    /// Delete a valid state for a task
    #[arg(long = "delete-task-state")]
    delete_task_state: Option<SupportedTask>,

    /// State value for --add-task-state or --delete-task-state (e.g., "setup:3,execute:3,spec:6.0.3")
    #[arg(long = "state-value")]
    state_value: Option<String>,

    /// List valid states for a specific task
    #[arg(long = "list-task-states")]
    list_task_states: Option<SupportedTask>,

    /// List valid states for all supported tasks
    #[arg(long = "list-all-task-states")]
    list_all_task_states: bool,

    /// Analyze task usage across all repositories
    #[arg(long = "analyze-tasks")]
    analyze_tasks: bool,

    /// Check all task implementations across repositories
    #[arg(long = "check-tasks")]
    check_tasks: bool,

    /// Output results as a markdown file (requires --check-tasks)
    #[arg(long = "output-markdown", requires = "check_tasks")]
    output_markdown: bool,

    /// Specify the output file path for markdown report (requires --output-markdown)
    #[arg(long = "report-path", requires = "output_markdown")]
    report_path: Option<String>,

    /// Show detailed output
    #[arg(short, long)]
    verbose: bool,

    /// Set git credentials (username:token format)
    #[arg(long = "set-git-credentials")]
    set_git_credentials: Option<String>,
}

async fn find_pipeline_files(repo_path: &Path) -> Result<Vec<PathBuf>> {
    // Create a bounded channel to prevent memory issues with large directories
    let (tx, mut rx) = tokio::sync::mpsc::channel(1000);
    
    // Spawn the blocking directory walk in a separate thread pool
    tokio::task::spawn_blocking({
        let tx = tx.clone();
        let repo_path = repo_path.to_path_buf();
        move || {
            for entry in WalkDir::new(&repo_path)
                .follow_links(true)
                .into_iter()
                .filter_map(Result::ok)
            {
                let path = entry.path().to_path_buf();
                if path.extension().map_or(false, |ext| ext == "yml" || ext == "yaml") {
                    if path.to_string_lossy().contains("pipeline") {
                        // Use try_send to avoid blocking if channel is full
                        if tx.blocking_send(path).is_err() {
                            break; // Channel closed, receiver dropped
                        }
                    }
                }
            }
        }
    });

    // Drop the sender to ensure the channel closes properly
    drop(tx);

    // Collect all paths
    let mut pipeline_files = Vec::new();
    while let Some(path) = rx.recv().await {
        pipeline_files.push(path);
    }
    
    Ok(pipeline_files)
}

struct SearchResult {
    repo: String,
    findings: Vec<Finding>,
}

struct Finding {
    file: PathBuf,
    matches: Vec<(usize, String)>, // (line number, line content)
}

async fn search_in_pipelines_concurrent(repos: &[String], query: &str) -> Result<()> {
    let max_concurrent = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    let semaphore = Arc::new(Semaphore::new(max_concurrent));
    let db = Database::new()?;
    ensure_all_repos_exist(&db).await?;
    
    // Create a channel for results
    let (tx, mut rx) = tokio::sync::mpsc::channel(repos.len());
    
    // Spawn tasks for each repo
    for repo in repos {
        let repo = repo.clone();
        let query = query.to_string();
        let permit = semaphore.clone().acquire_owned().await?;
        let tx = tx.clone();
        let db = Database::new()?;
        
        tokio::spawn(async move {
            let _permit = permit; // Hold the permit for the duration of this task
            
            // Only change: Get local path from URL
            let repo_path = db.get_local_path(&repo);
            let repo_name = repo.clone();
            
            match async {
                let pipeline_files = find_pipeline_files(&repo_path).await?;
                let mut repo_findings = Vec::new();
                
                for file in pipeline_files {
                    if let Ok(content) = fs::read_to_string(&file).await {
                        let mut matches = Vec::new();
                        for (i, line) in content.lines().enumerate() {
                            if line.contains(&query) {
                                matches.push((i + 1, line.trim().to_string()));
                            }
                        }
                        
                        if !matches.is_empty() {
                            repo_findings.push(Finding {
                                file,
                                matches,
                            });
                        }
                    }
                }
                
                Ok::<_, anyhow::Error>(SearchResult {
                    repo: repo_name, // Use the cloned name here
                    findings: repo_findings,
                })
            }.await {
                Ok(result) => {
                    let _ = tx.send(Ok(result)).await;
                }
                Err(e) => {
                    let _ = tx.send(Err(e)).await;
                }
            }
        });
    }
    
    // Drop the original sender so the channel can close
    drop(tx);
    
    // Collect and process results
    let mut results = Vec::new();
    while let Some(result) = rx.recv().await {
        match result {
            Ok(search_result) => {
                if !search_result.findings.is_empty() {
                    results.push(search_result);
                }
            }
            Err(e) => {
                eprintln!("Error processing repository: {}", e);
            }
        }
    }

    // Sort and display results
    results.sort_by(|a, b| a.repo.cmp(&b.repo));
    
    for result in results {
        println!("\nRepository: {}", result.repo);
        println!("{}", "-".repeat(60));
        
        for finding in result.findings {
            println!("  File: {}", finding.file.strip_prefix(&result.repo)
                .unwrap_or(&finding.file)
                .display());
            
            for (line_num, line) in finding.matches {
                println!("    Line {}: {}", line_num, line);
            }
        }
    }

    Ok(())
}

// Add this enum to represent supported task types
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, ValueEnum)]
pub enum SupportedTask {
    Gitversion,
    // Future tasks can be added here
}

impl std::fmt::Display for SupportedTask {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SupportedTask::Gitversion => write!(f, "gitversion"),
        }
    }
}

async fn search_gitversion_tasks(repos: &[String], _verbose: bool) -> Result<()> {
    let db = Database::new()?;
    ensure_all_repos_exist(&db).await?;
    let valid_states = db.list_valid_states(&SupportedTask::Gitversion)?;
    let valid_states: Vec<GitVersionState> = valid_states.into_iter()
        .map(|state| {
            match state {
                TaskValidState::Gitversion(gv) => gv,
            }
        })
        .collect();

    let mut any_invalid = false;
    println!("\nChecking GitVersion implementations:");
    println!("{}", "-".repeat(60));

    for repo_url in repos {
        let repo_path = db.get_local_path(repo_url);
        let repo_name = repo_url
            .split('/')
            .last()
            .unwrap_or(repo_url)
            .trim_end_matches(".git");

        let pipeline_files = find_pipeline_files(&repo_path).await?;
        let mut implementations = Vec::new();
        
        // Group setup and execute tasks per file
        for file in pipeline_files {
            let content = std::fs::read_to_string(&file)
                .with_context(|| format!("Failed to read {}", file.display()))?;
            
            let lines: Vec<&str> = content.lines().collect();
            let mut current_impl = GitVersionImplementation {
                setup: None,
                execute: None,
                file_path: file.clone(),  // Store the file path
            };
            
            for (i, line) in lines.iter().enumerate() {
                let line_trimmed = line.trim();
                if line_trimmed.starts_with('#') {
                    continue;
                }
                
                if line_trimmed.contains("task: gitversion/") {
                    let task_name = line_trimmed.split('@')
                        .next()
                        .unwrap_or("")
                        .split(':')
                        .nth(1)
                        .unwrap_or("")
                        .trim();
                    let version = line_trimmed.split('@')
                        .nth(1)
                        .unwrap_or("")
                        .trim()
                        .to_string();

                    // Look ahead for versionSpec
                    let mut version_spec = None;
                    for next_line in lines.iter().skip(i + 1).take(10) {
                        let next_trimmed = next_line.trim();
                        if next_trimmed.contains("versionSpec:") {
                            version_spec = Some(
                                next_trimmed
                                    .split(':')
                                    .nth(1)
                                    .unwrap_or("")
                                    .trim()
                                    .trim_matches('\'')
                                    .trim_matches('"')
                                    .to_string()
                            );
                            break;
                        }
                        if next_trimmed.contains("task:") {
                            break;
                        }
                    }

                    match task_name {
                        "gitversion/setup" => {
                            current_impl.setup = Some((version, version_spec));
                        }
                        "gitversion/execute" => {
                            current_impl.execute = Some(version);
                        }
                        _ => {}
                    }
                }
            }

            // If we found any GitVersion tasks, add the implementation
            if current_impl.setup.is_some() || current_impl.execute.is_some() {
                implementations.push(current_impl);
            }
        }
        
        if !implementations.is_empty() {
            for impl_ in &implementations {
                let mut is_valid = false;

                // Validate against valid states
                for state in &valid_states {
                    let setup_matches = impl_.setup.as_ref().map_or(false, |(version, spec)| {
                        version == &state.setup_version && 
                        spec.as_ref().map_or(false, |s| s == &state.spec_version)
                    });
                    
                    let execute_matches = impl_.execute.as_ref().map_or(false, |version| {
                        version == &state.execute_version
                    });

                    if setup_matches && execute_matches {
                        is_valid = true;
                        break;
                    }
                }

                if !is_valid {
                    any_invalid = true;
                }

                // Count implementations for this file path
                let _impl_count = implementations.iter()
                    .filter(|other_impl| other_impl.file_path == impl_.file_path)
                    .count();

                // Format setup version
                let status = if is_valid { "✓" } else { "✗" };
                
                let setup_info = impl_.setup.as_ref()
                    .map_or("setup@none".to_string(), |(v, _)| {
                        format!("setup@{}", v)
                    });

                let execute_info = impl_.execute.as_ref()
                    .map_or("execute@none".to_string(), |v| {
                        format!("execute@{}", v)
                    });

                let spec_info = impl_.setup.as_ref()
                    .and_then(|(_, spec)| spec.as_ref())
                    .map_or("spec@none".to_string(), |s| {
                        format!("spec@{}", s)
                    });

                // Count implementations for this repo
                let repo_impl_count = implementations.iter()
                    .filter(|other_impl| {
                        other_impl.setup == impl_.setup && 
                        other_impl.execute == impl_.execute
                    })
                    .count();

                // Format file path if needed
                let path_info = if repo_impl_count > 1 {
                    let relative_path = impl_.file_path
                        .strip_prefix(&repo_path)
                        .unwrap_or(&impl_.file_path)
                        .display();
                    format!(" ({})", relative_path)
                } else {
                    String::new()
                };

                println!("{} {:<25} {} | {} | {}{}", 
                    status,
                    repo_name,
                    setup_info,
                    execute_info,
                    spec_info,
                    path_info
                );
            }
        }
    }

    // If any implementations were invalid, show valid states at the bottom
    if any_invalid {
        println!("\nValid states:");
        for state in &valid_states {
            println!("  - setup@{} | execute@{} | spec@{}", 
                state.setup_version,
                state.execute_version,
                state.spec_version
            );
        }
    }

    Ok(())
}

async fn collect_task_usage(repos: &[String]) -> Result<()> {
    let mut task_map: HashMap<String, HashMap<String, Vec<(String, PathBuf)>>> = HashMap::new();
    
    for repo_url in repos {
        let db = Database::new()?;
        let repo_path = db.get_local_path(repo_url);
        // Extract just the repository name from the URL
        let repo_name = repo_url
            .split('/')
            .last()
            .unwrap_or(repo_url)
            .trim_end_matches(".git");

        let pipeline_files = find_pipeline_files(&repo_path).await?;
        
        for file in pipeline_files {
            let content = std::fs::read_to_string(&file)?;
            let lines: Vec<&str> = content.lines()
                .map(|line| line.trim())
                .filter(|line| !line.starts_with('#') && !line.starts_with("//"))
                .collect();

            let task_regex = Regex::new(r#"task:\s*([\w/]+)@(\d+)"#)?;

            for line in lines {
                if let Some(cap) = task_regex.captures(line) {
                    let task_name = cap[1].to_string();
                    let version = cap[2].to_string();
                    
                    task_map
                        .entry(task_name)
                        .or_default()
                        .entry(version)
                        .or_default()
                        .push((repo_name.to_string(), file.clone()));
                }
            }
        }
    }
    
    // Display results
    println!("Task Usage Analysis:");
    println!("------------------------------------------------------------");
    
    // Sort tasks by name
    let mut task_names: Vec<_> = task_map.keys().collect();
    task_names.sort();
    
    for task_name in task_names {
        println!("\n{}", task_name);
        
        let versions = task_map.get(task_name).unwrap();
        let mut version_nums: Vec<_> = versions.keys().collect();
        version_nums.sort();
        
        for version in version_nums {
            let repos = versions.get(version).unwrap();
            
            // Group by repo name (in case of multiple files in same repo)
            let mut repo_groups: HashMap<String, Vec<PathBuf>> = HashMap::new();
            for (repo_name, file_path) in repos {
                repo_groups
                    .entry(repo_name.clone())
                    .or_default()
                    .push(file_path.clone());
            }
            
            println!("    @{}", version);
            
            // Sort repos by name
            let mut repo_names: Vec<_> = repo_groups.keys().collect();
            repo_names.sort();
            
            for repo_name in repo_names {
                let paths = repo_groups.get(repo_name).unwrap();
                if paths.len() > 1 {
                    // Multiple files in same repo
                    println!("        {} ({} occurrences)", repo_name, paths.len());
                    for path in paths {
                        if let Ok(rel_path) = path.strip_prefix(repo_name) {
                            println!("            - {}", rel_path.display());
                        }
                    }
                } else {
                    println!("        {}", repo_name);
                }
            }
        }
    }
    
    Ok(())
}

#[derive(Debug)]
struct TaskImplementation {
    repo_name: String,
    version: String,
    file_path: PathBuf,
}

#[derive(Default)]
struct TaskIssues {
    missing_states: HashSet<String>,
    invalid_states: HashMap<String, Vec<String>>,
}

async fn check_all_task_implementations(repos: &[String], issues: Option<&mut TaskIssues>) -> Result<()> {
    let db = Database::new()?;
    let mut local_issues = TaskIssues::default();
    let issues_ref = issues.unwrap_or(&mut local_issues);
    
    // First ensure all repos exist locally
    ensure_all_repos_exist(&db).await?;
    
    // First, collect all tasks from all repositories
    let mut task_implementations: HashMap<String, Vec<TaskImplementation>> = HashMap::new();
    
    for repo_url in repos {
        let repo_path = db.get_local_path(repo_url);
        let repo_name = repo_url
            .split('/')
            .last()
            .unwrap_or(repo_url);

        let pipeline_files = find_pipeline_files(&repo_path).await?;
        
        for file in pipeline_files {
            let content = std::fs::read_to_string(&file)?;
            
            // Regular expression to match task definitions
            let task_regex = Regex::new(r#"task:\s*([\w/]+)@(\d+)"#)?;
            
            let lines: Vec<&str> = content.lines()
                .map(|line| line.trim())
                .filter(|line| !line.starts_with('#') && !line.starts_with("//"))
                .collect();

            for line in lines {
                if let Some(cap) = task_regex.captures(line) {
                    let task_name = cap[1].to_string();
                    let version = cap[2].to_string();
                    
                    task_implementations
                        .entry(task_name)
                        .or_default()
                        .push(TaskImplementation {
                            repo_name: repo_name.to_string(),
                            version,
                            file_path: file.clone(),
                        });
                }
            }
        }
    }
    
    // Sort task names for consistent output
    let mut task_names: Vec<_> = task_implementations.keys().collect();
    task_names.sort();
    
    // Process each task
    for task_name in task_names {
        println!("\nChecking {} implementations:", task_name);
        println!("------------------------------------------------------------");
        
        let _implementations = task_implementations.get(task_name).unwrap();
        
        // Special handling for GitVersion tasks
        if task_name == "gitversion/setup" || task_name == "gitversion/execute" {
            // Get valid states from database
            let valid_states = db.list_valid_states(&SupportedTask::Gitversion)?;
            let valid_states: Vec<GitVersionState> = valid_states.into_iter()
                .map(|state| {
                    match state {
                        TaskValidState::Gitversion(gv) => gv,
                    }
                })
                .collect();

            // Group implementations by repo to collect both setup and execute
            let mut repo_implementations: HashMap<String, Vec<(&TaskImplementation, &str)>> = HashMap::new();
            
            // Get both setup and execute implementations
            let empty_vec = Vec::new();
            let setup_impls = task_implementations.get("gitversion/setup").unwrap_or(&empty_vec);
            let execute_impls = task_implementations.get("gitversion/execute").unwrap_or(&empty_vec);
            
            // Combine both sets of implementations
            for impl_ in setup_impls {
                repo_implementations
                    .entry(impl_.repo_name.clone())
                    .or_default()
                    .push((impl_, "gitversion/setup"));
            }
            
            for impl_ in execute_impls {
                repo_implementations
                    .entry(impl_.repo_name.clone())
                    .or_default()
                    .push((impl_, "gitversion/execute"));
            }

            // Sort repos for consistent output
            let mut repo_names: Vec<_> = repo_implementations.keys().collect();
            repo_names.sort();

            for repo_name in &repo_names {
                let impls = repo_implementations.get(*repo_name).unwrap();
                
                // Find matching setup and execute implementations
                let mut setup_version = None;
                let mut execute_version = None;
                let mut spec_version = None;
                let mut file_path = None;

                for (impl_, task_type) in impls {
                    match *task_type {
                        "gitversion/setup" => {
                            setup_version = Some(impl_.version.clone());
                            file_path = Some(impl_.file_path.clone());
                            
                            // Extract spec version from file content
                            if let Ok(content) = std::fs::read_to_string(&impl_.file_path) {
                                let lines: Vec<&str> = content.lines().collect();
                                for (i, line) in lines.iter().enumerate() {
                                    if line.contains("task: gitversion/setup") {
                                        // Look ahead for versionSpec
                                        for next_line in lines.iter().skip(i + 1).take(10) {
                                            let next_trimmed = next_line.trim();
                                            if next_trimmed.contains("versionSpec:") {
                                                spec_version = Some(
                                                    next_trimmed
                                                        .split(':')
                                                        .nth(1)
                                                        .unwrap_or("")
                                                        .trim()
                                                        .trim_matches('\'')
                                                        .trim_matches('"')
                                                        .to_string()
                                                );
                                                break;
                                            }
                                            if next_trimmed.contains("task:") {
                                                break;
                                            }
                                        }
                                    }
                                }
                            }
                        },
                        "gitversion/execute" => {
                            execute_version = Some(impl_.version.clone());
                        },
                        _ => {}
                    }
                }

                // Validate against valid states
                let mut is_valid = false;
                for state in &valid_states {
                    if setup_version.as_ref().map_or(false, |v| v == &state.setup_version) &&
                       execute_version.as_ref().map_or(false, |v| v == &state.execute_version) &&
                       spec_version.as_ref().map_or(false, |s| s == &state.spec_version) {
                        is_valid = true;
                        break;
                    }
                }

                let status = if is_valid { "✓" } else { "✗" };
                let path_info = if let Some(path) = &file_path {
                    format!(" ({})", 
                        path.strip_prefix(repo_name)
                            .unwrap_or(path)
                            .display())
                } else {
                    String::new()
                };

                println!("{} {:<25} setup@{} | execute@{} | spec@{}{}", 
                    status,
                    repo_name,
                    setup_version.unwrap_or_else(|| "?".to_string()),
                    execute_version.unwrap_or_else(|| "?".to_string()),
                    spec_version.unwrap_or_else(|| "?".to_string()),
                    path_info
                );

                // Track invalid states
                if !is_valid {
                    issues_ref.invalid_states
                        .entry(task_name.to_string())
                        .or_default()
                        .push(repo_name.to_string());
                }
            }

            // Show valid states if we're processing setup tasks
            if task_name == "gitversion/setup" {
                println!("\nValid states:");
                for state in &valid_states {
                    println!("  - setup@{} | execute@{} | spec@{}", 
                        state.setup_version,
                        state.execute_version,
                        state.spec_version
                    );
                }
            }
        } else {
            // For other tasks, track that they need state definitions
            issues_ref.missing_states.insert(task_name.to_string());
            // Handle other tasks
            let implementations = task_implementations.get(task_name).unwrap();
            
            // Group by repo name for consistent display
            let mut repo_implementations: HashMap<String, Vec<&TaskImplementation>> = HashMap::new();
            for impl_ in implementations {
                repo_implementations
                    .entry(impl_.repo_name.clone())
                    .or_default()
                    .push(impl_);
            }

            // Sort repos for consistent output
            let mut repo_names: Vec<_> = repo_implementations.keys().collect();
            repo_names.sort();

            for repo_name in &repo_names {
                let impls = repo_implementations.get(*repo_name).unwrap();
                
                for impl_ in impls {
                    let path_info = if let Ok(rel_path) = impl_.file_path.strip_prefix(repo_name) {
                        format!(" ({})", rel_path.display())
                    } else {
                        String::new()
                    };

                    println!("? {:<25} @{}{}", 
                        repo_name,
                        impl_.version,
                        path_info
                    );
                }
            }

            // Show message about needing valid states
            println!("\nNeeds definition for valid state(s).");
        }
    }

    // At the end, print summary only if we're using local issues
    if std::ptr::eq(issues_ref, &mut local_issues) {
        println!("\nNeeds Fixing:");
        println!("------------------------------------------------------------");
        
        if !local_issues.missing_states.is_empty() {
            println!("    - {} tasks missing valid state definitions:", local_issues.missing_states.len());
            for task in local_issues.missing_states.iter().sorted() {
                println!("        - {}", task);
            }
        }

        if !local_issues.invalid_states.is_empty() {
            let total_invalid = local_issues.invalid_states.values()
                .map(|repos| repos.len())
                .sum::<usize>();
            println!("\n    - {} implementations with invalid states:", total_invalid);
            for (task, repos) in local_issues.invalid_states.iter().sorted() {
                println!("        - {} ({} repos):", task, repos.len());
                for repo in repos.iter().sorted() {
                    println!("            - {}", repo);
                }
            }
        }
    }

    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let db = Database::new()?;
    handle_cli_args(&cli, &db).await?;
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
enum TaskValidState {
    Gitversion(GitVersionState),
    // Future task states can be added here
}

fn format_task_states(task: &SupportedTask, states: Vec<TaskValidState>) -> String {
    match task {
        SupportedTask::Gitversion => {
            if states.is_empty() {
                "None".to_string()
            } else {
                states.into_iter()
                    .map(|state| match state {
                        TaskValidState::Gitversion(gv) => {
                            format!("- setup@{}, execute@{}, spec@{}",
                                gv.setup_version,
                                gv.execute_version,
                                gv.spec_version)
                        }
                    })
                    .collect::<Vec<String>>()
                    .join("\n")
            }
        }
    }
}

async fn collect_task_usage_data(repos: &[String]) -> Result<HashMap<String, HashMap<String, HashMap<String, Vec<PathBuf>>>>> {
    let mut handles = Vec::new();
    
    for repo_url in repos {
        let repo_url = repo_url.clone();
        let db = Database::new()?;
        
        let handle = tokio::spawn(async move {
            let mut repo_task_map = HashMap::new();
            let repo_path = db.get_local_path(&repo_url);
            let repo_name = repo_url
                .split('/')
                .last()
                .unwrap_or(&repo_url);

            let pipeline_files = find_pipeline_files(&repo_path).await?;
            
            for file in pipeline_files {
                let content = std::fs::read_to_string(&file)?;
                let task_regex = Regex::new(r#"task:\s*([\w/]+)@(\d+)"#)?;
                
                for cap in task_regex.captures_iter(&content) {
                    let task_name = cap[1].to_string();
                    let version = cap[2].to_string();
                    
                    repo_task_map
                        .entry(task_name)
                        .or_insert_with(HashMap::new)
                        .entry(version)
                        .or_insert_with(HashMap::new)
                        .entry(repo_name.to_string())
                        .or_insert_with(Vec::new)
                        .push(file.clone());
                }
            }
            
            Ok::<_, anyhow::Error>(repo_task_map)
        });
        
        handles.push(handle);
    }

    // Collect and merge results
    let mut final_map = HashMap::new();
    for handle in handles {
        let repo_map = handle.await??;
        
        for (task_name, versions) in repo_map {
            let task_entry = final_map.entry(task_name).or_insert_with(HashMap::new);
            for (version, repos) in versions {
                task_entry.entry(version).or_insert_with(HashMap::new).extend(repos);
            }
        }
    }

    Ok(final_map)
}

async fn ensure_all_repos_exist(db: &Database) -> Result<()> {
    let credentials = db.get_git_credentials()?
        .ok_or_else(|| anyhow::anyhow!("Git credentials not found"))?;

    // Ensure temp_repos directory exists before starting any operations
    let temp_dir = std::env::current_dir()?.join("temp_repos");
    tokio::fs::create_dir_all(&temp_dir).await?;

    // Create a semaphore to limit concurrent git operations
    let semaphore = Arc::new(Semaphore::new(4));
    let mut handles = Vec::new();

    for repo_url in db.list_repositories()? {
        let permit = semaphore.clone().acquire_owned().await?;
        let creds = (credentials.0.clone(), credentials.1.clone());
        
        handles.push(tokio::spawn(async move {
            let git_manager = GitManager::new(creds.0, creds.1, &repo_url);
            let result = git_manager.ensure_repo_exists().await;
            drop(permit);
            result
        }));
    }

    // Wait for all updates to complete
    for handle in handles {
        handle.await??;
    }

    Ok(())
}
