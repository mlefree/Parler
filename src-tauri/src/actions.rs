#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
use crate::apple_intelligence;
use crate::audio_toolkit::{is_microphone_access_denied, is_no_input_device_error};
use crate::managers::audio::AudioRecordingManager;
use crate::managers::history::HistoryManager;
use crate::managers::transcription::TranscriptionManager;
use crate::settings::{
    get_settings, AppSettings, PostProcessAction, APPLE_INTELLIGENCE_PROVIDER_ID,
};
use crate::shortcut;
use crate::tray::{change_tray_icon, TrayIconState};
use crate::utils::{
    self, show_processing_overlay, show_recording_overlay, show_transcribing_overlay,
};
use crate::TranscriptionCoordinator;
use ferrous_opencc::{config::BuiltinConfig, OpenCC};
use log::{debug, error};
use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tauri::Manager;
use tauri::{AppHandle, Emitter};

#[derive(Clone, serde::Serialize)]
struct RecordingErrorEvent {
    error_type: String,
    detail: Option<String>,
}

/// Id of the post-process action selected for the in-flight transcription,
/// set by the coordinator right before the pipeline stops.
pub struct ActiveActionState(pub Mutex<Option<String>>);

/// Drop guard that notifies the [`TranscriptionCoordinator`] when the
/// transcription pipeline finishes — whether it completes normally or panics.
struct FinishGuard(AppHandle);
impl Drop for FinishGuard {
    fn drop(&mut self) {
        if let Some(c) = self.0.try_state::<TranscriptionCoordinator>() {
            c.notify_processing_finished();
        }
    }
}

// Shortcut Action Trait
pub trait ShortcutAction: Send + Sync {
    fn start(&self, app: &AppHandle, binding_id: &str, shortcut_str: &str);
    fn stop(&self, app: &AppHandle, binding_id: &str, shortcut_str: &str);
}

// Transcribe Action
struct TranscribeAction {
    post_process: bool,
}

pub(crate) fn collect_learning_sample(
    app: &AppHandle,
    settings: &AppSettings,
    audio: Option<&std::path::Path>,
    details: &crate::managers::transcription::TranscriptionOutput,
    processed: &ProcessedTranscription,
    action: Option<&PostProcessAction>,
    trace: Option<serde_json::Value>,
    origin: &str,
    error: Option<&str>,
) {
    if !settings.learning.enabled || !get_settings(app).learning.enabled {
        return;
    }
    let configured_model = action.and_then(|a| {
        a.llm_model_id
            .as_deref()
            .and_then(|id| settings.llm_model(id))
            .or_else(|| settings.llm_models.first())
    });
    let input = crate::learning::Capture {
        raw_transcript: details.raw_text.clone(),
        transcript: details.text.clone(),
        model_output: processed.post_processed_text.clone(),
        output_text: processed.final_text.clone(),
        brain_root: action.and_then(|a| a.brain_path.clone()).or_else(|| {
            settings
                .default_post_process_action()
                .and_then(|a| a.brain_path.clone())
        }),
        metadata: serde_json::json!({"origin":origin,"asr":details,"error":error,
            "post_processing":{"action_id":action.map(|a|&a.id),"action_prompt":action.map(|a|&a.prompt),"configured_model":configured_model,
                "channel":action.and_then(|a|a.communication_channel.as_deref()),
                "status":if processed.post_processed_text.is_some(){"succeeded"}else if action.is_some(){"failed"}else{"not_requested"},
                "request":trace},
            "recording_settings":{"microphone":settings.selected_microphone,"language":settings.selected_language,"requested_model":settings.selected_model,
                "translate_to_english":settings.translate_to_english,"whisper_accelerator":settings.whisper_accelerator,
                "ort_accelerator":settings.ort_accelerator},"paste_status":"not_observed"}),
    };
    match crate::learning::capture(&settings.learning, audio, input) {
        Ok(Some(path)) => {
            let _ = app.emit(
                "learning-sample-added",
                path.file_name().map(|s| s.to_string_lossy().into_owned()),
            );
        }
        Ok(None) => {}
        Err(error) => {
            error!("Failed to save learning sample: {}", error);
            let _ = app.emit("learning-error", error);
        }
    }
}

/// Strip invisible Unicode characters that some LLMs may insert
fn strip_invisible_chars(s: &str) -> String {
    s.replace(['\u{200B}', '\u{200C}', '\u{200D}', '\u{FEFF}'], "")
}

/// Run a post-process action over the given text, resolving its saved
/// language model (falling back to the first saved model, then to the legacy
/// active provider configuration).
pub(crate) async fn run_post_process_action(
    settings: &AppSettings,
    text: &str,
    action: &PostProcessAction,
) -> Option<String> {
    run_post_process_action_with_trace(settings, text, action)
        .await
        .map(|result| result.text)
}

pub(crate) struct PostProcessOutput {
    pub text: String,
    pub trace: serde_json::Value,
}

/// Bound context retrieval and the cloud correction; callers retain local text on failure.
pub(crate) async fn run_post_process_action_with_trace(
    settings: &AppSettings,
    text: &str,
    action: &PostProcessAction,
) -> Option<PostProcessOutput> {
    match tokio::time::timeout(
        std::time::Duration::from_secs(8),
        run_post_process_action_inner(settings, text, action),
    )
    .await
    {
        Ok(result) => result,
        Err(_) => {
            error!("Post-processing exceeded 8 seconds; keeping the local transcription");
            None
        }
    }
}

async fn run_post_process_action_inner(
    settings: &AppSettings,
    text: &str,
    action: &PostProcessAction,
) -> Option<PostProcessOutput> {
    let model = action
        .llm_model_id
        .as_deref()
        .and_then(|id| settings.llm_model(id))
        .or_else(|| settings.llm_models.first());

    match model {
        Some(model) => {
            process_action(
                settings,
                text,
                &action.prompt,
                Some(&model.model),
                Some(&model.provider_id),
                action.brain_path.as_deref(),
                action.communication_channel.as_deref(),
            )
            .await
        }
        None => {
            debug!(
                "Action '{}' has no saved language model; using active provider configuration",
                action.id
            );
            process_action(
                settings,
                text,
                &action.prompt,
                None,
                None,
                action.brain_path.as_deref(),
                action.communication_channel.as_deref(),
            )
            .await
        }
    }
}

async fn process_action(
    settings: &AppSettings,
    transcription: &str,
    prompt: &str,
    action_model: Option<&str>,
    action_provider_id: Option<&str>,
    brain_path: Option<&str>,
    communication_channel: Option<&str>,
) -> Option<PostProcessOutput> {
    let provider = if let Some(pid) = action_provider_id.filter(|p| !p.is_empty()) {
        match settings.post_process_provider(pid).cloned() {
            Some(p) => p,
            None => {
                debug!(
                    "Action provider '{}' not found, falling back to active provider",
                    pid
                );
                settings.active_post_process_provider().cloned()?
            }
        }
    } else {
        match settings.active_post_process_provider().cloned() {
            Some(p) => p,
            None => {
                debug!("Action processing skipped: no provider configured");
                return None;
            }
        }
    };

    let model = action_model
        .filter(|m| !m.trim().is_empty())
        .map(|m| m.to_string())
        .or_else(|| settings.post_process_models.get(&provider.id).cloned())
        .unwrap_or_default();

    let mut full_prompt = if prompt.contains("${output}") {
        prompt.replace("${output}", transcription)
    } else {
        format!("{}\n\n{}", prompt, transcription)
    };

    let mut communication_profile = None;
    let mut preferred_terms = None;
    if let Some(path) = brain_path {
        let path = path.to_owned();
        let query = transcription.to_owned();
        let channel = communication_channel.map(str::to_owned);
        match tokio::task::spawn_blocking(move || {
            let context = crate::brain_context::retrieve(&path, &query)?;
            let profile =
                crate::communication::load(std::path::Path::new(&path), channel.as_deref())?;
            let terms = crate::lexicon::load(std::path::Path::new(&path))?.recognition_prompt(&[]);
            Ok::<_, String>((context, profile, terms))
        })
        .await
        {
            Ok(Ok((context, profile, terms))) => {
                communication_profile = profile;
                preferred_terms = terms;
                full_prompt.push_str("\n\n<reference_notes>\n");
                full_prompt.push_str(&context);
                full_prompt.push_str("\n</reference_notes>");
            }
            Ok(Err(error)) => {
                error!("Brain context or communication preferences unavailable: {}; keeping the original transcription", error);
                return None;
            }
            Err(error) => {
                error!(
                    "Brain context task failed: {}; keeping the original transcription",
                    error
                );
                return None;
            }
        }
    } else if communication_channel.is_some() {
        error!(
            "A communication channel requires a brain folder; keeping the original transcription"
        );
        return None;
    }

    debug!(
        "Starting action processing with provider '{}', model '{}', prompt length: {}",
        provider.id,
        model,
        full_prompt.len()
    );

    let system_prompt = crate::communication::system_prompt(
        brain_path.is_some(),
        communication_profile.as_ref(),
        preferred_terms.as_deref(),
    );
    let trace = if settings.learning.enabled {
        serde_json::json!({"provider_id":provider.id,"model":model,"channel":communication_channel,
            "system_prompt":system_prompt,"user_prompt":full_prompt,
            "temperature":if provider.id=="mistral" {Some(0.0)} else {None}})
    } else {
        serde_json::Value::Null
    };

    // Handle Apple Intelligence via native Swift APIs
    if provider.id == APPLE_INTELLIGENCE_PROVIDER_ID {
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        {
            if !apple_intelligence::check_apple_intelligence_availability() {
                debug!("Apple Intelligence selected but not available for action processing");
                return None;
            }
            let token_limit = model.trim().parse::<i32>().unwrap_or(0);
            return match apple_intelligence::process_text_with_system_prompt(
                &system_prompt,
                &full_prompt,
                token_limit,
            ) {
                Ok(result) if !result.trim().is_empty() => {
                    let result = strip_invisible_chars(&result);
                    debug!(
                        "Apple Intelligence action processing succeeded. Output length: {} chars",
                        result.len()
                    );
                    Some(PostProcessOutput {
                        text: result,
                        trace,
                    })
                }
                Ok(_) => {
                    debug!("Apple Intelligence action returned empty result");
                    None
                }
                Err(err) => {
                    error!("Apple Intelligence action processing failed: {}", err);
                    None
                }
            };
        }

        #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
        {
            debug!("Apple Intelligence provider selected on unsupported platform");
            return None;
        }
    }

    if model.trim().is_empty() {
        debug!(
            "Action processing skipped: no model configured for provider '{}'",
            provider.id
        );
        return None;
    }

    let api_key = settings
        .post_process_api_keys
        .get(&provider.id)
        .cloned()
        .unwrap_or_default();

    if provider.id == "mistral" && api_key.trim().is_empty() {
        error!("Mistral API key is missing; keeping the original transcription");
        return None;
    }

    match crate::llm_client::send_chat_completion_with_schema(
        &provider,
        api_key,
        &model,
        full_prompt,
        Some(system_prompt),
        None,
    )
    .await
    {
        Ok(Some(content)) if !content.trim().is_empty() => {
            let result = strip_invisible_chars(&content);
            debug!(
                "Action processing succeeded for provider '{}'. Output length: {} chars",
                provider.id,
                result.len()
            );
            Some(PostProcessOutput {
                text: result,
                trace,
            })
        }
        Ok(_) => {
            debug!("Action processing returned empty result");
            None
        }
        Err(e) => {
            error!(
                "Action processing failed for provider '{}': {}",
                provider.id, e
            );
            None
        }
    }
}

async fn maybe_convert_chinese_variant(
    settings: &AppSettings,
    transcription: &str,
) -> Option<String> {
    // Check if language is set to Simplified or Traditional Chinese
    let is_simplified = settings.selected_language == "zh-Hans";
    let is_traditional = settings.selected_language == "zh-Hant";

    if !is_simplified && !is_traditional {
        debug!("selected_language is not Simplified or Traditional Chinese; skipping translation");
        return None;
    }

    debug!(
        "Starting Chinese translation using OpenCC for language: {}",
        settings.selected_language
    );

    // Use OpenCC to convert based on selected language
    let config = if is_simplified {
        // Convert Traditional Chinese to Simplified Chinese
        BuiltinConfig::Tw2sp
    } else {
        // Convert Simplified Chinese to Traditional Chinese
        BuiltinConfig::S2tw
    };

    match OpenCC::from_config(config) {
        Ok(converter) => {
            let converted = converter.convert(transcription);
            debug!(
                "OpenCC translation completed. Input length: {}, Output length: {}",
                transcription.len(),
                converted.len()
            );
            Some(converted)
        }
        Err(e) => {
            error!("Failed to initialize OpenCC converter: {}. Falling back to original transcription.", e);
            None
        }
    }
}

pub(crate) struct ProcessedTranscription {
    pub final_text: String,
    pub post_processed_text: Option<String>,
    pub post_process_prompt: Option<String>,
}

pub(crate) async fn process_transcription_output(
    app: &AppHandle,
    transcription: &str,
    post_process: bool,
) -> ProcessedTranscription {
    let settings = get_settings(app);
    let mut final_text = transcription.to_string();
    let mut post_processed_text: Option<String> = None;
    let mut post_process_prompt: Option<String> = None;

    if let Some(converted_text) = maybe_convert_chinese_variant(&settings, transcription).await {
        final_text = converted_text;
    }

    if post_process {
        if let Some(action) = settings.default_post_process_action().cloned() {
            if let Some(processed_text) =
                run_post_process_action(&settings, &final_text, &action).await
            {
                post_processed_text = Some(processed_text.clone());
                final_text = processed_text;
                post_process_prompt = Some(action.prompt);
            }
        } else {
            debug!("Post-processing requested but no action is configured");
        }
    } else if final_text != transcription {
        post_processed_text = Some(final_text.clone());
    }

    ProcessedTranscription {
        final_text,
        post_processed_text,
        post_process_prompt,
    }
}

impl ShortcutAction for TranscribeAction {
    fn start(&self, app: &AppHandle, binding_id: &str, _shortcut_str: &str) {
        let start_time = Instant::now();
        debug!("TranscribeAction::start called for binding: {}", binding_id);

        // Load model in the background
        let tm = app.state::<Arc<TranscriptionManager>>();
        tm.initiate_model_load();

        let binding_id = binding_id.to_string();
        change_tray_icon(app, TrayIconState::Recording);
        show_recording_overlay(app);

        let rm = app.state::<Arc<AudioRecordingManager>>();

        // Apply mute after the microphone starts.
        let settings = get_settings(app);
        let is_always_on = settings.always_on_microphone;
        debug!("Microphone mode - always_on: {}", is_always_on);

        let mut recording_error: Option<String> = None;
        if is_always_on {
            // The stream is already running in always-on mode.
            debug!("Always-on mode: applying mute");
            let rm_clone = Arc::clone(&rm);
            // Apply system mute without a feedback playback device.
            std::thread::spawn(move || {
                rm_clone.apply_mute();
            });

            if let Err(e) = rm.try_start_recording(&binding_id) {
                debug!("Recording failed: {}", e);
                recording_error = Some(e);
            }
        } else {
            // Start the microphone before applying mute.
            debug!("On-demand mode: starting recording before mute");
            let recording_start_time = Instant::now();
            match rm.try_start_recording(&binding_id) {
                Ok(()) => {
                    debug!("Recording started in {:?}", recording_start_time.elapsed());
                    // Small delay to ensure microphone stream is active
                    let rm_clone = Arc::clone(&rm);
                    std::thread::spawn(move || {
                        std::thread::sleep(std::time::Duration::from_millis(100));
                        debug!("Applying delayed recording mute");

                        rm_clone.apply_mute();
                    });
                }
                Err(e) => {
                    debug!("Failed to start recording: {}", e);
                    recording_error = Some(e);
                }
            }
        }

        if recording_error.is_none() {
            // Dynamically register the cancel shortcut in a separate task to avoid deadlock
            shortcut::register_cancel_shortcut(app);
            // Register the bare-digit trigger keys for post-process actions
            shortcut::register_recording_action_shortcuts(app);
        } else {
            // Starting failed (for example due to blocked microphone permissions).
            // Revert UI state so we don't stay stuck in the recording overlay.
            utils::hide_recording_overlay(app);
            change_tray_icon(app, TrayIconState::Idle);
            if let Some(err) = recording_error {
                let error_type = if is_microphone_access_denied(&err) {
                    "microphone_permission_denied"
                } else if is_no_input_device_error(&err) {
                    "no_input_device"
                } else {
                    "unknown"
                };
                let _ = app.emit(
                    "recording-error",
                    RecordingErrorEvent {
                        error_type: error_type.to_string(),
                        detail: Some(err),
                    },
                );
            }
        }

        debug!(
            "TranscribeAction::start completed in {:?}",
            start_time.elapsed()
        );
    }

    fn stop(&self, app: &AppHandle, binding_id: &str, _shortcut_str: &str) {
        // Unregister the cancel shortcut when transcription stops
        shortcut::unregister_cancel_shortcut(app);
        shortcut::unregister_recording_action_shortcuts(app);

        let stop_time = Instant::now();
        debug!("TranscribeAction::stop called for binding: {}", binding_id);

        let ah = app.clone();
        let rm = Arc::clone(&app.state::<Arc<AudioRecordingManager>>());
        let tm = Arc::clone(&app.state::<Arc<TranscriptionManager>>());
        let hm = Arc::clone(&app.state::<Arc<HistoryManager>>());

        change_tray_icon(app, TrayIconState::Transcribing);
        show_transcribing_overlay(app);

        // Restore system audio after recording.
        rm.remove_mute();

        let binding_id = binding_id.to_string();
        let post_process = self.post_process;

        let selected_action_id =
            app.try_state::<ActiveActionState>()
                .and_then(|s| match s.0.lock() {
                    Ok(mut guard) => guard.take(),
                    Err(poisoned) => {
                        error!("ActiveActionState mutex poisoned, recovering");
                        poisoned.into_inner().take()
                    }
                });

        tauri::async_runtime::spawn(async move {
            let _guard = FinishGuard(ah.clone());
            debug!(
                "Starting async transcription task for binding: {}, action: {:?}",
                binding_id, selected_action_id
            );

            let stop_recording_time = Instant::now();
            if let Some(samples) = rm.stop_recording(&binding_id) {
                debug!(
                    "Recording stopped and samples retrieved in {:?}, sample count: {}",
                    stop_recording_time.elapsed(),
                    samples.len()
                );

                if samples.is_empty() {
                    debug!("Recording produced no audio samples; skipping persistence");
                    utils::hide_recording_overlay(&ah);
                    change_tray_icon(&ah, TrayIconState::Idle);
                } else {
                    // Save WAV concurrently with transcription
                    let sample_count = samples.len();
                    let file_name = format!("handy-{}.wav", chrono::Utc::now().timestamp());
                    let wav_path = hm.recordings_dir().join(&file_name);
                    let wav_path_for_verify = wav_path.clone();
                    let samples_for_wav = samples.clone();
                    let wav_handle = tauri::async_runtime::spawn_blocking(move || {
                        crate::audio_toolkit::save_wav_file(&wav_path, &samples_for_wav)
                    });

                    // Transcribe concurrently with WAV save
                    let transcription_time = Instant::now();
                    let transcription_settings = get_settings(&ah);
                    let brain_path = selected_action_id
                        .as_deref()
                        .and_then(|id| transcription_settings.post_process_action(id))
                        .or_else(|| transcription_settings.default_post_process_action())
                        .and_then(|a| a.brain_path.as_deref());
                    let transcription_result =
                        tm.transcribe_details_with_brain(samples, brain_path);

                    // Await WAV save and verify
                    let wav_saved = match wav_handle.await {
                        Ok(Ok(())) => {
                            match crate::audio_toolkit::verify_wav_file(
                                &wav_path_for_verify,
                                sample_count,
                            ) {
                                Ok(()) => true,
                                Err(e) => {
                                    error!("WAV verification failed: {}", e);
                                    false
                                }
                            }
                        }
                        Ok(Err(e)) => {
                            error!("Failed to save WAV file: {}", e);
                            false
                        }
                        Err(e) => {
                            error!("WAV save task panicked: {}", e);
                            false
                        }
                    };

                    match transcription_result {
                        Ok(details) => {
                            let transcription = details.text.clone();
                            debug!(
                                "Transcription completed in {:?}: '{}'",
                                transcription_time.elapsed(),
                                transcription
                            );

                            let settings = get_settings(&ah);

                            // Resolve the post-process action: explicitly selected
                            // during recording (trigger key or per-action shortcut),
                            // otherwise the default action when the generic
                            // post-process shortcut was used.
                            let action = selected_action_id
                                .as_deref()
                                .and_then(|id| settings.post_process_action(id))
                                .or_else(|| {
                                    if post_process {
                                        settings.default_post_process_action()
                                    } else {
                                        None
                                    }
                                })
                                .cloned();
                            let post_process_requested = post_process || action.is_some();

                            let mut learning_trace = None;
                            let processed = if let Some(ref action) = action {
                                show_processing_overlay(&ah);
                                let base =
                                    process_transcription_output(&ah, &transcription, false).await;
                                match run_post_process_action_with_trace(
                                    &settings,
                                    &base.final_text,
                                    action,
                                )
                                .await
                                {
                                    Some(result) => {
                                        learning_trace = Some(result.trace);
                                        ProcessedTranscription {
                                            final_text: result.text.clone(),
                                            post_processed_text: Some(result.text),
                                            post_process_prompt: Some(action.prompt.clone()),
                                        }
                                    }
                                    None => {
                                        let _ = ah.emit("post-process-fallback", ());
                                        base
                                    }
                                }
                            } else {
                                process_transcription_output(&ah, &transcription, false).await
                            };

                            if transcription_settings.learning.enabled {
                                collect_learning_sample(
                                    &ah,
                                    &settings,
                                    if wav_saved {
                                        Some(wav_path_for_verify.as_path())
                                    } else {
                                        None
                                    },
                                    &details,
                                    &processed,
                                    action.as_ref(),
                                    learning_trace,
                                    "live-dictation",
                                    None,
                                );
                            }

                            // Save to history if WAV was saved
                            if wav_saved {
                                if let Err(err) = hm.save_entry(
                                    file_name,
                                    transcription,
                                    post_process_requested,
                                    processed.post_processed_text.clone(),
                                    processed.post_process_prompt.clone(),
                                ) {
                                    error!("Failed to save history entry: {}", err);
                                }
                            }

                            if processed.final_text.is_empty() {
                                utils::hide_recording_overlay(&ah);
                                change_tray_icon(&ah, TrayIconState::Idle);
                            } else {
                                let ah_clone = ah.clone();
                                let paste_time = Instant::now();
                                let final_text = processed.final_text;
                                ah.run_on_main_thread(move || {
                                    match utils::paste(final_text, ah_clone.clone()) {
                                        Ok(()) => debug!(
                                            "Text pasted successfully in {:?}",
                                            paste_time.elapsed()
                                        ),
                                        Err(e) => error!("Failed to paste transcription: {}", e),
                                    }
                                    utils::hide_recording_overlay(&ah_clone);
                                    change_tray_icon(&ah_clone, TrayIconState::Idle);
                                })
                                .unwrap_or_else(|e| {
                                    error!("Failed to run paste on main thread: {:?}", e);
                                    utils::hide_recording_overlay(&ah);
                                    change_tray_icon(&ah, TrayIconState::Idle);
                                });
                            }
                        }
                        Err(err) => {
                            debug!("Global Shortcut Transcription error: {}", err);
                            if transcription_settings.learning.enabled {
                                let details =
                                    crate::managers::transcription::TranscriptionOutput::default();
                                let processed = ProcessedTranscription {
                                    final_text: String::new(),
                                    post_processed_text: None,
                                    post_process_prompt: None,
                                };
                                collect_learning_sample(
                                    &ah,
                                    &transcription_settings,
                                    if wav_saved {
                                        Some(wav_path_for_verify.as_path())
                                    } else {
                                        None
                                    },
                                    &details,
                                    &processed,
                                    None,
                                    None,
                                    "live-dictation",
                                    Some(&err.to_string()),
                                );
                            }
                            // Save entry with empty text so user can retry
                            if wav_saved {
                                if let Err(save_err) = hm.save_entry(
                                    file_name,
                                    String::new(),
                                    post_process,
                                    None,
                                    None,
                                ) {
                                    error!("Failed to save failed history entry: {}", save_err);
                                }
                            }
                            utils::hide_recording_overlay(&ah);
                            change_tray_icon(&ah, TrayIconState::Idle);
                        }
                    }
                }
            } else {
                debug!("No samples retrieved from recording stop");
                utils::hide_recording_overlay(&ah);
                change_tray_icon(&ah, TrayIconState::Idle);
            }
        });

        debug!(
            "TranscribeAction::stop completed in {:?}",
            stop_time.elapsed()
        );
    }
}

// Cancel Action
struct CancelAction;

impl ShortcutAction for CancelAction {
    fn start(&self, app: &AppHandle, _binding_id: &str, _shortcut_str: &str) {
        utils::cancel_current_operation(app);
    }

    fn stop(&self, _app: &AppHandle, _binding_id: &str, _shortcut_str: &str) {
        // Nothing to do on stop for cancel
    }
}

// Test Action
struct TestAction;

impl ShortcutAction for TestAction {
    fn start(&self, app: &AppHandle, binding_id: &str, shortcut_str: &str) {
        log::info!(
            "Shortcut ID '{}': Started - {} (App: {})", // Changed "Pressed" to "Started" for consistency
            binding_id,
            shortcut_str,
            app.package_info().name
        );
    }

    fn stop(&self, app: &AppHandle, binding_id: &str, shortcut_str: &str) {
        log::info!(
            "Shortcut ID '{}': Stopped - {} (App: {})", // Changed "Released" to "Stopped" for consistency
            binding_id,
            shortcut_str,
            app.package_info().name
        );
    }
}

// Static Action Map
pub static ACTION_MAP: Lazy<HashMap<String, Arc<dyn ShortcutAction>>> = Lazy::new(|| {
    let mut map = HashMap::new();
    map.insert(
        "transcribe".to_string(),
        Arc::new(TranscribeAction {
            post_process: false,
        }) as Arc<dyn ShortcutAction>,
    );
    map.insert(
        "transcribe_with_post_process".to_string(),
        Arc::new(TranscribeAction { post_process: true }) as Arc<dyn ShortcutAction>,
    );
    map.insert(
        "cancel".to_string(),
        Arc::new(CancelAction) as Arc<dyn ShortcutAction>,
    );
    map.insert(
        "test".to_string(),
        Arc::new(TestAction) as Arc<dyn ShortcutAction>,
    );
    map
});

#[cfg(test)]
mod offline_fallback_tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn fixture(url: String) -> (AppSettings, PostProcessAction) {
        let mut settings = crate::settings::get_default_settings();
        let provider = settings
            .post_process_providers
            .iter_mut()
            .find(|p| p.id == "mistral")
            .unwrap();
        provider.base_url = url;
        settings
            .post_process_api_keys
            .insert("mistral".into(), "local-test-only".into());
        settings.llm_models = vec![crate::settings::LLMModel {
            id: "test".into(),
            provider_id: "mistral".into(),
            model: "test".into(),
            label: "Test".into(),
        }];
        let action = PostProcessAction {
            id: "test".into(),
            name: "Test".into(),
            prompt: "Correct ${output}".into(),
            llm_model_id: Some("test".into()),
            icon: "sparkles".into(),
            trigger_key: None,
            brain_path: None,
            communication_channel: None,
        };
        (settings, action)
    }

    async fn server(response: String, stall: bool) -> (String, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = vec![0; 8192];
            socket.read(&mut buf).await.unwrap();
            socket.write_all(response.as_bytes()).await.unwrap();
            if stall {
                tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            }
        });
        (url, task)
    }

    #[tokio::test]
    async fn correction_errors_fall_back_and_success_is_preserved() {
        for (status, body, expected) in [
            (500, "unavailable", None),
            (200, "invalid json", None),
            (200, r#"{"choices":[]}"#, None),
            (200, r#"{"choices":[{"message":{"content":"   "}}]}"#, None),
            (
                200,
                r#"{"choices":[{"message":{"content":"MEG and DataForge."}}]}"#,
                Some("MEG and DataForge."),
            ),
        ] {
            let response = format!("HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
            let (url, task) = server(response, false).await;
            let (settings, action) = fixture(url);
            let result = run_post_process_action(&settings, "Meg and Data Forge", &action).await;
            assert_eq!(result.as_deref(), expected);
            task.await.unwrap();
        }
    }

    #[tokio::test]
    async fn correction_stalled_response_body_is_bounded() {
        // Headers arrive promptly, but the JSON body never finishes.
        let (url, task) = server(
            "HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\n{".into(),
            true,
        )
        .await;
        let (settings, action) = fixture(url);
        let start = Instant::now();
        let result = run_post_process_action(&settings, "Texte local intact", &action).await;
        assert!(result.is_none());
        assert!(start.elapsed() >= std::time::Duration::from_secs(4));
        assert!(start.elapsed() < std::time::Duration::from_secs(7));
        task.abort();
    }

    #[tokio::test]
    async fn correction_connection_refused_returns_local_fallback() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        drop(listener);
        let (settings, action) = fixture(url);
        let start = Instant::now();
        assert!(
            run_post_process_action(&settings, "Keep this local text", &action)
                .await
                .is_none()
        );
        assert!(start.elapsed() < std::time::Duration::from_secs(2));
    }
}
