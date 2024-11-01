use clap::Parser;
use crate::SupportedTask;

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
pub struct Cli {
    /// Search for a string in pipeline files
    #[arg(short = 's', long = "search")]
    pub search_string: Option<String>,

    /// List all repositories
    #[arg(long = "list-repos")]
    pub list_repos: bool,

    /// List all repositories with their pipeline files
    #[arg(long = "list-pipelines")]
    pub list_pipelines: bool,

    /// Add a repository to the database
    #[arg(long = "add-repo")]
    pub add_repo: Option<String>,

    /// Add multiple repositories to the database
    #[arg(long = "add-multiple-repos")]
    pub add_multiple_repos: Option<String>,

    /// When adding a repository, use this flag to skip checking if it exists locally
    #[arg(long = "new")]
    pub new: bool,

    /// Skip updating repositories before operations such as --check-tasks, --analyze-tasks, etc.
    #[arg(long = "no-update")]
    pub no_update: bool,

    /// Delete a repository from the database
    #[arg(long = "delete-repo")]
    pub delete_repo: Option<String>,

    /// Search for a specific task in pipelines (e.g., gitversion, powershell)
    #[arg(long = "search-task")]
    pub search_task: Option<SupportedTask>,

    /// Add a valid state for a task
    #[arg(long = "add-task-state")]
    pub add_task_state: Option<SupportedTask>,

    /// Delete a valid state for a task
    #[arg(long = "delete-task-state")]
    pub delete_task_state: Option<SupportedTask>,

    /// State value for --add-task-state or --delete-task-state (e.g., "setup:3,execute:3,spec:6.0.3")
    #[arg(long = "state-value")]
    pub state_value: Option<String>,

    /// List valid states for a specific task
    #[arg(long = "list-task-states")]
    pub list_task_states: Option<SupportedTask>,

    /// List valid states for all supported tasks
    #[arg(long = "list-all-task-states")]
    pub list_all_task_states: bool,

    /// Analyze task usage across all repositories
    #[arg(long = "analyze-tasks")]
    pub analyze_tasks: bool,

    /// Check all task implementations across repositories
    #[arg(long = "check-tasks")]
    pub check_tasks: bool,

    /// Output results as a markdown file (requires --check-tasks)
    #[arg(long = "output-markdown", requires = "check_tasks")]
    pub output_markdown: bool,

    /// Specify the output file path for markdown report (requires --output-markdown)
    #[arg(long = "report-path", requires = "output_markdown")]
    pub report_path: Option<String>,

    /// Show detailed output
    #[arg(short, long)]
    pub verbose: bool,

    /// Path to config file (defaults to ./gavinconfig.yml)
    #[arg(long = "config")]
    pub config_path: Option<String>,

    /// Set git credentials (username:token format)
    #[arg(long = "set-git-credentials")]
    pub set_git_credentials: Option<String>,
}
