//! Read-only, bounded Markdown retrieval. Notes are data, never instructions.
use std::collections::BTreeSet;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

const MAX_FILES: usize = 2_000;
const MAX_FILE_BYTES: u64 = 256 * 1024;
const MAX_TOTAL_BYTES: usize = 32 * 1024 * 1024;
const MAX_CONTEXT_CHARS: usize = 20_000;

pub fn validate_path(path: Option<String>) -> Result<Option<String>, String> {
    let Some(path) = path.filter(|p| !p.trim().is_empty()) else {
        return Ok(None);
    };
    let root = Path::new(path.trim());
    if !root.is_absolute() || !root.is_dir() {
        return Err("Brain folder must be an existing absolute directory path".into());
    }
    Ok(Some(root.to_string_lossy().into_owned()))
}

fn tokens(text: &str) -> BTreeSet<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.chars().count() >= 3)
        .filter(|w| {
            !matches!(
                *w,
                "the"
                    | "and"
                    | "for"
                    | "that"
                    | "this"
                    | "with"
                    | "have"
                    | "you"
                    | "les"
                    | "des"
                    | "une"
                    | "dans"
                    | "pour"
                    | "avec"
                    | "que"
                    | "qui"
                    | "sur"
                    | "est"
                    | "pas"
            )
        })
        .map(str::to_owned)
        .collect()
}

fn score(text: &str, query: &BTreeSet<String>, fuzzy: bool) -> usize {
    let words = tokens(text);
    query
        .iter()
        .map(|q| {
            if words.contains(q) {
                4
            } else if fuzzy
                && q.chars().count() >= 5
                && words
                    .iter()
                    .any(|w| w.chars().count() >= 5 && strsim::normalized_levenshtein(q, w) >= 0.75)
            {
                2
            } else {
                0
            }
        })
        .sum()
}

fn entity_candidates(title: &str, query: &BTreeSet<String>) -> Vec<(String, String)> {
    let words: Vec<_> = title
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect();
    let mut candidates = Vec::new();
    for q in query {
        for word in &words {
            // Short acronyms such as MEG need separate handling from long words.
            let acronym =
                (3..=5).contains(&word.chars().count()) && word.chars().all(|c| c.is_uppercase());
            if acronym
                && q != &word.to_lowercase()
                && q.chars().take(2).eq(word.to_lowercase().chars().take(2))
                && strsim::levenshtein(q, &word.to_lowercase()) == 1
            {
                candidates.push((q.clone(), (*word).to_owned()));
            }
        }
        // Recognizers often join or split compound project names (Data Forge).
        for pair in words.windows(2).take(1) {
            let joined = format!("{}{}", pair[0], pair[1]);
            let canonical = joined.to_lowercase();
            if q.chars().count() >= 6
                && pair
                    .iter()
                    .all(|w| w.chars().next().is_some_and(char::is_uppercase))
                && q != &canonical
                && (strsim::levenshtein(q, &canonical) == 1
                    || (q.chars().count() >= 7 && canonical.starts_with(q)))
            {
                candidates.push((q.clone(), format!("{} {}", pair[0], pair[1])));
            }
        }
    }
    candidates
}

fn markdown_files(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut pending = vec![root.to_owned()];
    let mut visited = 0;
    while let Some(dir) = pending.pop() {
        let Ok(entries) = fs::read_dir(dir) else {
            continue;
        };
        let mut entries: Vec<_> = entries.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            visited += 1;
            if visited > 10_000 || files.len() >= MAX_FILES {
                return files;
            }
            if entry.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            // Do not follow links outside the selected vault, including directory links.
            if kind.is_symlink() {
                continue;
            }
            if kind.is_dir() && !crate::brain_reference::is_learning_corpus(&entry.path()) {
                pending.push(entry.path());
            } else if kind.is_file()
                && entry
                    .path()
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("md"))
            {
                files.push(entry.path());
            }
        }
    }
    files.sort();
    files
}

pub fn retrieve(root: &str, transcription: &str) -> Result<String, String> {
    let root = Path::new(root);
    if crate::brain_reference::is_learning_corpus(root) {
        return Err("A learning corpus cannot be used as a brain".into());
    }
    if !root.is_dir() {
        return Err("Brain folder is no longer available".into());
    }
    let query = tokens(transcription);
    let lexicon = crate::lexicon::load(root)?;
    let files = markdown_files(root);
    let mut vocabulary = Vec::new();
    let mut excerpts = Vec::new();
    let mut candidates = BTreeSet::new();
    let mut total = 0;
    for path in files {
        if path == root.join(crate::lexicon::FILE_NAME) || path == root.join("Communication.md") {
            continue;
        }
        let relative = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .into_owned();
        let title = path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let possible_entities = entity_candidates(&title, &query);
        let title_score = score(&title, &query, true) + possible_entities.len() * 6;
        candidates.extend(possible_entities);
        let curated = matches!(
            relative.split('/').next(),
            Some("People" | "Projects" | "Vertesia" | "Clients")
        );
        if curated || title_score > 0 {
            vocabulary.push((title_score, title.clone()));
        }
        let mut bytes = Vec::new();
        let Ok(file) = fs::File::open(&path) else {
            continue;
        };
        if file.take(MAX_FILE_BYTES).read_to_end(&mut bytes).is_err() {
            continue;
        }
        total += bytes.len();
        if total > MAX_TOTAL_BYTES {
            break;
        }
        let Ok(content) = String::from_utf8(bytes) else {
            continue;
        };
        if matches!(
            crate::brain_reference::marker(&content),
            Some("lexicon" | "communication")
        ) {
            continue;
        }
        let content = content
            .strip_prefix("---\n")
            .and_then(|c| c.split_once("\n---").map(|(_, body)| body))
            .unwrap_or(&content);
        let baseline = usize::from(relative == "Now.md") * 3;
        // Rank paragraphs instead of sending whole files or the full vault.
        for (index, block) in content.split("\n\n").enumerate() {
            if block.trim().is_empty() {
                continue;
            }
            let rank = title_score * 2 + score(block, &query, false) + baseline;
            if rank > 0 {
                excerpts.push((
                    rank,
                    relative.clone(),
                    index,
                    block.chars().take(2_500).collect::<String>(),
                ));
            }
        }
    }
    vocabulary.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    vocabulary.dedup_by(|a, b| a.1 == b.1);
    let glossary = vocabulary
        .iter()
        .take(180)
        .map(|(_, title)| title.as_str())
        .collect::<Vec<_>>()
        .join("; ");
    let hints = candidates
        .iter()
        .take(20)
        .map(|(heard, entity)| format!("{heard} → {}", lexicon.canonical_spelling(entity)))
        .collect::<Vec<_>>()
        .join("; ");
    let mut output = lexicon.correction_context();
    output.push_str(&format!("Possible speech-recognition confusions (apply ONLY when the sentence's project context supports the entity; keep ordinary words otherwise):\n{}\n\nProject vocabulary (candidate spellings, not words to add):\n{}\n\nRelevant note excerpts:\n", hints.chars().take(2_000).collect::<String>(), glossary.chars().take(6_000).collect::<String>()));
    excerpts.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
    let mut per_file = std::collections::HashMap::new();
    for (_, path, _, block) in excerpts {
        let count = per_file.entry(path.clone()).or_insert(0);
        if *count >= 2 {
            continue;
        }
        let section = format!("\nSource: {path}\n{block}\n");
        if output.chars().count() + section.chars().count() > MAX_CONTEXT_CHARS {
            continue;
        }
        output.push_str(&section);
        *count += 1;
        if per_file.values().sum::<usize>() >= 8 {
            break;
        }
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn learning_samples_and_reference_snapshots_are_excluded() {
        let root =
            std::env::temp_dir().join(format!("parler-learning-exclusion-{}", std::process::id()));
        fs::create_dir_all(root.join("learning/sample/references")).unwrap();
        fs::write(root.join("learning").join(crate::learning::MARKER), "v1").unwrap();
        fs::write(
            root.join("learning/sample/transcript.md"),
            "PRIVATE_CORPUS_TRANSCRIPT about Vertesia",
        )
        .unwrap();
        fs::write(
            root.join("learning/sample/references/lexicon.md"),
            "---\nparler: lexicon\n---\nPRIVATE_LEXICON_SNAPSHOT",
        )
        .unwrap();
        assert!(crate::brain_reference::find(&root, "lexicon", &[])
            .unwrap()
            .is_none());
        let result =
            retrieve(root.to_str().unwrap(), "Vertesia PRIVATE_CORPUS_TRANSCRIPT").unwrap();
        assert!(!result.contains("PRIVATE_CORPUS_TRANSCRIPT"));
        assert!(!result.contains("PRIVATE_LEXICON_SNAPSHOT"));
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn matches_misheard_project_names_but_ignores_filler() {
        assert!(score("Vertesia", &tokens("le projet vertesia"), true) > 0);
        assert!(score("Vertesia", &tokens("le projet vertessia"), true) > 0);
        assert_eq!(
            score("Something else", &tokens("pour les une the"), true),
            0
        );
    }

    #[test]
    fn finds_short_acronyms_and_compound_names_without_forcing_replacements() {
        assert!(
            entity_candidates("MEG App", &tokens("entre Mec et DataForce"))
                .contains(&("mec".into(), "MEG".into()))
        );
        assert!(
            entity_candidates("Data Forge Accelerator", &tokens("DataForce"))
                .contains(&("dataforce".into(), "Data Forge".into()))
        );
        assert!(entity_candidates("MEG App", &tokens("on va boire un café")).is_empty());
        assert!(entity_candidates("MODE", &tokens("made only")).is_empty());
        assert!(entity_candidates("MOC", &tokens("mec")).is_empty());
        assert!(entity_candidates(
            "Vertesia Interaction Best Practices",
            &tokens("interactions")
        )
        .is_empty());
        assert!(entity_candidates("Deploy a project", &tokens("deploy")).is_empty());
        assert!(entity_candidates("Project vs Environment", &tokens("environment")).is_empty());
    }
    #[test]
    fn retrieval_is_bounded_and_excludes_hidden_files_and_symlinks() {
        let root = std::env::temp_dir().join(format!("parler-brain-test-{}", std::process::id()));
        fs::create_dir_all(root.join("Projects")).unwrap();
        fs::create_dir_all(root.join(".trash")).unwrap();
        fs::write(
            root.join("Projects/Vertesia.md"),
            "# Vertesia\n\nCanonical project spelling.\n\n".to_owned()
                + &"More Vertesia details. ".repeat(4_000),
        )
        .unwrap();
        fs::write(root.join(".trash/Vertesia secret.md"), "DO_NOT_SEND_HIDDEN").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(
            root.join(".trash/Vertesia secret.md"),
            root.join("Vertesia linked.md"),
        )
        .unwrap();
        let result = retrieve(root.to_str().unwrap(), "vertessia").unwrap();
        assert!(result.contains("Canonical project spelling"));
        assert!(result.chars().count() <= MAX_CONTEXT_CHARS);
        assert!(!result.contains("DO_NOT_SEND_HIDDEN"));
        assert!(!result.contains("Vertesia linked"));
        fs::remove_dir_all(root).unwrap();
    }
}
