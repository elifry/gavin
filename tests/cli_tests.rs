use anyhow::Result;
use gavin::{Database, GitVersionState, SupportedTask, TaskValidState};
use std::env;
use tempfile::tempdir;

#[tokio::test]
async fn test_task_state_deletion() -> Result<()> {
    // Setup
    let temp_dir = tempdir()?;
    env::set_current_dir(temp_dir.path())?;
    let db = Database::new()?;

    // Add a test state
    let task = SupportedTask::Default("copyfiles".to_string());
    let state = TaskValidState::Default("1".to_string());
    db.add_valid_state(&task, &state)?;

    // Verify state was added
    let states = db.list_valid_states(&task)?;
    assert_eq!(states.len(), 1);
    assert_eq!(states[0], state);

    // Delete the state
    db.delete_valid_state(&task, &state)?;

    // Verify state was deleted
    let states = db.list_valid_states(&task)?;
    assert_eq!(states.len(), 0);

    Ok(())
}

#[tokio::test]
async fn test_task_state_formatting() -> Result<()> {
    let temp_dir = tempdir()?;
    env::set_current_dir(temp_dir.path())?;
    let db = Database::new()?;

    // Test GitVersion state formatting
    let gv_task = SupportedTask::Gitversion;
    let gv_state = TaskValidState::Gitversion(GitVersionState::new("3", "3", "6.0.3"));
    db.add_valid_state(&gv_task, &gv_state)?;

    // Test Default task state formatting
    let default_task = SupportedTask::Default("copyfiles".to_string());
    let default_state = TaskValidState::Default("1".to_string());
    db.add_valid_state(&default_task, &default_state)?;

    // Verify state retrieval and formatting
    let gv_states = db.list_valid_states(&gv_task)?;
    let default_states = db.list_valid_states(&default_task)?;

    assert_eq!(gv_states.len(), 1);
    assert_eq!(default_states.len(), 1);

    // Test formatting
    if let TaskValidState::Gitversion(gv) = &gv_states[0] {
        assert_eq!(gv.setup_version, "3");
        assert_eq!(gv.execute_version, "3");
        assert_eq!(gv.spec_version, "6.0.3");
    } else {
        panic!("Expected GitVersion state");
    }

    if let TaskValidState::Default(version) = &default_states[0] {
        assert_eq!(version, "1");
    } else {
        panic!("Expected Default state");
    }

    Ok(())
}

#[tokio::test]
async fn test_cli_task_state_operations() -> Result<()> {
    let temp_dir = tempdir()?;
    env::set_current_dir(temp_dir.path())?;
    let db = Database::new()?;

    // Test adding a task state
    let task = "copyfiles";
    let state = "1";

    let task_obj = SupportedTask::Default(task.to_string());
    let state_obj = TaskValidState::Default(state.to_string());

    db.add_valid_state(&task_obj, &state_obj)?;

    // Verify state was added
    let states = db.list_valid_states(&task_obj)?;
    assert_eq!(states.len(), 1);
    assert_eq!(states[0], state_obj);

    // Test deleting a task state
    db.delete_valid_state(&task_obj, &state_obj)?;

    // Verify state was deleted
    let states = db.list_valid_states(&task_obj)?;
    assert_eq!(states.len(), 0);

    Ok(())
}
