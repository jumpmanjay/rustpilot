/// Resolve references in a prompt string.
///
/// - `@@path/to/file:10-20` → inline the content of lines 10-20
/// - `@path/to/file:10-20`  → keep as-is (tag reference for LLM to read)
///
/// Include references (@@) are expanded at send time.
/// Tag references (@) are left in the prompt for the LLM to interpret.
pub fn resolve_references(prompt: &str) -> String {
    let mut result = String::with_capacity(prompt.len());

    for line in prompt.lines() {
        let trimmed = line.trim();

        if trimmed.starts_with("@@") {
            // Include reference — expand inline
            let ref_str = &trimmed[2..];
            if let Some(content) = read_reference(ref_str) {
                result.push_str(&format!(
                    "--- {} ---\n{}\n--- end ---\n",
                    ref_str, content
                ));
            } else {
                // Could not read, keep as-is
                result.push_str(line);
                result.push('\n');
            }
        } else {
            result.push_str(line);
            result.push('\n');
        }
    }

    result
}

/// Parse a reference like `path/to/file.rs:10-20` and read those lines.
fn read_reference(reference: &str) -> Option<String> {
    let (path, range) = if let Some(colon) = reference.rfind(':') {
        let path = &reference[..colon];
        let range_str = &reference[colon + 1..];
        (path, Some(range_str))
    } else {
        (reference, None)
    };

    let content = std::fs::read_to_string(path).ok()?;

    if let Some(range_str) = range {
        let lines: Vec<&str> = content.lines().collect();
        if let Some(dash) = range_str.find('-') {
            let start: usize = range_str[..dash].parse().ok()?;
            let end: usize = range_str[dash + 1..].parse().ok()?;
            let start = start.saturating_sub(1); // 1-indexed to 0-indexed
            let end = end.min(lines.len());
            Some(
                lines[start..end]
                    .iter()
                    .enumerate()
                    .map(|(i, l)| format!("{:4} | {}", start + i + 1, l))
                    .collect::<Vec<_>>()
                    .join("\n"),
            )
        } else {
            let line_num: usize = range_str.parse().ok()?;
            let idx = line_num.saturating_sub(1);
            lines.get(idx).map(|l| format!("{:4} | {}", line_num, l))
        }
    } else {
        // Whole file
        Some(content)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tag_reference_unchanged() {
        let input = "Look at @src/main.rs:42 for context";
        let result = resolve_references(input);
        assert!(result.contains("@src/main.rs:42"));
    }
}
