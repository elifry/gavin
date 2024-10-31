use anyhow::Result;
use itertools::Itertools;
use chrono::Local;
use clap::ValueEnum;
use crate::{
    SupportedTask,
    TaskIssues,
    format_task_states,
    collect_task_usage_data,
    check_all_task_implementations,
    ensure_all_repos_exist,
};
use crate::database::Database;

pub async fn generate_markdown_report(repos: &[String], db: &Database, no_update: bool) -> Result<String> {
    ensure_all_repos_exist(db, no_update).await?;
    let mut md = String::new();
    let mut issues = TaskIssues::default();
    
    md.push_str("# Azure Pipeline Tasks Analysis\n\n");
    md.push_str(&format!("*Report generated on {}*\n\n", Local::now().format("%Y-%m-%d %H:%M:%S")));

    // Summary section
    md.push_str("## Summary\n\n");
    check_all_task_implementations(repos, Some(&mut issues), no_update).await?;
    
    generate_issues_summary(&mut md, &issues);
    generate_valid_states_section(&mut md, db).await?;
    generate_detailed_issues_section(&mut md, &issues);
    generate_task_usage_section(&mut md, repos).await?;

    Ok(md)
}

fn generate_issues_summary(md: &mut String, issues: &TaskIssues) {
    if !issues.missing_states.is_empty() || !issues.invalid_states.is_empty() {
        md.push_str("### Issues Requiring Attention\n\n");
        
        if !issues.missing_states.is_empty() {
            md.push_str(&format!("- **{}** tasks missing valid state definitions\n", 
                issues.missing_states.len()));
        }
        
        if !issues.invalid_states.is_empty() {
            let total_invalid = issues.invalid_states.values()
                .map(|repos| repos.len())
                .sum::<usize>();
            md.push_str(&format!("- **{}** implementations with invalid states across {} tasks\n", 
                total_invalid,
                issues.invalid_states.len()));
        }
        md.push_str("\n");
    }
}

async fn generate_valid_states_section(md: &mut String, db: &Database) -> Result<()> {
    md.push_str("## Valid Task States\n\n");
    for task in SupportedTask::value_variants() {
        md.push_str(&format!("### {}\n\n{}\n\n", 
            task, 
            format_task_states(task, db.list_valid_states(task)?)));
    }
    Ok(())
}

fn generate_detailed_issues_section(md: &mut String, issues: &TaskIssues) {
    if !issues.missing_states.is_empty() || !issues.invalid_states.is_empty() {
        md.push_str("## Detailed Issues\n\n");
        
        if !issues.missing_states.is_empty() {
            md.push_str("### Tasks Needing State Definitions\n\n");
            for task in issues.missing_states.iter().sorted() {
                md.push_str(&format!("- {}\n", task));
            }
            md.push_str("\n");
        }

        if !issues.invalid_states.is_empty() {
            md.push_str("### Invalid State Implementations\n\n");
            for (task, repos) in issues.invalid_states.iter().sorted() {
                md.push_str(&format!("#### {} ({} repos)\n\n", task, repos.len()));
                for repo in repos.iter().sorted() {
                    md.push_str(&format!("- {}\n", repo));
                }
                md.push_str("\n");
            }
        }
    }
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
                } else {
                    md.push_str(&format!("- {}\n", repo_name));
                }
            }
            md.push_str("\n");
        }
    }
    Ok(())
}
