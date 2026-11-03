# Desktop changelog

This changelog covers the Parler desktop application. The independent `web-app/` changelog is separate.

## 1.0.0 — 2026-11-03

First versioned milestone of the customized local macOS installation. This is a local deployment, not a published or notarized release.

### Dictation and reliability

- Whisper Turbo with automatic French/English detection; toggle recording with Option + Space.
- Optional Mistral + Markdown brain correction using Option + Shift + Space.
- Preserve local transcription when correction is unavailable, invalid, empty or too slow. HTTP connection timeout: 2 seconds; HTTP total timeout: 5 seconds; complete asynchronous action deadline: 8 seconds after recognition. No automatic retry or delayed paste replacement.
- Green recording waveform and explicit model teardown on application exit.
- Stable local signing and deployment to `/Applications/Parler.app` to preserve macOS permissions.

### Context and review

- Local relevant-excerpt retrieval and a lexicon for vocabulary hints and correction.
- Reference-file discovery through `parler: lexicon` and `parler: communication` markers, supporting renamed files.
- Explicit channel profiles for language, tone and format; French e-mail default.
- Edit saved language models and replace their API keys without recreating actions.
- Opt-in local learning corpus: transcripts, model output, correction request, reference snapshots, manual corrections and optional independent audio. Collection is not automatic training.

### Smaller application

- Remove feedback-sound playback, Rodio and bundled WAV themes.
- Simplify general/advanced controls while retaining history, learning and model comparisons.
- Strip executable symbols in standard local deployments. Installed size after cleanup was approximately 39 MiB versus 52 MiB previously; downloaded model storage is separate.

### Validation

- Three local simulated API tests passed, covering successful correction, connection refusal, server errors, invalid/empty replies and stalled response bodies (5-second timeout).
- Frontend production build and signature verification passed. Installed version 1.0.0 opened with Whisper Turbo, automatic language detection and existing settings preserved.
- Retry local app launch when LaunchServices briefly reports error -600 after replacing the running bundle.

## Earlier local builds

Changes were developed on top of version 0.9.1 without separate release tags. The upstream Handy README is preserved in `docs/UPSTREAM_README.md` for attribution and historical reference.
