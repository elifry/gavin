use anyhow::Result;
use gavin::config::Config;
use gavin::{Database, GitVersionState, SupportedTask, TaskValidState};
use std::env;
use tempfile::tempdir;

#[tokio::test]
async fn test_task_state_operations() -> Result<()> {
    // Create temporary directory for test database
    let temp_dir = tempdir()?;
    env::set_current_dir(temp_dir.path())?;

    let db = Database::new()?;

    // Test PowerShell state
    let powershell_task = SupportedTask::Default("powershell".to_string());
    let powershell_state = TaskValidState::Default("2".to_string());

    // Add state
    db.add_valid_state(&powershell_task, &powershell_state)
        .unwrap();

    // List states
    let states = db.list_valid_states(&powershell_task).unwrap();
    assert_eq!(states.len(), 1);
    assert_eq!(states[0], powershell_state);

    // Test case insensitivity
    let powershell_upper = SupportedTask::Default("PowerShell".to_string());
    let states_upper = db.list_valid_states(&powershell_upper).unwrap();
    assert_eq!(states_upper.len(), 1);
    assert_eq!(states_upper[0], powershell_state);

    // Delete state
    db.delete_valid_state(&powershell_task, &powershell_state)?;
    let states_after_delete = db.list_valid_states(&powershell_task)?;
    assert_eq!(states_after_delete.len(), 0);

    Ok(())
}

#[tokio::test]
async fn test_config_merge() -> Result<()> {
    // Create temporary directory for test database
    let temp_dir = tempdir()?;
    env::set_current_dir(temp_dir.path())?;

    let db = Database::new()?;

    // Create a test config
    let mut config = Config {
        task_states: Default::default(),
    };

    // Add GitVersion state
    config
        .task_states
        .gitversion
        .push(GitVersionState::new("3", "3", "6.0.3"));

    // Add PowerShell state
    config.task_states.other_tasks.insert(
        "powershell".to_string(),
        vec!["2.0".to_string(), "3.0".to_string()],
    );

    // Merge config into database
    db.merge_config_states(&config)?;

    // Verify GitVersion states
    let gitversion_states = db.list_valid_states(&SupportedTask::Gitversion)?;
    assert_eq!(gitversion_states.len(), 1);
    match &gitversion_states[0] {
        TaskValidState::Gitversion(gv) => {
            assert_eq!(gv.setup_version, "3");
            assert_eq!(gv.execute_version, "3");
            assert_eq!(gv.spec_version, "6.0.3");
        }
        _ => panic!("Expected GitVersion state"),
    }

    // Verify PowerShell states
    let powershell_states =
        db.list_valid_states(&SupportedTask::Default("powershell".to_string()))?;
    assert_eq!(powershell_states.len(), 2);
    assert!(powershell_states
        .iter()
        .any(|s| matches!(s, TaskValidState::Default(v) if v == "2.0")));
    assert!(powershell_states
        .iter()
        .any(|s| matches!(s, TaskValidState::Default(v) if v == "3.0")));

    Ok(())
}

#[tokio::test]
async fn test_case_insensitive_task_names() -> Result<()> {
    let temp_dir = tempdir()?;
    env::set_current_dir(temp_dir.path())?;
    let db = Database::new()?;

    // Add test states with different cases
    let task_lower = SupportedTask::Default("powershell".to_string());
    let state1 = TaskValidState::Default("1.0".to_string());
    let state2 = TaskValidState::Default("2.0".to_string());

    db.add_valid_state(&task_lower, &state1)?;
    db.add_valid_state(&task_lower, &state2)?;

    // Test retrieval with different cases
    let lower_case = db.list_valid_states(&SupportedTask::Default("powershell".to_string()))?;
    let upper_case = db.list_valid_states(&SupportedTask::Default("POWERSHELL".to_string()))?;

    // Print the actual states for debugging
    println!("Lower case states: {:?}", lower_case);
    println!("Upper case states: {:?}", upper_case);

    assert_eq!(lower_case.len(), 2);
    assert_eq!(upper_case.len(), 2);

    // Verify the actual states match
    assert!(lower_case.contains(&state1));
    assert!(lower_case.contains(&state2));
    assert!(upper_case.contains(&state1));
    assert!(upper_case.contains(&state2));

    Ok(())
}

// Add more test cases...
