use std::path::PathBuf;

/// Sanitizes a file path to prevent directory traversal and ensure safe file operations
pub fn sanitize_file_path(path: &str) -> PathBuf {
    PathBuf::from(path.replace(['/', '\\'], "_"))
}
