//! Session ID validation for path-safe persistence under workspace/sessions/{session_id}/.

/// Max session ID length.
const MAX_LEN: usize = 128;

/// Returns the session ID if it is safe for use in file paths.
/// Rejects empty, too long, or values containing path traversal characters.
pub fn sanitize(s: &str) -> Option<&str> {
    let s = s.trim();
    if s.is_empty() || s.len() > MAX_LEN {
        return None;
    }
    if s.contains("..") || s.contains('/') || s.contains('\\') {
        return None;
    }
    if !s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return None;
    }
    Some(s)
}
