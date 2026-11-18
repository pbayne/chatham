# Local STT Re-evaluation for Chatham (macOS M3 Pro) — 2026

Scope: on-device transcription upgrade for a meeting-notes app that today uses
**whisper.cpp** (`whisper-rs 0.16`, Metal, default `large-v3-turbo-q5`) and already
bundles a **FluidAudio** Swift sidecar for diarization. No Python. Sidecars are
prebuilt binaries via Tauri `externalBin`. Findings only — no production code.

All URLs below were fetched live and returned 200/valid content unless flagged.

---

## TL;DR

- **Best upgrade to implement first: Parakeet via the FluidAudio sidecar you already ship.**
  FluidAudio exposes a full ASR surface (`AsrManager`, `AsrModels.downloadAndLoad`), auto-downloads
  CoreML models from `huggingface.co/FluidInference`, runs on the **ANE**, and its Parakeet family
  tops the Open ASR English leaderboard. **Zero new Rust crates** — extend the existing Swift sidecar.
- **Moonshine has genuinely leveled up** (new *Medium Streaming* 245M at 6.65% Open-ASR avg, beating
  Whisper Large v3's 7.44%) — but it is still a **low-latency streaming / short-window** family
  (recommended <30 s input, CPU/ONNX on Mac, no ANE). It is a **live-caption** tool, **not** a
  meeting-accuracy default. The prior conclusion **still holds**, just update it from "tiny/base only"
  to "there's now a Medium, but it's a streaming model and the Mac path is CPU-bound."
- Prior research's verdict (**Parakeet-via-FluidAudio = the real upgrade; Moonshine = wrong tool for
  the quality default**) is **re-confirmed**, with the nuance that Moonshine is now a credible
  *optional* live-caption mode.

---

## 1. Moonshine.ai / Useful Sensors Moonshine — LATEST state

The org rebranded to **Moonshine AI** (`moonshine-ai`, formerly Useful Sensors). The repo is now a
broader "Moonshine Voice" voice-agent toolkit (STT + TTS "Phonon" + agents), very actively developed
(684 commits, 11.5k stars, commits within the last 2 weeks).

- Repo: https://github.com/moonshine-ai/moonshine
- Docs: https://moonshine-voice.readthedocs.io/en/latest/
- HF org: https://huggingface.co/moonshine-ai (32 models)
- Swift package: https://github.com/moonshine-ai/moonshine-swift
- v2 paper: https://arxiv.org/abs/2410.15608 and https://arxiv.org/abs/2509.02523 ("Flavors of Moonshine")

### Is there now a larger model beyond tiny/base? — YES

A **second generation of streaming models** now exists. English lineup (from
`docs/models/available-models.md`):

| Model | Params | Open-ASR avg WER | License |
|-------|--------|------------------|---------|
| **Medium Streaming** | 245M | **6.65%** | MIT |
| Small Streaming | 123M | 7.84% | MIT |
| Tiny Streaming | 34M | 12.00% | MIT |
| Base (legacy, deprecated) | 58M | 10.07% | MIT |
| Tiny (legacy) | 26M | 12.66% | MIT |

Source: https://raw.githubusercontent.com/moonshine-ai/moonshine/main/docs/models/available-models.md

### Real accuracy vs whisper-large-v3-turbo on long-form meetings — HONEST READ

- Moonshine's own comparison table (`docs/moonshine-vs-whisper.md`) shows **Medium Streaming 6.65%
  WER vs Whisper Large v3 7.44%** on the Open ASR Leaderboard — a real, verifiable win at 1/6 the
  params. URL: https://raw.githubusercontent.com/moonshine-ai/moonshine/main/docs/moonshine-vs-whisper.md
- **BUT three big caveats for the meeting use case:**
  1. These are **Open ASR Leaderboard / LibriSpeech-style clean read-speech** numbers, not
     multi-speaker far-field meeting audio. Moonshine does **not** publish meeting-corpus (AMI/
     Earnings) WER. The accuracy doc explicitly says the headline figures are the "Open ASR
     Leaderboard average across eight datasets" measured on **floating-point reference** models in
     HF Transformers — *the shipped files are 8-bit quantized `.ort`* (slightly worse).
     Source: https://raw.githubusercontent.com/moonshine-ai/moonshine/main/docs/models/accuracy.md
  2. It is architecturally a **streaming, short-window** model: docs recommend **staying below ~30 s
     of audio per call** and the whole design goal is <200 ms live latency, not long-form fidelity.
  3. There is **no comparison vs whisper-large-v3-turbo specifically** (only vanilla Large v3) and
     **no diarization** — it emits a stream, not speaker-attributed meeting transcript.
- **Verdict:** Moonshine Medium is a legitimate low-latency live-caption engine that is *competitive
  with* Large v3 on clean speech, but it is **not demonstrated to beat large-v3-turbo on real meeting
  audio**, and it is not a quality default. Treat the "beats Whisper" claim as true-but-narrow.

### Python-free shipping — YES, two clean paths

- **Swift Package** `MoonshineVoice` from https://github.com/moonshine-ai/moonshine-swift — `import
  MoonshineVoice`, `Transcriber(modelPath:modelArch:)` / `MicTranscriber`. macOS example project:
  https://github.com/moonshine-ai/moonshine/tree/main/examples/macos/BasicTranscription
- **Prebuilt C/C++ library** on GitHub Releases: `moonshine-voice-macos-arm64.tar.gz` (current tag
  **v0.1.6**) ships a **static `libmoonshine.a`** + C and C++ headers + bundled ONNX Runtime, plus the
  **Medium Streaming English model** in `medium-streaming-en/`.
  URL pattern (verified in docs):
  `https://github.com/moonshine-ai/moonshine/releases/download/v0.1.6/moonshine-voice-macos-arm64.tar.gz`
  Download helper: https://github.com/moonshine-ai/moonshine/blob/main/examples/c%2B%2B/README.md
- Model format: ONNX converted to memory-mappable OnnxRuntime `.ort` flatbuffers; safetensors also on HF.
- **Note:** the new English *streaming* models are **not standalone HF repos** — the standalone HF
  repos (`moonshine-base-*`, `moonshine-streaming-tiny-*`) are the per-language/legacy set;
  `moonshine-streaming-medium-en` as a HF URL **404s** (flagged — confirmed 404/401 live). The Medium
  English weights ship via the GitHub release archive + `moonshine-ai/moonshine-voice-assets` catalog,
  fetched automatically by the bindings. So for a Mac app you get Medium via the Swift package or the
  release tarball, not a public HF model-card URL.

### Apple-Silicon acceleration — CPU via ONNX Runtime (NOT ANE)

The portable core uses **ONNX Runtime** for cross-platform inference. On Mac the prebuilt lib links a
bundled `libonnxruntime`; there is **no CoreML/ANE pipeline** like FluidAudio/WhisperKit. It doesn't
need one — Medium Streaming is 245M and clocks **59 ms on a MacBook Pro** per their table, so CPU is
fine for live captions. But it means Moonshine does **not** exploit the ANE the way the ANE-native
options do, and running it during a meeting competes for CPU with everything else.

### sherpa-onnx path — only the OLD tiny model

sherpa-onnx ships **only `sherpa-onnx-moonshine-tiny-en-int8`** (old gen), *not* the new Medium
Streaming. Verified:
https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-moonshine-tiny-en-int8.tar.bz2
So the "Moonshine via sherpa-onnx/sherpa-rs sidecar" route gets you the **downgrade** tiny model, not
the new one. To get Medium you must use Moonshine's own Swift package / C lib.

---

## 2. Parakeet TDT via FluidAudio — CONFIRMED on-device, ANE, leaderboard-topping

FluidAudio is now a mature SDK at **v0.17.7** (README pins `from: "0.17.7"`), 500k+ monthly HF
downloads, used by many shipping Mac meeting apps (Talat, Slipbox, OpenOats, oats (Tauri+Vue!),
Logue, Thoth, etc.).

- Repo: https://github.com/FluidInference/FluidAudio
- Docs: https://docs.fluidinference.com/introduction
- HF org: https://huggingface.co/FluidInference
- Models reference: https://raw.githubusercontent.com/FluidInference/FluidAudio/main/Documentation/Models.md

### On-device / ANE — YES

"Fully local, low-latency audio AI on Apple devices, with inference offloaded to the **Apple Neural
Engine (ANE)**… avoiding GPU/MPS entirely." Same SDK you already bundle for diarization.

### ASR Swift API surface (verified from README) — this is the integration contract

```swift
import FluidAudio

Task {
    // 1) Download + load CoreML models (auto-fetched from huggingface.co/FluidInference)
    let models = try await AsrModels.downloadAndLoad(version: .ultra)  // .v3 default; .v2 English-only
    let asrManager = AsrManager(config: .default)
    try await asrManager.loadModels(models)

    // 2) Transcribe 16 kHz samples, a file URL, or an AVAudioPCMBuffer
    let result = try await asrManager.transcribe(samples)        // [Float] @16kHz
    // let result = try await asrManager.transcribe(audioBuffer) // AVAudioPCMBuffer
    print(result.text)
}
```
Long-form is handled by `SlidingWindowAsrManager` (chunk ~15 s + overlap + stitch). CLI exists too:
`swift run fluidaudiocli transcribe audio.wav`.
Source: https://raw.githubusercontent.com/FluidInference/FluidAudio/main/README.md (lines ~353–383)

### Model versions & repos (all verified in Models.md)

| Version enum | What | Repo |
|---|---|---|
| `.v3` (default) | Parakeet TDT 0.6B, **25 European langs** | https://huggingface.co/FluidInference/parakeet-tdt-0.6b-v3-coreml |
| `.v2` | Parakeet TDT 0.6B, English-only | https://huggingface.co/FluidInference/parakeet-tdt-0.6b-v2-coreml |
| `.ultra` (**recommended**) | moondream post-train of v3; more accurate at same speed (LibriSpeech test-clean 2.13% vs 2.27%), int8 encoder 595 MB, ANE, iOS17+ | https://huggingface.co/FluidInference/parakeet-ultra-coreml |

The upstream NVIDIA Parakeet TDT 0.6B v2 is the model that **tops the HF Open ASR English
leaderboard** (https://huggingface.co/spaces/hf-audio/open_asr_leaderboard); FluidAudio ships a CoreML
conversion of that family and v3 adds multilingual.

### Meeting-relevant extras FluidAudio now has (new since prior research)

- **Nemotron 3.5 Transcribe Lite** — long-form English **with word timestamps + speakers**, 49.5-min
  AMI meeting in 5.5 s on M5 Pro; honestly flagged as **26.4% WER / 28.7% cpWER** (fast, not as
  accurate as full model). `NemotronTranscribeLiteManager`.
  https://huggingface.co/FluidInference/nemotron-3.5-transcribe-lite-coreml
- **Parakeet EOU** 120M streaming (`/160ms`,`/320ms`,`/1280ms`) for live partials —
  this is FluidAudio's answer to the same niche Moonshine targets, **on the ANE**:
  https://huggingface.co/FluidInference/parakeet-realtime-eou-120m-coreml
- **Bark 3.0 / Bark 3.0 Clean** — post-trains of v3 tuned for **verbatim meeting transcripts** (Bark
  3.0) vs cleaned dictation (Clean): https://huggingface.co/FluidInference/bark-3.0-coreml

### New Rust crates? — NONE if you extend the Swift sidecar

Chatham already ships a FluidAudio Swift sidecar (`speaker-diarize`). Adding ASR is **the same SDK,
same binary toolchain, same HF auto-download mechanism** — just call `AsrManager` in a new (or the
same) sidecar binary referenced by `externalBin`. **No Cargo change, no dependency-proxy risk.**
(`fluidaudio-rs ~0.1` exists — https://github.com/FluidInference/fluidaudio-rs — but it's a brand-new
`cargo add` crate = proxy-risky; the Swift-sidecar path avoids crates entirely and is what you already
do.)

### Scorecard

- Runtime dependency: CoreML (OS-provided) + FluidAudio SPM in the Swift sidecar.
- Shipping: **prebuilt Swift sidecar binary** (extend existing) + CoreML models auto-downloaded to cache.
- Acceleration: **ANE** (native).
- Integration effort: **LOW–MED** (SDK already vendored; add an ASR entrypoint + wire Rust to call it
  like the diarize sidecar; replace per-chunk whisper.cpp calls or run a stop-time full-file pass).
- New crates: **none**.

---

## 3. WhisperKit (Argmax) — CONFIRMED, solid #2

- Repo (now part of argmax-oss-swift): https://github.com/argmaxinc/argmax-oss-swift
  (classic: https://github.com/argmaxinc/WhisperKit)
- Model repo (verified, 81M downloads): https://huggingface.co/argmaxinc/whisperkit-coreml
  — includes `openai_whisper-large-v3-v20240930_turbo` CoreML variants (the large-v3-turbo CoreML build).
- ICML 2025 paper: https://icml.cc/virtual/2025/47854

Scorecard:
- Runtime: CoreML; Swift package shippable as a sidecar binary.
- Shipping: **Swift sidecar** + CoreML model folders from `whisperkit-coreml`.
- Acceleration: **ANE** via CoreML.
- Integration effort: **MED** (new sidecar; but it's still Whisper — same accuracy ceiling you
  already have, just moved from Metal/whisper.cpp to ANE/CoreML with better battery + speed).
- New crates: none (Swift package).
- **Why it's #2 not #1:** it keeps you on the Whisper accuracy curve. It's the right move only if you
  want to stay on Whisper semantics (identical output behavior, prompt/initial_prompt support) but get
  ANE efficiency. For a genuine *accuracy* jump, Parakeet beats large-v3-turbo on the leaderboard.

---

## 4. Other strong 2026 local STT on Apple Silicon

- **NVIDIA Canary / Parakeet upstream** — already covered via FluidAudio conversions; no separate Mac
  path worth a new sidecar.
- **Cohere Transcribe (via FluidAudio)** — 14-lang encoder-decoder, but 1.8 GB INT8 encoder + 35 s
  hard cap; heavy, not worth it over Parakeet for English meetings.
  https://huggingface.co/FluidInference/cohere-transcribe-03-2026-coreml
- **LFM3-Audio-2B (Liquid AI, via FluidAudio)** — one model for ASR+liveASR+TTS, LibriSpeech
  test-clean 1.32% WER but ~2.5 GB; overkill unless you also want on-device TTS.
  https://huggingface.co/FluidInference/lfm3-audio-2b-coreml
- **sherpa-onnx / sherpa-rs** — the one genuinely *Rust-crate* path (`sherpa-rs ~Oct 2025`, within
  proxy threshold). Can run Parakeet TDT ONNX and Moonshine-tiny ONNX in-process without a Swift
  sidecar. Viable **only if you specifically want to avoid Swift and stay in Rust**, but it's CPU/
  generic-ONNX (no ANE) and gets you old Moonshine-tiny, so it's strictly worse than the FluidAudio
  ANE path for a Mac-only app. Repo: https://github.com/thewh1teagle/sherpa-rs

---

## Final recommendation

### Best transcription upgrade to implement first (meeting accuracy on M3 Pro)
**Parakeet (FluidAudio `AsrManager`, `version: .ultra` or `.v3`) running in a FluidAudio Swift
sidecar.** It's the only option that (a) beats your current large-v3-turbo on the Open ASR leaderboard,
(b) runs on the **ANE** (frees Metal/CPU, better battery during long recordings), (c) reuses the SDK
you **already bundle**, and (d) needs **zero new Rust crates** → no dependency-proxy risk.

### Shipping approach
Extend the existing FluidAudio Swift sidecar (or add a sibling `transcribe` sidecar) referenced by
Tauri `externalBin`. Have Rust invoke it exactly like `speaker-diarize`. Models auto-download from
`huggingface.co/FluidInference` into cache on first run (you already do HF model downloads). Keep
whisper.cpp as a fallback/alt provider during rollout. Start with a **stop-time full-file pass** via
`SlidingWindowAsrManager` (simplest, highest accuracy); optionally move to per-chunk later.

### Honest verdict on moonshine.ai
**Not worth adding as a quality default — but newly worth considering as an optional low-latency
live-caption mode.** The new **Medium Streaming (245M, 6.65% Open-ASR)** is a real improvement over the
tiny/base you previously rejected and is competitive with Whisper Large v3 *on clean speech*. However:
it's a sub-30 s streaming model, **CPU/ONNX on Mac (no ANE)**, has **no diarization**, and has **no
published meeting-corpus accuracy**. For the final, speaker-attributed meeting transcript it is the
wrong tool; Parakeet-via-FluidAudio wins on accuracy, ANE efficiency, and zero-new-dependency
shipping. If you ever want live captions *while recording*, **FluidAudio's Parakeet EOU (ANE,
in-SDK)** is a better fit than Moonshine because it stays on the one SDK and the ANE — so even the
live-caption niche doesn't strictly require Moonshine. Add Moonshine only if you specifically want its
cross-platform streaming UX; otherwise skip it.

### Prior-research check
The earlier conclusion ("Parakeet-via-FluidAudio is the real upgrade; Moonshine tiny/base is a
downgrade for meetings") **still holds**. Update only the Moonshine line: there is now a larger Medium
model that is no longer a downgrade *on clean speech*, but it remains a streaming/live engine, not a
meeting-accuracy default.
