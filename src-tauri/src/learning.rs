use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use specta::Type;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;

pub const MARKER: &str = ".parler-learning-corpus";
static SEQUENCE: AtomicU64 = AtomicU64::new(0);
static BUILD_HASH: OnceLock<Option<String>> = OnceLock::new();

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(default)]
pub struct LearningSettings {
    pub enabled: bool,
    pub save_audio: bool,
    pub directory: Option<String>,
}

impl Default for LearningSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            save_audio: true,
            directory: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct LearningSample {
    pub id: String,
    pub created_at: String,
    pub raw_transcript: String,
    pub transcript: String,
    pub model_output: Option<String>,
    pub output_text: String,
    pub user_correction: String,
    pub audio_saved: bool,
}

pub struct Capture {
    pub raw_transcript: String,
    pub transcript: String,
    pub model_output: Option<String>,
    pub output_text: String,
    pub metadata: Value,
    pub brain_root: Option<String>,
}

fn write_private(path: &Path, content: &[u8]) -> Result<(), String> {
    fs::write(path, content).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn digest(path: &Path) -> Result<String, String> {
    let mut file = fs::File::open(path).map_err(|e| e.to_string())?;
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 32 * 1024];
    loop {
        let len = file.read(&mut buffer).map_err(|e| e.to_string())?;
        if len == 0 {
            break;
        }
        hash.update(&buffer[..len]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

pub fn prepare(directory: &str) -> Result<PathBuf, String> {
    let root = PathBuf::from(directory.trim());
    if !root.is_absolute() {
        return Err("Learning folder must be an absolute path".into());
    }
    fs::create_dir_all(&root).map_err(|e| e.to_string())?;
    let marker = root.join(MARKER);
    if !marker.exists() {
        write_private(&marker, b"Parler learning corpus v1\n")?;
    }
    Ok(root)
}

pub fn capture(
    settings: &LearningSettings,
    audio: Option<&Path>,
    input: Capture,
) -> Result<Option<PathBuf>, String> {
    if !settings.enabled {
        return Ok(None);
    }
    let root = prepare(
        settings
            .directory
            .as_deref()
            .ok_or("Choose a learning folder first")?,
    )?;
    let created_at = Utc::now().to_rfc3339();
    let id = format!(
        "{}-{}-{}",
        Utc::now().format("%Y-%m-%d_%H-%M-%S%.3f"),
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    );
    let staging = root.join(format!(".pending-{id}"));
    fs::create_dir(&staging).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&staging, fs::Permissions::from_mode(0o700))
            .map_err(|e| e.to_string())?;
    }
    let result = (|| {
        let mut metadata = input.metadata;
        if !metadata.is_object() {
            return Err("Learning metadata must be an object".into());
        }
        let mut audio_saved = false;
        let mut audio_error = None;
        if settings.save_audio {
            if let Some(source) = audio {
                match fs::copy(source, staging.join("audio.wav")) {
                    Ok(_) => audio_saved = true,
                    Err(error) => audio_error = Some(error.to_string()),
                }
            } else {
                audio_error = Some("No audio file was available".into());
            }
        }
        let sample = LearningSample {
            id: id.clone(),
            created_at: created_at.clone(),
            raw_transcript: input.raw_transcript,
            transcript: input.transcript,
            model_output: input.model_output,
            output_text: input.output_text,
            user_correction: String::new(),
            audio_saved,
        };
        metadata["schema_version"] = json!(1);
        metadata["app_version"] = json!(env!("CARGO_PKG_VERSION"));
        metadata["app_binary_sha256"] =
            json!(BUILD_HASH
                .get_or_init(|| std::env::current_exe().ok().and_then(|p| digest(&p).ok())));
        metadata["sample"] = serde_json::to_value(&sample).map_err(|e| e.to_string())?;
        metadata["correction_file"] = json!("correction.md");
        metadata["correction_source_of_truth"] = json!("correction.md");
        metadata["review_status"] = json!("unreviewed");
        metadata["audio"] = json!({"saved":audio_saved,"error":audio_error,
            "sha256":audio.and_then(|p| digest(p).ok()),"format":"WAV PCM16 mono 16000 Hz"});
        let mut snapshots = Vec::new();
        if let Some(brain) = input.brain_root {
            let refs = staging.join("references");
            for (kind, names) in [
                ("lexicon", vec!["Lexical dictation.md", "Lexique dictee.md"]),
                ("communication", vec!["Communication.md"]),
            ] {
                match crate::brain_reference::find(Path::new(&brain), kind, &names) {
                    Ok(Some(source)) => {
                        fs::create_dir_all(&refs).map_err(|e| e.to_string())?;
                        let target = refs.join(format!("{kind}.md"));
                        if fs::metadata(&source).map_err(|e| e.to_string())?.len() <= 32 * 1024 {
                            fs::copy(&source, &target).map_err(|e| e.to_string())?;
                            snapshots.push(json!({"kind":kind,"source":source,"file":format!("references/{kind}.md"),"sha256":digest(&target)?,"captured_at":created_at}));
                        }
                    }
                    Ok(None) => {}
                    Err(error) => snapshots.push(json!({"kind":kind,"error":error})),
                }
            }
        }
        metadata["reference_snapshots"] = json!(snapshots);
        metadata["snapshot_timing"] = json!("capture completion; exact ASR vocabulary and correction request are stored separately when available");
        let text = format!("# Dictation sample\n\nRecorded: {}\n\n## Raw recognition\n\n{}\n\n## Transcript after cleanup\n\n{}\n\n## Model output\n\n{}\n\n## Prepared output\n\n{}\n\n## Your correction\n\nEdit [correction.md](correction.md), or use Parler's learning panel. A model output is not a validated correction.\n", sample.created_at, sample.raw_transcript, sample.transcript, sample.model_output.as_deref().unwrap_or("(No successful post-processing output)"), sample.output_text);
        write_private(&staging.join("transcript.md"), text.as_bytes())?;
        write_private(&staging.join("correction.md"), b"")?;
        write_private(
            &staging.join("metadata.json"),
            serde_json::to_string_pretty(&metadata)
                .map_err(|e| e.to_string())?
                .as_bytes(),
        )?;
        let destination = root.join(&id);
        fs::rename(&staging, &destination).map_err(|e| e.to_string())?;
        Ok(destination)
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&staging);
    }
    result.map(Some)
}

fn sample_path(root: &Path, id: &str) -> Result<PathBuf, String> {
    if id.is_empty()
        || id.starts_with('.')
        || !id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    {
        return Err("Invalid learning sample ID".into());
    }
    let path = root.join(id);
    if fs::symlink_metadata(&path)
        .map_err(|e| e.to_string())?
        .file_type()
        .is_symlink()
    {
        return Err("Learning sample cannot be a symlink".into());
    }
    Ok(path)
}

pub fn list(directory: &str) -> Result<Vec<LearningSample>, String> {
    let root = Path::new(directory);
    if !root.exists() {
        return Ok(Vec::new());
    }
    let mut ids: Vec<_> = fs::read_dir(root)
        .map_err(|e| e.to_string())?
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir() && !t.is_symlink()))
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|s| !s.starts_with('.'))
        .take(10_000)
        .collect();
    ids.sort_by(|a, b| b.cmp(a));
    let mut samples = Vec::new();
    for id in ids {
        let Ok(path) = sample_path(root, &id) else {
            continue;
        };
        let Ok(bytes) = fs::read(path.join("metadata.json")) else {
            continue;
        };
        let Ok(metadata) = serde_json::from_slice::<Value>(&bytes) else {
            continue;
        };
        let Ok(mut sample) = serde_json::from_value::<LearningSample>(metadata["sample"].clone())
        else {
            continue;
        };
        sample.user_correction = fs::read_to_string(path.join("correction.md")).unwrap_or_default();
        samples.push(sample);
        if samples.len() == 20 {
            break;
        }
    }
    Ok(samples)
}

pub fn save_correction(directory: &str, id: &str, correction: &str) -> Result<(), String> {
    if correction.len() > 128 * 1024 {
        return Err("Correction is too large".into());
    }
    let path = sample_path(Path::new(directory), id)?;
    let mut metadata: Value =
        serde_json::from_slice(&fs::read(path.join("metadata.json")).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    write_private(&path.join("correction.md"), correction.as_bytes())?;
    metadata["review_status"] = json!(if correction.trim().is_empty() {
        "unreviewed"
    } else {
        "corrected"
    });
    metadata["sample"]["user_correction"] = json!(correction);
    metadata["reviewed_at"] = json!(Utc::now().to_rfc3339());
    write_private(
        &path.join("metadata.json"),
        serde_json::to_string_pretty(&metadata)
            .map_err(|e| e.to_string())?
            .as_bytes(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    fn input() -> Capture {
        Capture {
            raw_transcript: "raw".into(),
            transcript: "clean".into(),
            model_output: Some("model".into()),
            output_text: "model".into(),
            metadata: json!({"origin":"test"}),
            brain_root: None,
        }
    }
    #[test]
    fn disabled_mode_writes_nothing_and_enabled_mode_preserves_audio_and_correction() {
        let root =
            std::env::temp_dir().join(format!("parler-learning-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let mut settings = LearningSettings {
            directory: Some(root.to_string_lossy().into()),
            ..Default::default()
        };
        assert!(capture(&settings, None, input()).unwrap().is_none());
        assert!(!root.exists());
        let audio =
            std::env::temp_dir().join(format!("parler-learning-audio-{}.wav", std::process::id()));
        fs::write(&audio, b"test audio").unwrap();
        settings.enabled = true;
        let sample = capture(&settings, Some(&audio), input()).unwrap().unwrap();
        fs::remove_file(&audio).unwrap();
        assert_eq!(fs::read(sample.join("audio.wav")).unwrap(), b"test audio");
        let id = sample.file_name().unwrap().to_str().unwrap();
        save_correction(root.to_str().unwrap(), id, "human correction").unwrap();
        assert_eq!(
            list(root.to_str().unwrap()).unwrap()[0].user_correction,
            "human correction"
        );
        assert!(save_correction(root.to_str().unwrap(), "../escape", "x").is_err());
        settings.save_audio = false;
        let no_audio = capture(&settings, None, input()).unwrap().unwrap();
        assert!(!no_audio.join("audio.wav").exists());
        fs::remove_dir_all(root).unwrap();
    }
}
