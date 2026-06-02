use std::{
    fs,
    io::{self, IsTerminal, Read},
    path::PathBuf,
};

use anyhow::{Context, Result, anyhow};

#[allow(dead_code)]
pub fn prompt_from_args(parts: &[String], force_stdin: bool) -> Result<String> {
    if !parts.is_empty() {
        return Ok(parts.join(" "));
    }

    if force_stdin || !io::stdin().is_terminal() {
        let mut buffer = String::new();
        io::stdin().read_to_string(&mut buffer)?;
        let prompt = buffer.trim().to_string();
        if prompt.is_empty() {
            return Err(anyhow!("stdin did not contain a prompt"));
        }
        return Ok(prompt);
    }

    Err(anyhow!("missing prompt; pass text or pipe stdin"))
}

pub fn cache_prefix_from_args(
    inline: &Option<String>,
    files: &[PathBuf],
) -> Result<Option<String>> {
    let mut blocks = Vec::new();

    if let Some(text) = inline.as_ref().filter(|value| !value.trim().is_empty()) {
        blocks.push(format!(
            "<<<BAMBOO_CACHE_PREFIX_INLINE>>>\n{}",
            normalize_newlines(text)
        ));
    }

    let mut paths = files.to_vec();
    paths.sort_by(|left, right| {
        left.to_string_lossy()
            .as_ref()
            .cmp(right.to_string_lossy().as_ref())
    });

    for path in paths {
        let raw = fs::read_to_string(&path)
            .with_context(|| format!("failed to read cache prefix file {}", path.display()))?;
        if raw.trim().is_empty() {
            continue;
        }

        let content = normalize_newlines(&raw);
        let trailing_newline = if content.ends_with('\n') { "" } else { "\n" };
        blocks.push(format!(
            "<<<BAMBOO_CACHE_PREFIX_FILE path=\"{}\">>>\n{}{}<<<BAMBOO_CACHE_PREFIX_FILE_END>>>",
            path.display(),
            content,
            trailing_newline
        ));
    }

    if blocks.is_empty() {
        return Ok(None);
    }

    Ok(Some(format!(
        "<<<BAMBOO_CACHE_PREFIX_V1>>>\n{}\n<<<BAMBOO_CACHE_PREFIX_END>>>",
        blocks.join("\n\n")
    )))
}

#[allow(dead_code)]
pub fn apply_cache_prefix(cache_prefix: Option<&str>, prompt: String) -> String {
    match cache_prefix {
        Some(prefix) if !prefix.is_empty() => {
            format!("{prefix}\n\n<<<BAMBOO_TASK>>>\n{prompt}")
        }
        _ => prompt,
    }
}

pub fn cache_key_from_prefix(prefix: &str) -> String {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in prefix.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("bamboo-{hash:016x}")
}

fn normalize_newlines(value: &str) -> String {
    value.replace("\r\n", "\n").replace('\r', "\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn applies_cache_prefix_before_task() {
        let prompt = apply_cache_prefix(Some("stable"), "do it".to_string());
        assert_eq!(prompt, "stable\n\n<<<BAMBOO_TASK>>>\ndo it");
    }

    #[test]
    fn leaves_prompt_unchanged_without_prefix() {
        assert_eq!(apply_cache_prefix(None, "do it".to_string()), "do it");
    }

    #[test]
    fn cache_key_from_prefix_is_stable() {
        assert_eq!(
            cache_key_from_prefix("stable"),
            cache_key_from_prefix("stable")
        );
        assert_ne!(
            cache_key_from_prefix("stable"),
            cache_key_from_prefix("other")
        );
    }
}
