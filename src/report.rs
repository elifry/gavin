use crate::database::Database;
use crate::{collect_task_usage_data, format_task_states, TaskIssues};
use anyhow::Result;
use itertools::Itertools;
use std::collections::HashMap;
use std::path::PathBuf;

pub async fn generate_markdown_report(
    repos: &[String],
    db: &Database,
    issues: &TaskIssues,
) -> Result<String> {
    let mut md = String::new();

    generate_header(&mut md);
    generate_summary_section(&mut md, issues)?;
    generate_valid_states_section(&mut md, db).await?;
    generate_issues_section(&mut md, issues)?;
    generate_implementation_details(&mut md, issues)?;
    generate_task_usage_section(&mut md, repos).await?;

    Ok(md)
}

fn generate_header(md: &mut String) {
    let now = chrono::Local::now();
    md.push_str("# Pipeline Task Analysis Report\n\n");
    md.push_str(&format!(
        "Generated on: {}\n\n",
        now.format("%Y-%m-%d %H:%M:%S")
    ));
}

fn generate_summary_section(md: &mut String, issues: &TaskIssues) -> Result<()> {
    md.push_str("## Summary\n\n");

    // Count total implementations
    let total_impls = issues
        .all_implementations
        .values()
        .map(|v| v.len())
        .sum::<usize>();

    md.push_str(&format!(
        "- Total implementations analyzed: **{}**\n",
        total_impls
    ));
    md.push_str(&format!(
        "- Tasks with missing states: **{}**\n",
        issues.missing_states.len()
    ));

    let invalid_count = issues
        .invalid_states
        .values()
        .map(|repos| repos.len())
        .sum::<usize>();
    md.push_str(&format!(
        "- Invalid state implementations: **{}**\n\n",
        invalid_count
    ));

    Ok(())
}

fn generate_issues_section(md: &mut String, issues: &TaskIssues) -> Result<()> {
    if !issues.missing_states.is_empty() || !issues.invalid_states.is_empty() {
        md.push_str("## Issues Found\n\n");

        if !issues.missing_states.is_empty() {
            md.push_str("### Tasks Missing State Definitions\n\n");
            for task in &issues.missing_states {
                md.push_str(&format!("- {}\n", task));
            }
            md.push('\n');
        }

        if !issues.invalid_states.is_empty() {
            md.push_str("### Invalid State Implementations\n\n");
            for (task, repos) in &issues.invalid_states {
                md.push_str(&format!("#### {}\n\n", task));
                for (repo, impls) in repos {
                    md.push_str(&format!("- {}\n", repo));
                    for impl_ in impls {
                        md.push_str(&format!(
                            "  - {}: {}\n",
                            impl_.version,
                            impl_.file_path.display()
                        ));
                    }
                }
                md.push('\n');
            }
        }
    }
    Ok(())
}

fn generate_implementation_details(md: &mut String, issues: &TaskIssues) -> Result<()> {
    md.push_str("## Implementation Details\n\n");

    for (task, impls) in &issues.all_implementations {
        md.push_str(&format!("### {}\n\n", task));

        // Group implementations by version
        let mut by_version: HashMap<String, Vec<(&String, &PathBuf)>> = HashMap::new();

        for impl_ in impls {
            by_version
                .entry(impl_.version.clone())
                .or_default()
                .push((&impl_.repo_name, &impl_.file_path));
        }

        // Sort versions for consistent output
        let mut versions: Vec<_> = by_version.keys().collect();
        versions.sort();

        for version in versions {
            let impls = &by_version[version];
            md.push_str(&format!("#### Version {}\n\n", version));

            // Group by repo
            let mut by_repo: HashMap<&String, Vec<&PathBuf>> = HashMap::new();
            for (repo, path) in impls {
                by_repo.entry(repo).or_default().push(path);
            }

            for (repo, paths) in by_repo {
                if paths.len() > 1 {
                    md.push_str(&format!("- {} ({} occurrences)\n", repo, paths.len()));
                    for path in paths {
                        md.push_str(&format!("  - {}\n", path.display()));
                    }
                } else {
                    md.push_str(&format!("- {}: {}\n", repo, paths[0].display()));
                }
            }
            md.push('\n');
        }
    }
    Ok(())
}

async fn generate_valid_states_section(md: &mut String, db: &Database) -> Result<()> {
    md.push_str("## Valid Task States\n\n");
    for task in db.get_all_tasks()? {
        md.push_str(&format!(
            "### {}\n\n{}\n\n",
            task,
            format_task_states(&task, db.list_valid_states(&task)?)
        ));
    }
    Ok(())
}

async fn generate_task_usage_section(md: &mut String, repos: &[String]) -> Result<()> {
    md.push_str("## Task Usage Analysis\n\n");
    let task_usage = collect_task_usage_data(repos).await?;

    for task_name in task_usage.keys().sorted() {
        md.push_str(&format!("### {}\n\n", task_name));
        let versions = task_usage.get(task_name).unwrap();

        for version in versions.keys().sorted() {
            md.push_str(&format!("#### Version {}\n\n", version));
            let repos = versions.get(version).unwrap();

            for repo_url in repos.keys().sorted() {
                let repo_name = repo_url
                    .split('/')
                    .last()
                    .unwrap_or(repo_url)
                    .trim_end_matches(".git");

                let paths = repos.get(repo_url).unwrap();
                if paths.len() > 1 {
                    md.push_str(&format!("- {} ({} occurrences)\n", repo_name, paths.len()));
                    for path in paths {
                        if let Ok(rel_path) = path.strip_prefix(repo_name) {
                            md.push_str(&format!("  - {}\n", rel_path.display()));
                        }
                    }
                } else if let Some(path) = paths.first() {
                    if let Ok(rel_path) = path.strip_prefix(repo_name) {
                        md.push_str(&format!("- {}: {}\n", repo_name, rel_path.display()));
                    } else {
                        md.push_str(&format!("- {}: {}\n", repo_name, path.display()));
                    }
                }
            }
            md.push('\n');
        }
    }
    Ok(())
}
