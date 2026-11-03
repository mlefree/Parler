use std::fs;
use std::path::{Path, PathBuf};

pub fn is_learning_corpus(path: &Path) -> bool {
    path.join(crate::learning::MARKER).is_file()
}

pub fn marker(content: &str) -> Option<&str> {
    let frontmatter = content.strip_prefix("---\n")?.split_once("\n---")?.0;
    frontmatter.lines().find_map(|line| {
        line.strip_prefix("parler:")
            .map(|v| v.trim().trim_matches('"').trim_matches('\''))
    })
}

pub fn find(root: &Path, kind: &str, legacy_names: &[&str]) -> Result<Option<PathBuf>, String> {
    if is_learning_corpus(root) {
        return Err("A learning corpus cannot be used as a brain".into());
    }
    let mut pending = vec![(root.to_owned(), 0)];
    let mut scanned = 0;
    let mut found = None;
    while let Some((dir, depth)) = pending.pop() {
        let Ok(entries) = fs::read_dir(dir) else {
            continue;
        };
        let mut entries: Vec<_> = entries.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            if entry.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            let Ok(kind_meta) = entry.file_type() else {
                continue;
            };
            if kind_meta.is_symlink() {
                continue;
            }
            if kind_meta.is_dir() && depth < 4 && !is_learning_corpus(&entry.path()) {
                pending.push((entry.path(), depth + 1));
            } else if kind_meta.is_file()
                && entry
                    .path()
                    .extension()
                    .is_some_and(|x| x.eq_ignore_ascii_case("md"))
            {
                scanned += 1;
                if scanned > 2_000 {
                    return Err("Too many files while locating brain preferences".into());
                }
                // Only frontmatter is needed to identify the reference file.
                use std::io::Read;
                let mut bytes = Vec::new();
                let Ok(file) = fs::File::open(entry.path()) else {
                    continue;
                };
                if file.take(2_048).read_to_end(&mut bytes).is_err() {
                    continue;
                }
                let content = String::from_utf8_lossy(&bytes);
                if marker(&content) == Some(kind) {
                    if found.is_some() {
                        return Err(format!("Multiple brain files are marked parler: {}", kind));
                    }
                    found = Some(entry.path());
                }
            }
        }
    }
    if found.is_some() {
        return Ok(found);
    }
    Ok(legacy_names
        .iter()
        .map(|name| root.join(name))
        .find(|p| p.exists()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn discovers_renamed_files_by_frontmatter_and_rejects_ambiguity() {
        let root =
            std::env::temp_dir().join(format!("parler-reference-test-{}", std::process::id()));
        fs::create_dir_all(root.join("References")).unwrap();
        let path = root.join("References/New name.md");
        let content = "---\nparler: lexicon\n---\n# Vocabulary";
        fs::write(&path, content).unwrap();
        assert_eq!(find(&root, "lexicon", &["Old.md"]).unwrap(), Some(path));
        fs::write(root.join("Duplicate.md"), content).unwrap();
        assert!(find(&root, "lexicon", &[]).is_err());
        fs::remove_dir_all(root).unwrap();
    }
}
