# Parler

Parler is a desktop dictation application derived from [Handy](https://github.com/cjpais/Handy). This repository contains a customized macOS build with local bilingual recognition, optional Mistral correction using a Markdown brain, and a local learning corpus.

Desktop version: **1.0.0**. See [CHANGELOG.md](CHANGELOG.md) for release notes. The `web-app/` project has its own version and is not part of this desktop release.

## Daily use

These are the shortcuts configured for this installation; shortcuts can be changed in the application.

| Shortcut | Behavior |
| --- | --- |
| Option + Space | Start recording; press again to stop, transcribe locally and paste. |
| Option + Shift + Space | Start recording; press again to stop, transcribe locally, correct with Mistral and paste. |
| Escape | Cancel the current operation. |

Push-to-talk is disabled in this installation. Recording appears as a green waveform. Processing starts after recording stops: this is **not live streaming transcription or streaming Mistral correction**.

The current recognition model is **Whisper Turbo**, with **Auto Detect** language and translation to English disabled. French, English and mixed dictation are supported, but recognition and correction can still make mistakes. Download the recognition model before using dictation offline. Other transcription models remain available for comparisons.

## Offline behavior and fallback

Local recognition works without Internet once the model is downloaded. Option + Space never requests cloud correction.

Option + Shift + Space also keeps working when Mistral is unavailable: if correction fails, the existing local transcription is retained and sent through the normal paste path. A failed correction is not saved as a successful model result. There is no background retry or later replacement of pasted text.

- Cloud connections have a **2-second connection timeout**.
- Each cloud HTTP request has a **5-second total timeout**, including reading the response body.
- The action, including local brain-context retrieval, has an **8-second asynchronous deadline**. This starts after speech recognition; recognition and pasting have their own processing time.
- Connection errors, HTTP errors, invalid or empty responses and timeouts trigger fallback. The settings window receives a fallback notice; it may not be visible while that window is hidden.

Fallback preserves the recognized language. Channel-specific translation, tone and vocabulary correction require a successful Mistral response. The lexicon still supplies local Whisper vocabulary hints. You can retry correction from history when the connection returns.

## Mistral, brain and communication profiles

In **Models**, add or edit a saved Mistral language model and its API key. Editing preserves the model ID and associated actions. This installation uses `mistral-small-latest`; a larger correction model has not consistently improved our small bilingual test set.

Post-processing actions select a saved model, Markdown brain folder and optional communication channel. This installation uses `/Users/mat-vert/Workspace/vert-mat/vert-mat-brain`.

Brain references are discovered by frontmatter rather than a fixed filename, so they can be renamed:

```yaml
---
parler: lexicon
---
```

The current lexicon is `Lexical dictation.md`. Its three-column Markdown table contains preferred terms, recognition variants and context. Canonical terms supply Whisper hints; correction uses variants and relevant brain excerpts. Files are reread for subsequent dictations; edits do not train Whisper.

Use `parler: communication` for the channel-profile document, currently `Communication.md`. Profiles explicitly selected by actions define language, tone and format:

| Profile | Language |
| --- | --- |
| Default dictation | Preserve the spoken language |
| Team Slack | English |
| Frenchies messages | French |
| E-mail | French |
| Professional documents | English |
| ChatGPT | Preserve the spoken language |

The app does not automatically infer the destination channel from the foreground application. Keep one document per marker; duplicate marked references are rejected rather than arbitrarily selected.

## Learning corpus

In **Advanced → Learning corpus**, enable collection when you want to review examples. It is **off by default**. The configured local folder is `/Users/mat-vert/Workspace/mlefree/Parler/learning`.

Each sample can retain the raw recognition, cleaned transcript, model output when available, exact correction request without credentials, reference snapshots, metadata and an independent audio copy. The audio is the app's recorded/VAD-filtered signal, not an untouched continuous microphone stream. The corpus is excluded from Git and brain retrieval. Turning collection off preserves existing examples. Normal history pruning does not delete their independent audio copies.

Review samples and save the intended final text as a correction. Collection does **not** automatically update the lexicon, treat Mistral output as ground truth, fine-tune a model or upload the corpus. A later review/evaluation workflow can use these corrected examples to propose vocabulary and prompt improvements. See the local `learning/README.md` for the file format when present.

## Data and privacy

Recognition and audio storage are local. Optional Mistral correction sends the **recognized text, prompts and selected brain excerpts** to Mistral's API; it is not an entirely offline operation. Audio is not sent by this correction path. The learning corpus contains potentially private transcripts and brain snapshots: manage it accordingly.

Application settings, provider credentials, downloaded models and history remain in the existing Tauri application-data directory (`~/Library/Application Support/com.melvynx.parler` on this Mac). Do not commit API keys or learning data.

## Build and install locally

Install Rust stable and Bun, then:

```bash
bun install
mkdir -p src-tauri/resources/models
curl -fL -o src-tauri/resources/models/silero_vad_v4.onnx https://blob.handy.computer/silero_vad_v4.onnx
bun run deploy:local
```

`deploy:local` builds, signs, installs and launches **only `/Applications/Parler.app`**, using a persistent local signing identity to retain macOS permissions across updates. Do not routinely launch build copies or reset permissions. See [BUILD.md](BUILD.md) for signing setup and platform prerequisites.

The interface focuses on dictation, microphone, models, post-processing, history and learning. Audio-feedback playback and Rodio were removed, and sound themes are excluded from the bundle. Advanced paste/experimental settings and secondary shortcut editors are no longer displayed; legacy settings remain readable. Other transcription engines and downloaded models were retained for comparison. Cross-platform source remains inherited, but this customized release was validated on this Mac.

## Versioning and development

Use `MAJOR.MINOR.PATCH`: patches for fixes, minor versions for compatible features, major versions for incompatible changes. Keep `package.json`, `src-tauri/Cargo.toml`, `src-tauri/Cargo.lock` and `src-tauri/tauri.conf.json` synchronized. Record behavior changes, fallback limits and validation in [CHANGELOG.md](CHANGELOG.md). A version bump and local deployment do not publish a release.

The backend is Rust/Tauri and the interface is React/TypeScript. See [AGENTS.md](AGENTS.md) for repository guidance. The original project documentation is archived in [docs/UPSTREAM_README.md](docs/UPSTREAM_README.md); installation commands there refer to Handy, not this build.

## License and acknowledgments

MIT — see [LICENSE](LICENSE). Based on Handy and its contributors, Whisper/whisper.cpp, Silero VAD and Tauri.
