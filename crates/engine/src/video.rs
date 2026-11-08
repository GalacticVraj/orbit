//! Video actions through the Media Foundation transcode API (H.264 / AAC in MP4). ffmpeg on PATH is the fallback
//! for sources Media Foundation can't open.

use crate::{files, name, saved};
use orbit_core::{output_path, Outcome, Progress};
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;
use windows::core::{Interface, Result, GUID, HSTRING, PROPVARIANT};
use windows::Win32::Foundation::BOOL;
use windows::Win32::Media::MediaFoundation::*;

#[derive(Clone, Copy, PartialEq)]
pub enum Mode {
    Audio,
    Compress,
    Mp4,
}

pub fn run(src: &Path, mode: Mode, progress: Progress) -> std::result::Result<Outcome, String> {
    let out = match mode {
        Mode::Audio => output_path(src, "", "m4a"),
        Mode::Compress => output_path(src, "_compressed", "mp4"),
        Mode::Mp4 => output_path(src, "", "mp4"),
    };
    let _mf = Mf::start().map_err(|_| "Media Foundation isn't available.".to_string())?;
    let result = match unsafe { open(src) } {
        Ok(source) => unsafe {
            let r = transcode(&source, src, &out, mode, progress);
            let _ = source.Shutdown();
            r.map_err(|e| why(e, mode))
        },
        Err(_) => ffmpeg(src, &out, mode).ok_or_else(|| format!("{} isn't a video Windows can read.", name(src))),
    };
    if let Err(e) = result {
        let _ = std::fs::remove_file(&out);
        return Err(e);
    }
    let msg = match mode {
        Mode::Audio => "Audio extracted".to_string(),
        Mode::Mp4 => "Saved as MP4".to_string(),
        Mode::Compress => saved(files::len(src), files::len(&out)),
    };
    Ok(Outcome::new(msg, vec![out]))
}

fn why(e: String, mode: Mode) -> String {
    match (e.as_str(), mode) {
        ("no audio", _) => "This video has no audio track.".into(),
        ("no video", _) => "This file has no video track.".into(),
        _ => format!("The conversion failed ({e})."),
    }
}

/// MFStartup / MFShutdown pair.
struct Mf;
impl Mf {
    fn start() -> Result<Mf> {
        unsafe { MFStartup(MF_VERSION, MFSTARTUP_FULL) }.map(|_| Mf)
    }
}
impl Drop for Mf {
    fn drop(&mut self) {
        let _ = unsafe { MFShutdown() };
    }
}

unsafe fn open(path: &Path) -> Result<IMFMediaSource> {
    let resolver = MFCreateSourceResolver()?;
    let (mut kind, mut obj) = (MF_OBJECT_TYPE::default(), None);
    let flags = (MF_RESOLUTION_MEDIASOURCE.0 | MF_RESOLUTION_READ.0) as u32;
    resolver.CreateObjectFromURL(&HSTRING::from(path.as_os_str()), flags, None, &mut kind, &mut obj)?;
    obj.ok_or_else(|| windows::core::Error::from(windows::Win32::Foundation::E_FAIL))?.cast()
}

fn hi_lo(v: u64) -> (u32, u32) {
    ((v >> 32) as u32, v as u32)
}

unsafe fn attrs() -> Result<IMFAttributes> {
    let mut a = None;
    MFCreateAttributes(&mut a, 8)?;
    Ok(a.unwrap())
}

/// Stereo AAC, preferring 48 kHz / 192 kbps among what the installed encoder offers.
unsafe fn aac() -> Result<IMFAttributes> {
    let types = MFTranscodeGetAudioOutputAvailableTypes(&MFAudioFormat_AAC, MFT_ENUM_FLAG_ALL.0 as u32, None)?;
    let mut best: Option<(u32, IMFMediaType)> = None;
    for i in 0..types.GetElementCount()? {
        let t: IMFMediaType = types.GetElement(i)?.cast()?;
        let ch = t.GetUINT32(&MF_MT_AUDIO_NUM_CHANNELS).unwrap_or(0);
        let hz = t.GetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND).unwrap_or(0);
        let bps = t.GetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND).unwrap_or(0);
        let score = (ch == 2) as u32 * 4 + (hz == 48000) as u32 * 2 + (bps == 24000) as u32;
        if best.as_ref().map_or(true, |(s, _)| score > *s) {
            best = Some((score, t));
        }
    }
    let a = attrs()?;
    best.ok_or_else(|| windows::core::Error::from(windows::Win32::Foundation::E_FAIL))?.1.CopyAllItems(&a)?;
    Ok(a)
}

/// H.264 settings derived from the source stream: Compress = ~55-60% bitrate and ≤1080p, Mp4 = same size and bitrate.
/// `video_bps`: fallback estimate when the stream doesn't state its bitrate.
unsafe fn h264(src: &IMFMediaType, mode: Mode, video_bps: u64) -> Result<IMFAttributes> {
    let (w, h) = hi_lo(src.GetUINT64(&MF_MT_FRAME_SIZE)?);
    let (num, den) = hi_lo(src.GetUINT64(&MF_MT_FRAME_RATE).unwrap_or(30 << 32 | 1));
    let fps = if num > 0 && den > 0 { num as f64 / den as f64 } else { 30.0 };
    let scale = if mode == Mode::Compress { (1080.0 / w.min(h).max(1) as f64).min(1.0) } else { 1.0 };
    let even = |v: u32| ((v as f64 * scale) as u32 & !1).max(2);
    let (tw, th) = (even(w), even(h));
    let source_bps = src.GetUINT32(&MF_MT_AVG_BITRATE).map(u64::from).unwrap_or(video_bps);
    let ceiling = (tw as f64 * th as f64 * fps * 0.15) as u64; // keeps raw/intra sources sane
    // Calibration: Windows' H.264 encoder overshoots its target ~25% (2.81 -> 1.95 Mbps asked 1.55), so ask 0.45 to land ~55-60%.
    let factor = if mode == Mode::Compress { 0.45 } else { 1.0 };
    let bps = ((source_bps as f64 * factor) as u64).min(ceiling).max(300_000) as u32;
    let a = attrs()?;
    a.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264)?;
    a.SetUINT64(&MF_MT_FRAME_SIZE, (tw as u64) << 32 | th as u64)?;
    a.SetUINT64(&MF_MT_FRAME_RATE, (num.max(1) as u64) << 32 | den.max(1) as u64)?;
    a.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, 1 << 32 | 1)?;
    a.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
    a.SetUINT32(&MF_MT_AVG_BITRATE, bps)?;
    Ok(a)
}

unsafe fn transcode(source: &IMFMediaSource, src: &Path, out: &Path, mode: Mode, progress: Progress) -> std::result::Result<(), String> {
    let hr = |e: windows::core::Error| e.message().trim().to_string();
    let pd = source.CreatePresentationDescriptor().map_err(hr)?;
    let duration = pd.GetUINT64(&MF_PD_DURATION).unwrap_or(0); // 100 ns units
    let (mut video, mut audio) = (None, false);
    for i in 0..pd.GetStreamDescriptorCount().map_err(hr)? {
        let (mut selected, mut sd) = (BOOL(0), None);
        pd.GetStreamDescriptorByIndex(i, &mut selected, &mut sd).map_err(hr)?;
        let Some(handler) = sd.and_then(|s| s.GetMediaTypeHandler().ok()) else { continue };
        match handler.GetMajorType() {
            Ok(t) if t == MFMediaType_Video && video.is_none() => video = handler.GetCurrentMediaType().ok(),
            Ok(t) if t == MFMediaType_Audio => audio = true,
            _ => {}
        }
    }
    let profile = MFCreateTranscodeProfile().map_err(hr)?;
    if audio {
        profile.SetAudioAttributes(&aac().map_err(hr)?).map_err(hr)?;
    } else if mode == Mode::Audio {
        return Err("no audio".into());
    }
    if mode != Mode::Audio {
        let v = video.ok_or("no video")?;
        let secs = (duration as f64 / 1e7).max(0.1);
        let total_bps = (files::len(src) as f64 * 8.0 / secs) as u64;
        let video_bps = total_bps.saturating_sub(if audio { 128_000 } else { 0 });
        profile.SetVideoAttributes(&h264(&v, mode, video_bps).map_err(hr)?).map_err(hr)?;
    }
    let container = attrs().map_err(hr)?;
    container.SetGUID(&MF_TRANSCODE_CONTAINERTYPE, &MFTranscodeContainerType_MPEG4).map_err(hr)?;
    profile.SetContainerAttributes(&container).map_err(hr)?;

    let topology = MFCreateTranscodeTopology(source, &HSTRING::from(out.as_os_str()), &profile).map_err(hr)?;
    let session = MFCreateMediaSession(None).map_err(hr)?;
    session.SetTopology(0, &topology).map_err(hr)?;
    session.Start(&GUID::zeroed(), &PROPVARIANT::default()).map_err(hr)?;
    let result = pump(&session, MESessionEnded, |s| {
        if duration > 0 {
            if let Ok(t) = s.GetClock().and_then(|c| c.cast::<IMFPresentationClock>()).and_then(|c| c.GetTime()) {
                progress((t as f64 / duration as f64).clamp(0.0, 1.0) as f32);
            }
        }
    });
    // Close finalises the MP4 (writes the moov box); wait for it before releasing anything.
    if session.Close().is_ok() {
        let _ = pump(&session, MESessionClosed, |_| {});
    }
    let _ = session.Shutdown();
    result.map_err(hr)
}

/// Polls session events every 250 ms until `until` arrives (or an error), calling `tick` in between.
unsafe fn pump(session: &IMFMediaSession, until: MF_EVENT_TYPE, tick: impl Fn(&IMFMediaSession)) -> Result<()> {
    loop {
        match session.GetEvent(MF_EVENT_FLAG_NO_WAIT) {
            Ok(ev) => {
                ev.GetStatus()?.ok()?;
                let t = ev.GetType()?;
                if t == until.0 as u32 {
                    return Ok(());
                }
                if t == MEError.0 as u32 {
                    return Err(windows::Win32::Foundation::E_FAIL.into());
                }
                continue;
            }
            Err(e) if e.code() == MF_E_NO_EVENTS_AVAILABLE => {}
            Err(e) => return Err(e),
        }
        tick(session);
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// Fallback for formats Media Foundation can't open. Returns None if ffmpeg is missing or fails.
fn ffmpeg(src: &Path, out: &Path, mode: Mode) -> Option<()> {
    let codec: &[&str] = match mode {
        Mode::Audio => &["-vn", "-c:a", "aac", "-b:a", "192k"],
        Mode::Compress => &[
            "-vf",
            "scale='if(gte(iw,ih),-2,min(1080,iw))':'if(gte(iw,ih),min(1080,ih),-2)'",
            "-c:v", "libx264", "-crf", "26", "-preset", "medium", "-c:a", "aac", "-b:a", "128k",
        ],
        Mode::Mp4 => &["-c:v", "libx264", "-crf", "20", "-preset", "medium", "-c:a", "aac", "-b:a", "192k"],
    };
    let status = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-n", "-i"])
        .arg(src)
        .args(codec)
        .arg(out)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(0x0800_0000)
        .status()
        .ok()?;
    status.success().then_some(())
}
