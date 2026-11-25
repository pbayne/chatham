//! Make audio the import sidecar (AVFoundation) can't open into a file it can.
//!
//! Measured on macOS 26 through the real sidecar: MP3, AAC/M4A, ALAC, WAV,
//! AIFF, 16/24-bit FLAC and Ogg Vorbis/Opus all import natively. What fails is
//! the Matroska container (WebM/MKV/MKA) — "Couldn't open input file" — even
//! though the Opus inside it decodes fine. So:
//!
//! - WebM/MKV with Opus (nearly all of them): *remux* the packets into Ogg —
//!   no decode, no quality loss, seconds even for a long episode.
//! - WebM/MKV with anything else symphonia decodes (Vorbis, FLAC, PCM, AAC):
//!   decode to 16-bit PCM WAV at the source rate; the sidecar resamples.

use anyhow::{anyhow, bail, Context, Result};
use std::fs::File;
use std::io::{BufWriter, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use symphonia::core::audio::{SampleBuffer, SignalSpec};
use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_NULL};
use symphonia::core::errors::Error as SymErr;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

/// Extensions that must go through [`convert_for_import`] before import.
pub fn needs_conversion(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref(),
        Some("webm" | "mkv" | "mka")
    )
}

/// Produce an importable file from `src` inside `out_dir`: an Ogg remux for
/// Opus, a WAV decode for other codecs. Returns its path.
pub fn convert_for_import(src: &Path, out_dir: &Path) -> Result<PathBuf> {
    let stem = src.file_stem().and_then(|s| s.to_str()).unwrap_or("audio");
    match remux_opus_to_ogg(src, &out_dir.join(format!("{stem}.opus.ogg"))) {
        Ok(Some(p)) => Ok(p),
        Ok(None) => to_wav(src, &out_dir.join(format!("{stem}.import.wav"))),
        Err(e) => Err(e),
    }
}

fn open_format(src: &Path) -> Result<Box<dyn symphonia::core::formats::FormatReader>> {
    let file = File::open(src).with_context(|| format!("open {}", src.display()))?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = src.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    Ok(symphonia::default::get_probe()
        .format(&hint, mss, &FormatOptions::default(), &MetadataOptions::default())
        .map_err(|e| anyhow!("Couldn't read this audio file: {e}"))?
        .format)
}

/// Samples (at 48 kHz) in one Opus packet, from its TOC byte (RFC 6716 §3.1).
pub(crate) fn opus_packet_samples(packet: &[u8]) -> Option<u64> {
    let toc = *packet.first()?;
    let config = toc >> 3;
    // Frame size in 48 kHz samples, by config range: SILK 10/20/40/60 ms,
    // Hybrid 10/20 ms, CELT 2.5/5/10/20 ms.
    let frame: u64 = match config {
        0..=11 => [480, 960, 1920, 2880][(config % 4) as usize],
        12..=15 => [480, 960][(config % 2) as usize],
        _ => [120, 240, 480, 960][(config % 4) as usize],
    };
    let frames: u64 = match toc & 0x3 {
        0 => 1,
        1 | 2 => 2,
        _ => (*packet.get(1)? & 0x3f) as u64,
    };
    Some(frame * frames)
}

/// If `src`'s first audio track is Opus, copy its packets into an Ogg Opus
/// file (RFC 7845) at `dest` and return Some(dest); None for other codecs.
fn remux_opus_to_ogg(src: &Path, dest: &Path) -> Result<Option<PathBuf>> {
    use symphonia::core::codecs::CODEC_TYPE_OPUS;
    let mut format = open_format(src)?;
    let Some(track) = format.tracks().iter().find(|t| t.codec_params.codec != CODEC_TYPE_NULL) else {
        bail!("This file has no audio track");
    };
    if track.codec_params.codec != CODEC_TYPE_OPUS {
        return Ok(None);
    }
    let track_id = track.id;
    let channels = track.codec_params.channels.map(|c| c.count() as u8).unwrap_or(2);
    // Matroska carries the OpusHead as CodecPrivate; synthesise one if absent.
    let head: Vec<u8> = match track.codec_params.extra_data.as_deref() {
        Some(h) if h.starts_with(b"OpusHead") => h.to_vec(),
        _ => {
            let mut h = b"OpusHead".to_vec();
            h.push(1); // version
            h.push(channels);
            h.extend_from_slice(&312u16.to_le_bytes()); // pre-skip (libopus default)
            h.extend_from_slice(&48_000u32.to_le_bytes());
            h.extend_from_slice(&0i16.to_le_bytes()); // output gain
            h.push(0); // mapping family 0 (mono/stereo)
            h
        }
    };
    if head.len() >= 19 && head[18] != 0 && channels > 2 {
        // Surround mapping families need the mapping table carried over, which
        // CodecPrivate already includes; nothing extra to do.
    }

    let serial: u32 = 0x4348_4154; // "CHAT"
    let file = BufWriter::new(File::create(dest)?);
    let mut w = ogg::writing::PacketWriter::new(file);
    use ogg::writing::PacketWriteEndInfo::{EndPage, EndStream, NormalPacket};
    w.write_packet(head, serial, EndPage, 0)?;
    let mut tags = b"OpusTags".to_vec();
    let vendor = b"Chatham";
    tags.extend_from_slice(&(vendor.len() as u32).to_le_bytes());
    tags.extend_from_slice(vendor);
    tags.extend_from_slice(&0u32.to_le_bytes());
    w.write_packet(tags, serial, EndPage, 0)?;

    let mut granule: u64 = 0;
    let mut pending: Option<Vec<u8>> = None;
    let mut in_page = 0usize;
    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(SymErr::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(SymErr::ResetRequired) => break,
            Err(e) => return Err(anyhow!("read error: {e}")),
        };
        if packet.track_id() != track_id || packet.data.is_empty() {
            continue;
        }
        // Write the previous packet now that we know it isn't the last one.
        if let Some(prev) = pending.take() {
            in_page += 1;
            // ~1 s pages (50 × 20 ms) keep seeking granular and pages small.
            let end = if in_page >= 50 { in_page = 0; EndPage } else { NormalPacket };
            w.write_packet(prev, serial, end, granule)?;
        }
        granule += opus_packet_samples(&packet.data).unwrap_or(960);
        pending = Some(packet.data.to_vec());
    }
    match pending {
        Some(last) => w.write_packet(last, serial, EndStream, granule)?,
        None => bail!("No audio packets found in this file"),
    }
    Ok(Some(dest.to_path_buf()))
}

/// Decode `src` and write it as 16-bit PCM WAV at `dest`. Fails with a
/// plain-language error for codecs we can't decode.
pub fn to_wav(src: &Path, dest: &Path) -> Result<PathBuf> {
    let mut format = open_format(src)?;
    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
        .ok_or_else(|| anyhow!("This file has no audio track"))?;
    let track_id = track.id;
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|_| {
            anyhow!("This file's audio codec isn't supported — convert it to MP3 or M4A first")
        })?;

    let mut out: Option<WavWriter> = None;
    let mut buf: Option<SampleBuffer<i16>> = None;
    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(SymErr::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(SymErr::ResetRequired) => break,
            Err(e) => return Err(anyhow!("read error: {e}")),
        };
        if packet.track_id() != track_id {
            continue;
        }
        let decoded = match decoder.decode(&packet) {
            Ok(d) => d,
            // A corrupt packet is skipped, as players do; anything else is fatal.
            Err(SymErr::DecodeError(_)) => continue,
            Err(e) => return Err(anyhow!("decode error: {e}")),
        };
        let spec = *decoded.spec();
        if out.is_none() {
            out = Some(WavWriter::create(dest, spec)?);
        }
        let b = buf.get_or_insert_with(|| SampleBuffer::<i16>::new(decoded.capacity() as u64, spec));
        if b.capacity() < decoded.capacity() * spec.channels.count() {
            *b = SampleBuffer::<i16>::new(decoded.capacity() as u64, spec);
        }
        b.copy_interleaved_ref(decoded);
        out.as_mut().unwrap().write(b.samples())?;
    }
    match out {
        Some(w) => {
            w.finish()?;
            Ok(dest.to_path_buf())
        }
        None => bail!("No audio could be decoded from this file"),
    }
}

/// Minimal streaming RIFF/WAVE (PCM 16-bit) writer; sizes patched on finish.
struct WavWriter {
    w: BufWriter<File>,
    data_bytes: u64,
}

impl WavWriter {
    fn create(path: &Path, spec: SignalSpec) -> Result<Self> {
        let channels = spec.channels.count() as u16;
        let rate = spec.rate;
        let mut w = BufWriter::new(File::create(path)?);
        let block = channels as u32 * 2;
        w.write_all(b"RIFF")?;
        w.write_all(&0u32.to_le_bytes())?; // patched
        w.write_all(b"WAVEfmt ")?;
        w.write_all(&16u32.to_le_bytes())?;
        w.write_all(&1u16.to_le_bytes())?; // PCM
        w.write_all(&channels.to_le_bytes())?;
        w.write_all(&rate.to_le_bytes())?;
        w.write_all(&(rate * block).to_le_bytes())?;
        w.write_all(&(block as u16).to_le_bytes())?;
        w.write_all(&16u16.to_le_bytes())?;
        w.write_all(b"data")?;
        w.write_all(&0u32.to_le_bytes())?; // patched
        Ok(Self { w, data_bytes: 0 })
    }

    fn write(&mut self, samples: &[i16]) -> Result<()> {
        for s in samples {
            self.w.write_all(&s.to_le_bytes())?;
        }
        self.data_bytes += samples.len() as u64 * 2;
        // WAV sizes are u32: past 4 GB the header can't describe the file.
        if self.data_bytes > u32::MAX as u64 - 36 {
            bail!("Audio too long to convert (over 4 GB of PCM)");
        }
        Ok(())
    }

    fn finish(mut self) -> Result<()> {
        let data = self.data_bytes as u32;
        self.w.flush()?;
        let f = self.w.get_mut();
        f.seek(SeekFrom::Start(4))?;
        f.write_all(&(36 + data).to_le_bytes())?;
        f.seek(SeekFrom::Start(40))?;
        f.write_all(&data.to_le_bytes())?;
        f.flush()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Write a tiny 16-bit WAV, the one format we can produce without an
    /// encoder, and run it back through `to_wav`: proves probe → decode →
    /// writer round-trips samples exactly and the header is well formed.
    #[test]
    fn wav_round_trip_preserves_samples_and_header() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("tone.wav");
        let spec = SignalSpec::new(8000, symphonia::core::audio::Channels::FRONT_LEFT | symphonia::core::audio::Channels::FRONT_RIGHT);
        let mut w = WavWriter::create(&src, spec).unwrap();
        let samples: Vec<i16> = (0..8000 * 2).map(|i| ((i % 200) as i16 - 100) * 100).collect();
        w.write(&samples).unwrap();
        w.finish().unwrap();

        let out = convert_for_import(&src, dir.path()).unwrap();
        assert_eq!(out, dir.path().join("tone.import.wav"), "non-Opus decodes to WAV");
        let bytes = std::fs::read(&out).unwrap();
        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(&bytes[8..16], b"WAVEfmt ");
        assert_eq!(u16::from_le_bytes([bytes[22], bytes[23]]), 2, "channels");
        assert_eq!(u32::from_le_bytes(bytes[24..28].try_into().unwrap()), 8000, "rate");
        let data_len = u32::from_le_bytes(bytes[40..44].try_into().unwrap()) as usize;
        assert_eq!(data_len, samples.len() * 2);
        assert_eq!(u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize, 36 + data_len);
        let got: Vec<i16> = bytes[44..].chunks(2).map(|c| i16::from_le_bytes([c[0], c[1]])).collect();
        assert_eq!(got, samples);
    }

    #[test]
    fn routes_only_the_formats_avfoundation_cant_read() {
        for p in ["c.webm", "C.WEBM", "d.mkv", "e.mka"] {
            assert!(needs_conversion(Path::new(p)), "{p}");
        }
        // Measured: AVFoundation on macOS 26 reads these itself.
        for p in ["a.mp3", "b.m4a", "c.ogg", "d.opus", "e.wav", "f.flac", "g"] {
            assert!(!needs_conversion(Path::new(p)), "{p}");
        }
    }

    #[test]
    fn garbage_is_a_readable_error() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("x.webm");
        std::fs::write(&p, b"not audio at all").unwrap();
        let e = convert_for_import(&p, dir.path()).unwrap_err().to_string();
        assert!(e.contains("Couldn't read"), "{e}");
    }

    #[test]
    fn opus_toc_frame_counts() {
        assert_eq!(opus_packet_samples(&[(1 << 3) | 0]), Some(960), "SILK 20 ms, 1 frame");
        assert_eq!(opus_packet_samples(&[(31 << 3) | 0]), Some(960), "CELT 20 ms");
        assert_eq!(opus_packet_samples(&[(31 << 3) | 1]), Some(1920), "2 frames");
        assert_eq!(opus_packet_samples(&[(28 << 3) | 3, 4]), Some(480), "code 3: 4 × 2.5 ms");
        assert_eq!(opus_packet_samples(&[(3 << 3) | 0]), Some(2880), "SILK 60 ms");
        assert_eq!(opus_packet_samples(&[]), None);
    }

    /// Real-file check against fixtures made with ffmpeg; skipped when absent.
    /// CHATHAM_AUDIO_FIXTURES=dir cargo test --lib audio_convert -- --ignored
    #[test]
    #[ignore]
    fn converts_real_fixtures() {
        let Ok(dir) = std::env::var("CHATHAM_AUDIO_FIXTURES") else { return };
        let out_dir = tempfile::tempdir().unwrap();
        for name in ["t.webm", "v.webm", "t.mka"] {
            let p = Path::new(&dir).join(name);
            if !p.exists() {
                continue;
            }
            match convert_for_import(&p, out_dir.path()) {
                Ok(out) => {
                    let keep = Path::new(&dir).join(format!("converted-{}", out.file_name().unwrap().to_string_lossy()));
                    std::fs::copy(&out, &keep).unwrap();
                    println!("{name}: ok -> {} ({} bytes)", keep.display(), std::fs::metadata(&keep).unwrap().len())
                }
                Err(e) => println!("{name}: {e}"),
            }
        }
    }
}
