use gavin::VersionCompare;

#[test]
fn test_version_comparison() {
    let versions = vec![
        ("1", "1.0.0"),
        ("1.0", "1.0.0"),
        ("1.0.0", "1.0.0"),
        ("2", "2.0.0"),
        ("2.0", "2.0.0"),
    ];

    for (v1, v2) in versions {
        assert!(
            v1.to_string().version_eq(v2),
            "Expected {} to equal {}",
            v1,
            v2
        );
    }
}

#[test]
fn test_invalid_version_comparison() {
    // Test that invalid versions fall back to string comparison
    assert!("invalid".to_string().version_eq("invalid"));
    assert!(!"invalid".to_string().version_eq("other"));
}
