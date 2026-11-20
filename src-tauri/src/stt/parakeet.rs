//! Parakeet (on-device) batch STT adapter. Unlike the cloud adapters and
//! local Whisper, Parakeet runs in the `speaker-diarize` Swift sidecar
//! (FluidAudio / Neural Engine), so the adapter holds a Tauri `AppHandle`
//! and shells out per chunk via `diarize::transcribe_file`.
//!
//! Parakeet transcribes a whole WAV and returns text plus optional per-token
//! timings — the same `(text, words)` shape the rest of the pipeline expects,
//! so chunk-relative ms rebase onto the playback clock exactly as Whisper's
//! words do. It takes no `initial_prompt`/vocabulary bias (the model has no
//! such knob), so `ctx.bias_terms` / `ctx.prior_context` are ignored.

use anyhow::Result;
use async_trait::async_trait;
use std::path::Path;
use tauri::AppHandle;

use crate::diarize;
use crate::stt::adapter::{BatchSttAdapter, TranscribeCtx, TranscribeResult, Word};

pub struct ParakeetAdapter {
    app: AppHandle,
}

impl ParakeetAdapter {
    pub fn new(app: AppHandle) -> Self {
        Self { app }
    }
}

#[async_trait]
impl BatchSttAdapter for ParakeetAdapter {
    fn provider_id(&self) -> &'static str {
        "parakeet"
    }

    fn label(&self) -> &'static str {
        "Parakeet (on-device)"
    }

    fn supports_language(&self, _lang: &str) -> bool {
        true
    }

    fn supports_word_timestamps(&self, _model: &str) -> bool {
        // Parakeet returns `tokenTimings` when available; the sidecar omits the
        // field otherwise and the adapter yields an empty `words` vec, which
        // downstream treats as "no per-word highlight for this chunk".
        true
    }

    async fn transcribe(
        &self,
        ctx: TranscribeCtx<'_>,
        audio: &Path,
    ) -> Result<TranscribeResult> {
        let out = diarize::transcribe_file(&self.app, audio, ctx.language).await?;
        let words = out
            .words
            .into_iter()
            .map(|(text, start_ms, end_ms)| Word { text, start_ms, end_ms })
            .collect();
        // Parakeet reports no detected language (it transcribes to the
        // requested/implicit language); None is the honest answer.
        Ok(TranscribeResult { text: out.text, words, detected_language: None })
    }
}
