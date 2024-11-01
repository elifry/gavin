use gavin::{TaskValidState, GitVersionState};

#[test]
fn test_task_valid_state_serialization() {
    // Test Default variant
    let state = TaskValidState::Default("2".to_string());
    let json = serde_json::to_string(&state).unwrap();
    println!("Serialized Default: {}", json);
    let deserialized: TaskValidState = serde_json::from_str(&json).unwrap();
    assert_eq!(state, deserialized);

    // Test Gitversion variant
    let gv_state = GitVersionState::new("3", "3", "6.0.3");
    let state = TaskValidState::Gitversion(gv_state);
    let json = serde_json::to_string(&state).unwrap();
    println!("Serialized Gitversion: {}", json);
    let deserialized: TaskValidState = serde_json::from_str(&json).unwrap();
    assert_eq!(state, deserialized);
}
