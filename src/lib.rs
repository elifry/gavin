use serde::{Serialize, Deserialize};
use std::path::{Path, PathBuf};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use anyhow::{Result, Context};
use walkdir::WalkDir;
use tokio::sync::Semaphore;
use tokio::fs;
use regex::Regex;
// use itertools::Itertools;
use semver::Version;

// Re-export modules and types
pub mod database;
pub mod gitversion;
pub mod config;
pub mod utils;
pub mod cli;
pub mod cli_handler;
pub mod report;
pub mod git_manager;

// Re-export commonly used types
pub use database::Database;
pub use gitversion::{GitVersionState, GitVersionImplementation};
pub use config::Config;
pub use cli_handler::handle_cli_args;
pub use git_manager::GitManager;
pub use cli::Cli;

struct SearchResult {
    repo: String,
    findings: Vec<Finding>,
}

struct Finding {
    file: PathBuf,
    matches: Vec<(usize, String)>, // (line number, line content)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "value")]
pub enum TaskValidState {
    Gitversion(GitVersionState),
    Default(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum SupportedTask {
    Gitversion,
    Default(String),
}

#[derive(Debug, Clone, Ord, PartialOrd, Eq, PartialEq)]
pub struct TaskImplementation {
    repo_name: String,
    version: String,
    file_path: PathBuf,
}

// Add a custom parser for clap
impl std::str::FromStr for SupportedTask {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "gitversion" => Ok(SupportedTask::Gitversion),
            other => Ok(SupportedTask::Default(other.to_string())),
        }
    }
}

impl std::fmt::Display for SupportedTask {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SupportedTask::Gitversion => write!(f, "gitversion"),
            SupportedTask::Default(name) => write!(f, "{}", name),
        }
    }
}

impl std::fmt::Display for TaskValidState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TaskValidState::Gitversion(state) => write!(
                f, 
                "setup@{}, execute@{}, spec@{}", 
                state.setup_version, 
                state.execute_version, 
                state.spec_version
            ),
            TaskValidState::Default(version) => write!(f, "@{}", version),
        }
    }
}

impl SupportedTask {
    pub fn get_all_variants() -> Vec<Self> {
        vec![
            Self::Gitversion,
            // Add any known default tasks here if needed
        ]
    }
}

pub fn format_task_states(_task: &SupportedTask, states: Vec<TaskValidState>) -> String {
    if states.is_empty() {
        return "None".to_string();
    }

    states.iter()
        .map(|state| format!("- {}", state))
        .collect::<Vec<_>>()
        .join("\n")
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
                if path.extension().map_or(false, |ext| ext == "yml" || ext == "yaml") && path.to_string_lossy().contains("pipeline") && tx.blocking_send(path).is_err() {
                    break; // Channel closed, receiver dropped
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

async fn search_in_pipelines_concurrent(repos: &[String], query: &str) -> Result<()> {
    let max_concurrent = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    let semaphore = Arc::new(Semaphore::new(max_concurrent));
    let db = Database::new()?;
    ensure_all_repos_exist(&db, false).await?;
    
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

pub fn parse_task_name(name: &str) -> Result<SupportedTask> {
    if name.eq_ignore_ascii_case("gitversion") {
        Ok(SupportedTask::Gitversion)
    } else {
        Ok(SupportedTask::Default(name.to_string()))
    }
}

async fn search_gitversion_tasks(repos: &[String], _verbose: bool) -> Result<()> {
    let db = Database::new()?;
    ensure_all_repos_exist(&db, false).await?;
    let valid_states = db.list_valid_states(&SupportedTask::Gitversion)?;
    let valid_states: Vec<GitVersionState> = valid_states.into_iter()
        .map(|state| {
            match state {
                TaskValidState::Gitversion(gv) => gv,
                TaskValidState::Default(_) => panic!("Expected GitVersion state, got Default state"),
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

pub(crate) async fn check_all_task_implementations(
    repos: &[String], 
    issues: Option<&mut TaskIssues>, 
    no_update: bool
) -> Result<TaskIssues> {
    let db = Database::new()?;
    let mut local_issues = TaskIssues::default();
    let issues_ref = issues.unwrap_or(&mut local_issues);
    
    // First ensure all repos exist locally
    ensure_all_repos_exist(&db, no_update).await?;
    
    // First, collect all tasks from all repositories
    let mut task_implementations: HashMap<String, Vec<TaskImplementation>> = HashMap::new();
    
    for repo_url in repos {
        let repo_path = db.get_local_path(repo_url);
        let repo_name = repo_url
            .split('/')
            .last()
            .unwrap_or(repo_url);

        let pipeline_files = find_pipeline_files(&repo_path).await?;
        
        for pipeline_file in pipeline_files {
            let content = std::fs::read_to_string(&pipeline_file)?;
            
            // Regular expression to match task definitions
            let task_regex = Regex::new(r#"task:\s*([\w/]+)@(\d+)"#)?;
            
            let lines: Vec<&str> = content.lines()
                .map(|line| line.trim())
                .filter(|line| !line.starts_with('#') && !line.starts_with("//"))
                .collect();

            for line in lines {
                if let Some(cap) = task_regex.captures(line) {
                    let task_name = cap[1].to_string();
                    let task_version = cap[2].to_string();
                    
                    task_implementations
                        .entry(task_name)
                        .or_default()
                        .push(TaskImplementation {
                            repo_name: repo_name.to_string(),
                            version: task_version,
                            file_path: pipeline_file.clone(),
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
        
        let implementations = task_implementations.get(task_name).unwrap();
        issues_ref.all_implementations.insert(task_name.clone(), implementations.clone());
        
        // Special handling for GitVersion tasks
        if task_name == "gitversion/setup" || task_name == "gitversion/execute" {
            // Get valid states from database
            let valid_states = db.list_valid_states(&SupportedTask::Gitversion)?;
            let valid_states: Vec<GitVersionState> = valid_states.into_iter()
                .map(|state| {
                    match state {
                        TaskValidState::Gitversion(gv) => gv,
                        TaskValidState::Default(_) => panic!("Expected GitVersion state, got Default state"),
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
                let default_str = "?".to_string();
                println!("{} {:<25} {} | {} | {}{}", 
                    status,
                    repo_name,
                    setup_version.as_ref().unwrap_or(&default_str),
                    execute_version.unwrap_or_else(|| "?".to_string()),
                    spec_version.unwrap_or_else(|| "?".to_string()),
                    path_info
                );
                // Track invalid states
                if !is_valid {
                    issues_ref.invalid_states
                        .entry(task_name.to_string())
                        .or_default()
                        .entry(repo_name.to_string())
                        .or_default()
                        .push(TaskImplementation {
                            repo_name: repo_name.to_string(),
                            version: setup_version.unwrap_or_else(|| "?".to_string()),
                            file_path: file_path.clone().unwrap_or_default(),
                        });
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
            // Handle other tasks
            let task = SupportedTask::Default(task_name.clone());
            let valid_states = db.list_valid_states(&task)?;
            
            if valid_states.is_empty() {
                issues_ref.missing_states.insert(task_name.clone());
            }
            
            for implementation in implementations {
                let is_valid = valid_states.iter().any(|state| {
                    matches!(state, TaskValidState::Default(v) if v == &implementation.version)
                });
                
                if !is_valid {
                    issues_ref.invalid_states
                        .entry(task_name.to_string())
                        .or_default()
                        .entry(implementation.repo_name.clone())
                        .or_default()
                        .push(implementation.clone());
                }
            }
        }
    }
    
    Ok(local_issues)
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

async fn ensure_all_repos_exist(db: &Database, skip_update: bool) -> Result<()> {
    let credentials = db.get_git_credentials()?
        .ok_or_else(|| anyhow::anyhow!("Git credentials not found"))?;

    let temp_dir = std::env::current_dir()?.join("temp_repos");
    tokio::fs::create_dir_all(&temp_dir).await?;

    let semaphore = Arc::new(Semaphore::new(4));
    let mut handles = Vec::new();

    for repo_url in db.list_repositories()? {
        let permit = semaphore.clone().acquire_owned().await?;
        let creds = (credentials.0.clone(), credentials.1.clone());
        
        handles.push(tokio::spawn(async move {
            let git_manager = GitManager::new(creds.0, creds.1, &repo_url);
            let result = if skip_update {
                git_manager.ensure_repo_exists_no_update().await
            } else {
                git_manager.ensure_repo_exists().await
            };
            drop(permit);
            result
        }));
    }

    for handle in handles {
        handle.await??;
    }

    Ok(())
}

pub async fn search_default_task(repos: &[String], task_name: &str, verbose: bool) -> Result<()> {
    let db = Database::new()?;
    ensure_all_repos_exist(&db, false).await?;
    let valid_states = db.list_valid_states(&SupportedTask::Default(task_name.to_string()))?;
    
    println!("\nChecking {} implementations:", task_name);
    println!("{}", "-".repeat(60));

    for repo_url in repos {
        let repo_path = db.get_local_path(repo_url);
        let pipeline_files = find_pipeline_files(&repo_path).await?;
        
        for file in pipeline_files {
            let content = std::fs::read_to_string(&file)?;
            let task_regex = Regex::new(&format!(r#"task:\s*{}\s*@(\d+)"#, task_name))?;
            
            for cap in task_regex.captures_iter(&content) {
                let version = cap[1].to_string();
                let is_valid = valid_states.iter().any(|state| {
                    matches!(state, TaskValidState::Default(v) if v == &version)
                });

                let status = if is_valid { "✓" } else { "✗" };
                let path_info = file.strip_prefix(&repo_path)
                    .map_or_else(|_| file.display().to_string(),
                               |p| p.display().to_string());

                println!("{} {:<25} @{} ({})", 
                    status,
                    repo_url.split('/').last().unwrap_or(repo_url),
                    version,
                    path_info
                );

                if verbose {
                    println!("    Valid versions: {:?}", valid_states.iter()
                        .filter_map(|s| match s {
                            TaskValidState::Default(v) => Some(v),
                            _ => None
                        })
                        .collect::<Vec<_>>());
                }
            }
        }
    }

    Ok(())
}

#[derive(Default)]
pub struct TaskIssues {
    pub missing_states: HashSet<String>,
    pub invalid_states: HashMap<String, HashMap<String, Vec<TaskImplementation>>>,
    pub all_implementations: HashMap<String, Vec<TaskImplementation>>,
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

pub trait VersionCompare {
    fn version_eq(&self, other: &str) -> bool;
}

impl VersionCompare for String {
    fn version_eq(&self, other: &str) -> bool {
        // Normalize version strings
        let normalize = |v: &str| -> Result<Version> {
            // Convert single number to proper semver
            let v = if v.chars().all(|c| c.is_ascii_digit()) {
                format!("{}.0.0", v)
            } else if v.matches('.').count() == 1 {
                format!("{}.0", v)
            } else {
                v.to_string()
            };
            Version::parse(&v).map_err(|e| anyhow::anyhow!("Invalid version: {}", e))
        };

        match (normalize(self), normalize(other)) {
            (Ok(v1), Ok(v2)) => v1 == v2,
            _ => self == other // Fallback to string comparison if parsing fails
        }
    }
}

// Update TaskValidState equality comparison
impl PartialEq for TaskValidState {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (TaskValidState::Gitversion(a), TaskValidState::Gitversion(b)) => {
                a.setup_version.version_eq(&b.setup_version) &&
                a.execute_version.version_eq(&b.execute_version) &&
                a.spec_version.version_eq(&b.spec_version)
            },
            (TaskValidState::Default(a), TaskValidState::Default(b)) => {
                a.version_eq(b)
            },
            _ => false
        }
    }
}