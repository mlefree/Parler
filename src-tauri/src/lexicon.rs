use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

pub const FILE_NAME: &str = "Lexical dictation.md";
const MAX_BYTES: u64 = 32 * 1024;
const MAX_ENTRIES: usize = 64;
const MAX_PROMPT_CHARS: usize = 1_200;

#[derive(Debug, Default)]
pub struct Lexicon {
    pub entries: Vec<Entry>,
}

#[derive(Debug)]
pub struct Entry {
    pub preferred: String,
    pub variants: Vec<String>,
    pub context: String,
}

fn normalize(value: &str) -> String {
    value
        .chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn cells(line: &str) -> Vec<&str> {
    line.trim()
        .trim_matches('|')
        .split('|')
        .map(|c| c.trim().trim_matches('`'))
        .collect()
}

pub fn parse(content: &str) -> Result<Lexicon, String> {
    let mut lexicon = Lexicon::default();
    let mut in_table = false;
    let mut found_header = false;
    let mut in_code = false;
    let mut seen = BTreeSet::new();
    for line in content.lines() {
        if line.trim_start().starts_with("```") || line.trim_start().starts_with("~~~") {
            in_code = !in_code;
            continue;
        }
        if in_code {
            continue;
        }
        if !line.trim_start().starts_with('|') {
            in_table = false;
            continue;
        }
        let row = cells(line);
        if row.len() != 3 {
            continue;
        }
        let first = row[0].to_lowercase();
        if matches!(first.as_str(), "preferred term" | "terme préféré") {
            found_header = true;
            in_table = true;
            continue;
        }
        if !in_table
            || row
                .iter()
                .all(|c| c.chars().all(|v| v == '-' || v == ':' || v.is_whitespace()))
        {
            continue;
        }
        let preferred = row[0].trim();
        if preferred.is_empty()
            || preferred.chars().count() > 80
            || !seen.insert(normalize(preferred))
        {
            continue;
        }
        if lexicon.entries.len() >= MAX_ENTRIES {
            break;
        }
        lexicon.entries.push(Entry {
            preferred: preferred.to_owned(),
            variants: row[1]
                .split(';')
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .take(12)
                .map(|v| v.chars().take(80).collect())
                .collect(),
            context: row[2].chars().take(600).collect(),
        });
    }
    if !found_header {
        return Err(
            "Dictation lexicon needs a Preferred term / Heard variants / Context table".into(),
        );
    }
    Ok(lexicon)
}

pub fn load(root: &Path) -> Result<Lexicon, String> {
    let Some(path) =
        crate::brain_reference::find(root, "lexicon", &[FILE_NAME, "Lexique dictee.md"])?
    else {
        return Ok(Lexicon::default());
    };
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Lexicon::default()),
        Err(_) => return Err("Cannot read dictation lexicon metadata".into()),
    };
    if !metadata.file_type().is_file() || metadata.len() > MAX_BYTES {
        return Err("Dictation lexicon must be a regular Markdown file of at most 32 KiB".into());
    }
    let content = fs::read_to_string(path)
        .map_err(|_| "Cannot read dictation lexicon as UTF-8".to_string())?;
    parse(&content)
}

impl Lexicon {
    pub fn canonical_spelling<'a>(&'a self, candidate: &'a str) -> &'a str {
        let key = normalize(candidate);
        self.entries
            .iter()
            .find(|e| {
                normalize(&e.preferred) == key || e.variants.iter().any(|v| normalize(v) == key)
            })
            .map(|e| e.preferred.as_str())
            .unwrap_or(candidate)
    }

    pub fn recognition_prompt(&self, manual_words: &[String]) -> Option<String> {
        let mut words = Vec::new();
        let mut seen = BTreeSet::new();
        let mut size = 0;
        for term in self
            .entries
            .iter()
            .map(|e| e.preferred.as_str())
            .chain(manual_words.iter().map(String::as_str))
        {
            let term = term.trim();
            if term.is_empty() || !seen.insert(normalize(term)) {
                continue;
            }
            let addition = term.chars().count() + usize::from(!words.is_empty()) * 2;
            if size + addition > MAX_PROMPT_CHARS {
                continue;
            }
            size += addition;
            words.push(term);
        }
        if words.is_empty() {
            None
        } else {
            Some(words.join(", "))
        }
    }

    pub fn correction_context(&self) -> String {
        if self.entries.is_empty() {
            return String::new();
        }
        let mut output = format!("Explicit dictation lexicon ({FILE_NAME}). Preferred spellings take priority over inferred spellings. Variants are conditional candidates, never unconditional replacements. Follow their context limits; preserve ordinary words, negations, dates and spoken languages.\n");
        for entry in &self.entries {
            let row = format!(
                "Preferred: {}\nHeard variants: {}\nContext: {}\n\n",
                entry.preferred,
                entry.variants.join("; "),
                entry.context
            );
            if output.chars().count() + row.chars().count() > 6_000 {
                break;
            }
            output.push_str(&row);
        }
        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const TABLE: &str = "| Preferred term | Heard variants | Context |\n| --- | --- | --- |\n| MEG | Mec; Mac | Project only; keep ordinary mec and Apple Mac. |\n| DataForge | DataForce; Data Forge | Project spelling. |";

    #[test]
    fn recognition_receives_only_preferred_spellings_and_preserves_manual_words() {
        let lexicon = parse(TABLE).unwrap();
        let prompt = lexicon
            .recognition_prompt(&["meg".into(), "Other".into()])
            .unwrap();
        assert_eq!(prompt, "MEG, DataForge, Other");
        assert!(!prompt.contains("Mec"));
        assert!(!prompt.contains("DataForce"));
        assert!(lexicon
            .correction_context()
            .contains("keep ordinary mec and Apple Mac"));
        assert_eq!(lexicon.canonical_spelling("Data Forge"), "DataForge");
    }

    #[test]
    fn ignores_code_examples_and_bounds_the_recognition_prompt() {
        let content = format!("```markdown\n{TABLE}\n```\n{TABLE}");
        let lexicon = parse(&content).unwrap();
        assert_eq!(lexicon.entries.len(), 2);
        let manual = vec!["x".repeat(2_000), "Fits".into()];
        assert_eq!(
            lexicon.recognition_prompt(&manual).as_deref(),
            Some("MEG, DataForge, Fits")
        );
        assert!(parse("# Missing table").is_err());
    }

    #[test]
    fn reloads_edits_without_restart_and_does_not_follow_symlinks() {
        let root = std::env::temp_dir().join(format!("parler-lexicon-test-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        assert!(load(&root).unwrap().entries.is_empty());
        fs::write(root.join(FILE_NAME), TABLE).unwrap();
        assert_eq!(load(&root).unwrap().entries.len(), 2);
        fs::write(root.join(FILE_NAME), TABLE.replace("DataForge", "NewTerm")).unwrap();
        assert_eq!(load(&root).unwrap().entries[1].preferred, "NewTerm");
        fs::remove_file(root.join(FILE_NAME)).unwrap();
        #[cfg(unix)]
        {
            fs::write(root.join("target.md"), TABLE).unwrap();
            std::os::unix::fs::symlink(root.join("target.md"), root.join(FILE_NAME)).unwrap();
            assert!(load(&root).is_err());
        }
        fs::remove_dir_all(root).unwrap();
    }
}
