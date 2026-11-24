//! Game sounds for the Model Viewer: soundscripts (`scripts/game_sounds_manifest.txt`),
//! a WAV decoder (PCM / MS-ADPCM / MP3-in-WAV, the formats Source games ship), the mapping
//! from animation events to sounds, and playback through the default audio device.

use crate::formats::keyvalues::{self, Kv};
use crate::gamefs::GameFs;
use std::collections::HashMap;
use std::num::NonZero;

// ---------------------------------------------------------------------------
// Soundscripts
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct SoundEntry {
    pub name: String,
    /// wave paths relative to the game root, e.g. `sound/player/footsteps/concrete1.wav`
    pub waves: Vec<String>,
    pub volume: (f32, f32),
    pub pitch: (f32, f32),
}

#[derive(Default)]
pub struct SoundScripts {
    /// lower-case name -> entry
    pub entries: HashMap<String, SoundEntry>,
    pub files: usize,
}

/// Strip the soundscript prefix characters (`)` spatial stereo, `^` distance variant, `*`
/// streamed, `#` music, `@` omni, ...) and make a `sound/...` path.
pub fn wave_path(raw: &str) -> String {
    let s = raw.trim().trim_start_matches(['*', '#', '@', '>', '<', '^', ')', '}', '!', '?', '&', '~', '$', '(', '{']);
    let s = s.replace('\\', "/");
    let s = s.trim_start_matches('/');
    if s.to_ascii_lowercase().starts_with("sound/") { s.to_string() } else { format!("sound/{s}") }
}

/// The engine's `$gender` substitution in wave paths: "female" when the model's name says so,
/// otherwise "male".
pub fn expand_gender(wave: &str, model: &str) -> String {
    if !wave.contains("$gender") {
        return wave.to_string();
    }
    let g = if model.to_ascii_lowercase().contains("female") { "female" } else { "male" };
    wave.replace("$gender", g)
}

fn parse_range(v: Option<&str>, named: &[(&str, f32)], default: f32) -> (f32, f32) {
    let Some(v) = v else { return (default, default) };
    let one = |s: &str| -> f32 {
        let s = s.trim();
        named.iter().find(|(n, _)| n.eq_ignore_ascii_case(s)).map(|(_, x)| *x).or_else(|| s.parse().ok()).unwrap_or(default)
    };
    match v.split_once(',') {
        Some((a, b)) => {
            let (a, b) = (one(a), one(b));
            (a.min(b), a.max(b))
        }
        None => {
            let x = one(v);
            (x, x)
        }
    }
}

const VOLUMES: &[(&str, f32)] = &[("VOL_NORM", 1.0)];
const PITCHES: &[(&str, f32)] = &[("PITCH_NORM", 100.0), ("PITCH_LOW", 95.0), ("PITCH_HIGH", 120.0)];

impl SoundScripts {
    /// Parse one soundscript file's text into the table (later files don't override earlier ones,
    /// like the engine, which warns about duplicates and keeps the first).
    pub fn add_text(&mut self, text: &str) {
        for e in keyvalues::parse(text) {
            if e.children().is_empty() {
                continue;
            }
            let mut waves = Vec::new();
            let mut collect = |kv: &Kv| {
                for c in kv.children() {
                    if c.key.eq_ignore_ascii_case("wave") {
                        if let Some(w) = c.str() {
                            if !w.trim().is_empty() {
                                waves.push(wave_path(w));
                            }
                        }
                    }
                }
            };
            collect(&e);
            if let Some(r) = e.get("rndwave") {
                collect(r);
            }
            if waves.is_empty() {
                continue;
            }
            let key = e.key.to_ascii_lowercase();
            if self.entries.contains_key(&key) {
                continue;
            }
            let volume = parse_range(e.get_str("volume"), VOLUMES, 1.0);
            let pitch = parse_range(e.get_str("pitch"), PITCHES, 100.0);
            self.entries.insert(key, SoundEntry { name: e.key.clone(), waves, volume, pitch });
        }
        self.files += 1;
    }

    /// Load every soundscript listed in `scripts/game_sounds_manifest.txt`
    /// (or `scripts/game_sounds.txt` when a game has no manifest).
    pub fn load(fs: &GameFs) -> SoundScripts {
        let mut s = SoundScripts::default();
        let mut files: Vec<String> = Vec::new();
        if let Some(text) = fs.read("scripts/game_sounds_manifest.txt").map(|b| String::from_utf8_lossy(&b).into_owned()) {
            for top in keyvalues::parse(&text) {
                for c in top.children() {
                    if let Some(v) = c.str() {
                        let v = v.trim().replace('\\', "/");
                        if v.to_ascii_lowercase().ends_with(".txt") && !files.iter().any(|f| f.eq_ignore_ascii_case(&v)) {
                            files.push(v);
                        }
                    }
                }
            }
        }
        if files.is_empty() {
            files.push("scripts/game_sounds.txt".into());
        }
        for f in files {
            if let Some(b) = fs.read(&f) {
                s.add_text(&String::from_utf8_lossy(&b));
            }
        }
        s
    }

    pub fn get(&self, name: &str) -> Option<&SoundEntry> {
        self.entries.get(&name.trim().to_ascii_lowercase())
    }
}

// ---------------------------------------------------------------------------
// Animation events -> sounds
// ---------------------------------------------------------------------------

/// Named NPC events whose sound is hard-coded in the game's NPC code.
const NAMED_EVENT_SOUNDS: &[(&str, &[&str])] = &[
    ("AE_ZOMBIE_STEP_LEFT", &["Zombie.FootstepLeft"]),
    ("AE_ZOMBIE_STEP_RIGHT", &["Zombie.FootstepRight"]),
    ("AE_ZOMBIE_SCUFF_LEFT", &["Zombie.ScuffLeft"]),
    ("AE_ZOMBIE_SCUFF_RIGHT", &["Zombie.ScuffRight"]),
    ("AE_ZOMBIE_ATTACK_SCREAM", &["Zombie.Attack"]),
    ("AE_ZOMBIE_ALERTSOUND", &["Zombie.Alert"]),
    ("AE_ANTLION_WALK_FOOTSTEP", &["NPC_Antlion.Footstep"]),
    ("AE_ANTLION_FOOTSTEP_SOFT", &["NPC_Antlion.FootstepSoft"]),
    ("AE_ANTLION_FOOTSTEP_HEAVY", &["NPC_Antlion.FootstepHeavy"]),
    ("AE_ANTLION_MELEE1_SOUND", &["NPC_Antlion.MeleeAttackSingle"]),
    ("AE_ANTLION_MELEE2_SOUND", &["NPC_Antlion.MeleeAttackDouble"]),
];

/// Candidate sound names for an animation event, best first (empty when the event makes no
/// sound). Names are gamesounds (soundscript entries) or raw wave paths.
/// `running` picks the run variant of NPC footsteps.
pub fn event_sounds(event: i32, name: &str, options: &str, running: bool) -> Vec<String> {
    let opt = options.trim();
    let first = opt.split_whitespace().next().unwrap_or("").trim_matches('"').to_string();
    let side = |left: bool| if left { "Left" } else { "Right" };
    let upper = name.to_ascii_uppercase();
    if !name.is_empty() {
        if let Some((_, s)) = NAMED_EVENT_SOUNDS.iter().find(|(n, _)| upper == *n) {
            return s.iter().map(|x| x.to_string()).collect();
        }
        // AE_CL_PLAYSOUND, AE_SV_PLAYSOUND, AE_CL_EXCLUDE_PLAYER_SOUND, AE_CL_PLAYSOUND_ATTACHMENT, ...
        if upper.contains("SOUND") && !upper.contains("STOPSOUND") && !first.is_empty() {
            return vec![first];
        }
        if (upper.ends_with("LEFTFOOT") || upper.ends_with("RIGHTFOOT")) && !first.is_empty() {
            return vec![format!("{first}.Footstep{}", side(upper.ends_with("LEFTFOOT")))];
        }
        return Vec::new();
    }
    match event {
        // SCRIPT_EVENT_SOUND, SCRIPT_EVENT_SOUND_VOICE, CL_EVENT_SOUND
        1004 | 1008 | 5004 if !first.is_empty() => vec![first],
        // CL_EVENT_FOOTSTEP_LEFT / RIGHT: "<options>.(Run)Footstep<Side>", default NPC_CombineS
        6004 | 6005 => {
            let base = if first.is_empty() { "NPC_CombineS".to_string() } else { first };
            let s = side(event == 6004);
            if running { vec![format!("{base}.RunFootstep{s}"), format!("{base}.Footstep{s}")] } else { vec![format!("{base}.Footstep{s}"), format!("{base}.RunFootstep{s}")] }
        }
        // CL_EVENT_MFOOTSTEP_* (material footsteps): the surface's step sound; the viewer has
        // no ground, so use concrete
        6006 | 6008 => vec!["Concrete.StepLeft".into()],
        6007 | 6009 => vec!["Concrete.StepRight".into()],
        // TF2 player footstep ("left" / "right"): surface step sound
        7001 => vec![format!("Concrete.Step{}", side(!first.eq_ignore_ascii_case("right")))],
        _ => Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// WAV decoding
// ---------------------------------------------------------------------------

/// Decoded audio: interleaved f32 samples.
#[derive(Clone)]
pub struct Pcm {
    pub channels: u16,
    pub rate: u32,
    pub samples: Vec<f32>,
}

const ADPCM_ADAPT: [i32; 16] = [230, 230, 230, 230, 307, 409, 512, 614, 768, 614, 512, 409, 307, 230, 230, 230];
const ADPCM_COEF: [(i32, i32); 7] = [(256, 0), (512, -256), (0, 0), (192, 64), (240, 0), (460, -208), (392, -232)];

fn le16(b: &[u8], o: usize) -> Option<u16> {
    b.get(o..o + 2).map(|x| u16::from_le_bytes([x[0], x[1]]))
}
fn le32(b: &[u8], o: usize) -> Option<u32> {
    b.get(o..o + 4).map(|x| u32::from_le_bytes([x[0], x[1], x[2], x[3]]))
}

fn decode_ms_adpcm(data: &[u8], channels: usize, block_align: usize, coefs: &[(i32, i32)]) -> Vec<f32> {
    let mut out = Vec::new();
    if channels == 0 || block_align < 7 * channels {
        return out;
    }
    for block in data.chunks(block_align) {
        if block.len() < 7 * channels {
            break;
        }
        let mut pred = vec![0usize; channels];
        let mut delta = vec![0i32; channels];
        let mut s1 = vec![0i32; channels];
        let mut s2 = vec![0i32; channels];
        let mut o = 0;
        for c in 0..channels {
            pred[c] = (block[o] as usize).min(coefs.len().saturating_sub(1));
            o += 1;
        }
        for c in 0..channels {
            delta[c] = i16::from_le_bytes([block[o], block[o + 1]]) as i32;
            o += 2;
        }
        for c in 0..channels {
            s1[c] = i16::from_le_bytes([block[o], block[o + 1]]) as i32;
            o += 2;
        }
        for c in 0..channels {
            s2[c] = i16::from_le_bytes([block[o], block[o + 1]]) as i32;
            o += 2;
        }
        for c in 0..channels {
            out.push(s2[c] as f32 / 32768.0);
        }
        for c in 0..channels {
            out.push(s1[c] as f32 / 32768.0);
        }
        let mut c = 0usize;
        for &byte in &block[o..] {
            for nib in [byte >> 4, byte & 0x0f] {
                let (c1, c2) = coefs.get(pred[c]).copied().unwrap_or((256, 0));
                let signed = if nib >= 8 { nib as i32 - 16 } else { nib as i32 };
                let p = ((s1[c] * c1 + s2[c] * c2) >> 8) + signed * delta[c];
                let p = p.clamp(-32768, 32767);
                s2[c] = s1[c];
                s1[c] = p;
                delta[c] = ((ADPCM_ADAPT[nib as usize] * delta[c]) >> 8).max(16);
                out.push(p as f32 / 32768.0);
                c = (c + 1) % channels;
            }
        }
    }
    out
}

/// Decode a RIFF WAVE file (PCM 8/16/24/32-bit, float, MS-ADPCM; MP3-in-WAV via `mp3`).
pub fn decode_wav(b: &[u8]) -> Result<Pcm, String> {
    if b.len() < 12 || &b[0..4] != b"RIFF" || &b[8..12] != b"WAVE" {
        return Err("not a RIFF WAVE file".into());
    }
    let mut o = 12;
    let mut fmt: Option<&[u8]> = None;
    let mut data: Option<&[u8]> = None;
    while o + 8 <= b.len() {
        let id = &b[o..o + 4];
        let size = le32(b, o + 4).unwrap_or(0) as usize;
        let start = o + 8;
        // Valve files sometimes declare more data than the file holds
        let end = start.saturating_add(size).min(b.len());
        match id {
            b"fmt " => fmt = Some(&b[start..end]),
            b"data" => data = Some(&b[start..end]),
            _ => {}
        }
        o = start.saturating_add(size).saturating_add(size & 1);
    }
    let fmt = fmt.ok_or("no fmt chunk")?;
    let data = data.ok_or("no data chunk")?;
    let tag = le16(fmt, 0).ok_or("short fmt chunk")?;
    let channels = le16(fmt, 2).unwrap_or(1).max(1);
    let rate = le32(fmt, 4).unwrap_or(22050).max(1);
    let block_align = le16(fmt, 12).unwrap_or(0) as usize;
    let bits = le16(fmt, 14).unwrap_or(16);
    // WAVE_FORMAT_EXTENSIBLE: the real tag is the first 2 bytes of the sub-format GUID
    let tag = if tag == 0xFFFE { le16(fmt, 24).unwrap_or(1) } else { tag };
    let samples: Vec<f32> = match (tag, bits) {
        (1, 8) => data.iter().map(|&x| (x as f32 - 128.0) / 128.0).collect(),
        (1, 16) => data.chunks_exact(2).map(|x| i16::from_le_bytes([x[0], x[1]]) as f32 / 32768.0).collect(),
        (1, 24) => data.chunks_exact(3).map(|x| (((x[0] as i32) << 8 | (x[1] as i32) << 16 | (x[2] as i32) << 24) >> 8) as f32 / 8_388_608.0).collect(),
        (1, 32) => data.chunks_exact(4).map(|x| i32::from_le_bytes([x[0], x[1], x[2], x[3]]) as f32 / 2_147_483_648.0).collect(),
        (3, 32) => data.chunks_exact(4).map(|x| f32::from_le_bytes([x[0], x[1], x[2], x[3]])).collect(),
        (2, _) => {
            let mut coefs: Vec<(i32, i32)> = Vec::new();
            if let Some(n) = le16(fmt, 20) {
                for i in 0..n as usize {
                    match (le16(fmt, 22 + i * 4), le16(fmt, 24 + i * 4)) {
                        (Some(a), Some(c)) => coefs.push((a as i16 as i32, c as i16 as i32)),
                        _ => break,
                    }
                }
            }
            if coefs.is_empty() {
                coefs = ADPCM_COEF.to_vec();
            }
            decode_ms_adpcm(data, channels as usize, block_align.max(7 * channels as usize), &coefs)
        }
        (0x55, _) => return decode_mp3(data),
        _ => return Err(format!("unsupported WAV format {tag:#x} ({bits}-bit)")),
    };
    Ok(Pcm { channels, rate, samples })
}

/// Decode MP3 data (a `.mp3` file or the data chunk of an MP3-in-WAV).
pub fn decode_mp3(bytes: &[u8]) -> Result<Pcm, String> {
    use rodio::Source;
    let dec = rodio::Decoder::new(std::io::Cursor::new(bytes.to_vec())).map_err(|e| e.to_string())?;
    let channels = dec.channels().get();
    let rate = dec.sample_rate().get();
    let samples: Vec<f32> = dec.map(|s| s as f32).collect();
    Ok(Pcm { channels, rate, samples })
}

pub fn decode(path: &str, bytes: &[u8]) -> Result<Pcm, String> {
    if path.to_ascii_lowercase().ends_with(".mp3") { decode_mp3(bytes) } else { decode_wav(bytes) }
}

// ---------------------------------------------------------------------------
// Playback
// ---------------------------------------------------------------------------

/// Plays game sounds on the default output device (opened on first use).
#[derive(Default)]
pub struct Player {
    sink: Option<rodio::MixerDeviceSink>,
    failed: Option<String>,
    cache: HashMap<String, Option<Pcm>>,
    rng: u64,
}

/// Result of resolving a sound name.
pub enum Resolved {
    Script(SoundEntry),
    Wave(String),
}

/// Find a sound: a soundscript entry first, then a raw wave path.
pub fn resolve(name: &str, scripts: Option<&SoundScripts>, fs: &GameFs) -> Option<Resolved> {
    if let Some(e) = scripts.and_then(|s| s.get(name)) {
        return Some(Resolved::Script(e.clone()));
    }
    let p = wave_path(name);
    for cand in [p.clone(), format!("{p}.wav")] {
        if fs.exists(&cand) {
            return Some(Resolved::Wave(cand));
        }
    }
    None
}

impl Player {
    fn rand(&mut self) -> f32 {
        if self.rng == 0 {
            self.rng = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(0x9E37_79B9_7F4A_7C15) | 1;
        }
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        (self.rng >> 40) as f32 / (1u64 << 24) as f32
    }

    /// Why audio is unavailable, if opening the device failed.
    pub fn error(&self) -> Option<&str> {
        self.failed.as_deref()
    }

    fn load(&mut self, fs: &GameFs, path: &str) -> Option<Pcm> {
        let key = path.to_ascii_lowercase();
        if let Some(c) = self.cache.get(&key) {
            return c.clone();
        }
        let pcm = fs.read(path).and_then(|b| decode(path, &b).ok());
        if self.cache.len() > 256 {
            self.cache.clear();
        }
        self.cache.insert(key, pcm.clone());
        pcm
    }

    /// Play a gamesound or wave for `model` (used for `$gender`). Returns the wave played.
    pub fn play(&mut self, name: &str, scripts: Option<&SoundScripts>, fs: &GameFs, model: &str, master_volume: f32) -> Option<String> {
        let (wave, volume, pitch) = match resolve(name, scripts, fs)? {
            Resolved::Script(e) => {
                let w = e.waves[((self.rand() * e.waves.len() as f32) as usize).min(e.waves.len() - 1)].clone();
                let v = e.volume.0 + (e.volume.1 - e.volume.0) * self.rand();
                let p = e.pitch.0 + (e.pitch.1 - e.pitch.0) * self.rand();
                (w, v, p)
            }
            Resolved::Wave(w) => (w, 1.0, 100.0),
        };
        let wave = expand_gender(&wave, model);
        let pcm = self.load(fs, &wave)?;
        self.play_pcm(&pcm, volume * master_volume, pitch / 100.0);
        Some(wave)
    }

    pub fn play_pcm(&mut self, pcm: &Pcm, volume: f32, speed: f32) {
        use rodio::Source;
        if self.sink.is_none() && self.failed.is_none() {
            match rodio::DeviceSinkBuilder::open_default_sink() {
                Ok(mut s) => {
                    s.log_on_drop(false);
                    self.sink = Some(s);
                }
                Err(e) => self.failed = Some(e.to_string()),
            }
        }
        let Some(sink) = &self.sink else { return };
        let (Some(ch), Some(rate)) = (NonZero::new(pcm.channels), NonZero::new(pcm.rate)) else { return };
        if pcm.samples.is_empty() {
            return;
        }
        let buf = rodio::buffer::SamplesBuffer::new(ch, rate, pcm.samples.iter().map(|&s| s as rodio::Sample).collect::<Vec<_>>());
        sink.mixer().add(buf.speed(speed.clamp(0.25, 4.0)).amplify(volume.clamp(0.0, 4.0) as rodio::Float));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn soundscript_entries() {
        let mut s = SoundScripts::default();
        s.add_text(
            r#"
"NPC_Citizen.FootstepLeft"
{
	"channel"	"CHAN_BODY"
	"volume"	"0.5, 0.6"
	"pitch"		"PITCH_NORM"
	"rndwave"
	{
		"wave"	")npc/footsteps/hardboot_generic1.wav"
		"wave"	"^npc\footsteps\hardboot_generic2.wav"
	}
}
"Concrete.StepLeft" { "volume" "VOL_NORM" "pitch" "95,105" "wave" "player/footsteps/concrete1.wav" }
"#,
        );
        let e = s.get("npc_citizen.footstepleft").unwrap();
        assert_eq!(e.waves, vec!["sound/npc/footsteps/hardboot_generic1.wav", "sound/npc/footsteps/hardboot_generic2.wav"]);
        assert_eq!(e.volume, (0.5, 0.6));
        assert_eq!(e.pitch, (100.0, 100.0));
        let c = s.get("Concrete.StepLeft").unwrap();
        assert_eq!(c.pitch, (95.0, 105.0));
        assert_eq!(c.volume, (1.0, 1.0));
    }

    #[test]
    fn events_map_to_sounds() {
        assert_eq!(event_sounds(0, "AE_CL_PLAYSOUND", "NPC_Combine.Zipline_Start", false), vec!["NPC_Combine.Zipline_Start"]);
        assert_eq!(event_sounds(6004, "", "NPC_Citizen", true)[0], "NPC_Citizen.RunFootstepLeft");
        assert_eq!(event_sounds(6005, "", "", false)[0], "NPC_CombineS.FootstepRight");
        assert_eq!(event_sounds(7001, "", "right", false), vec!["Concrete.StepRight"]);
        assert_eq!(event_sounds(5004, "", "taunt_screams", false), vec!["taunt_screams"]);
        assert!(event_sounds(0, "AE_WPN_HIDE", "", false).is_empty());
        assert!(event_sounds(1003, "", "1", false).is_empty());
    }

    fn wav(tag: u16, ch: u16, rate: u32, align: u16, bits: u16, extra: &[u8], data: &[u8]) -> Vec<u8> {
        let mut fmt = Vec::new();
        fmt.extend_from_slice(&tag.to_le_bytes());
        fmt.extend_from_slice(&ch.to_le_bytes());
        fmt.extend_from_slice(&rate.to_le_bytes());
        fmt.extend_from_slice(&(rate * align as u32).to_le_bytes());
        fmt.extend_from_slice(&align.to_le_bytes());
        fmt.extend_from_slice(&bits.to_le_bytes());
        fmt.extend_from_slice(extra);
        let mut b = b"RIFF\0\0\0\0WAVE".to_vec();
        b.extend_from_slice(b"fmt ");
        b.extend_from_slice(&(fmt.len() as u32).to_le_bytes());
        b.extend_from_slice(&fmt);
        b.extend_from_slice(b"cue \x04\0\0\0abcd");
        b.extend_from_slice(b"data");
        b.extend_from_slice(&(data.len() as u32).to_le_bytes());
        b.extend_from_slice(data);
        b
    }

    #[test]
    fn decodes_pcm16_and_adpcm() {
        let data: Vec<u8> = [0i16, 16384, -16384, 32767].iter().flat_map(|s| s.to_le_bytes()).collect();
        let p = decode_wav(&wav(1, 1, 22050, 2, 16, &[], &data)).unwrap();
        assert_eq!(p.samples.len(), 4);
        assert!((p.samples[1] - 0.5).abs() < 1e-4);
        // one mono MS-ADPCM block: header (pred 0, delta 16, s1 100, s2 50) + zero nibbles
        let mut blk = vec![0u8];
        for v in [16i16, 100, 50] {
            blk.extend_from_slice(&v.to_le_bytes());
        }
        blk.extend_from_slice(&[0, 0]);
        let mut extra = Vec::new();
        extra.extend_from_slice(&32u16.to_le_bytes()); // cbSize
        extra.extend_from_slice(&6u16.to_le_bytes()); // samples per block
        extra.extend_from_slice(&7u16.to_le_bytes());
        for (a, c) in ADPCM_COEF {
            extra.extend_from_slice(&(a as i16).to_le_bytes());
            extra.extend_from_slice(&(c as i16).to_le_bytes());
        }
        let p = decode_wav(&wav(2, 1, 22050, blk.len() as u16, 4, &extra, &blk)).unwrap();
        // s2, s1, then predictions with coef (256, 0): repeat s1
        assert_eq!(p.samples.len(), 6);
        assert!((p.samples[0] * 32768.0 - 50.0).abs() < 0.5);
        assert!((p.samples[5] * 32768.0 - 100.0).abs() < 0.5);
    }
}
