//! Turning Jellyfin's session / media-stream JSON into the flat columns finstats stores.
//! Shared by the live collector and the Jellystat importer so both produce identical rows.

use serde_json::{Value, json};

#[derive(Debug, Default, Clone)]
pub struct Streams {
    pub bitrate: Option<i64>,
    pub video_codec: Option<String>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub video_range: Option<String>,
    pub bit_depth: Option<i64>,
    pub audio_codec: Option<String>,
    pub audio_channels: Option<i64>,
    pub audio_language: Option<String>,
    pub subtitle_codec: Option<String>,
    pub subtitle_language: Option<String>,
}

fn s(v: &Value) -> Option<String> {
    v.as_str().map(str::trim).filter(|x| !x.is_empty()).map(str::to_string)
}

/// Accepts numbers and numeric strings (Jellystat stores bigints as strings).
pub fn int(v: &Value) -> Option<i64> {
    match v {
        Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)),
        Value::String(x) => x.trim().parse::<i64>().ok().or_else(|| x.trim().parse::<f64>().ok().map(|f| f as i64)),
        _ => None,
    }
}

pub fn ticks_to_s(v: &Value) -> Option<i64> {
    int(v).map(|t| t / 10_000_000)
}

impl Streams {
    /// `media_streams` is a Jellyfin `MediaStreams` array; `play_state` selects the
    /// audio/subtitle track actually in use (falls back to the default track).
    pub fn extract(media_streams: &Value, play_state: &Value) -> Self {
        let mut out = Streams::default();
        let Some(streams) = media_streams.as_array() else { return out };
        let audio_idx = play_state["AudioStreamIndex"].as_i64();
        let sub_idx = play_state["SubtitleStreamIndex"].as_i64().filter(|i| *i >= 0);

        let of_type = |t: &'static str| streams.iter().filter(move |x| x["Type"].as_str() == Some(t));

        if let Some(v) = of_type("Video").next() {
            out.video_codec = s(&v["Codec"]).map(|c| c.to_lowercase());
            out.width = int(&v["Width"]);
            out.height = int(&v["Height"]);
            out.bit_depth = int(&v["BitDepth"]);
            out.video_range = s(&v["VideoRangeType"]).or_else(|| s(&v["VideoRange"]));
        }
        let audio = audio_idx
            .and_then(|i| of_type("Audio").find(|a| a["Index"].as_i64() == Some(i)))
            .or_else(|| of_type("Audio").find(|a| a["IsDefault"].as_bool() == Some(true)))
            .or_else(|| of_type("Audio").next());
        if let Some(a) = audio {
            out.audio_codec = s(&a["Codec"]).map(|c| c.to_lowercase());
            out.audio_channels = int(&a["Channels"]);
            out.audio_language = s(&a["Language"]);
        }
        if let Some(sub) = sub_idx.and_then(|i| of_type("Subtitle").find(|x| x["Index"].as_i64() == Some(i))) {
            out.subtitle_codec = s(&sub["Codec"]).map(|c| c.to_lowercase());
            out.subtitle_language = s(&sub["Language"]).or(Some("und".into()));
        }
        let total: i64 = streams.iter().filter_map(|x| int(&x["BitRate"])).sum();
        out.bitrate = (total > 0).then_some(total);
        out
    }

    pub fn has_video(&self) -> bool {
        self.video_codec.is_some()
    }
}

/// The languages of every track of one kind ("Audio", "Subtitle") as a JSON array, each once, in track
/// order: `["jpn","eng"]`. A track without a language counts as "und", because "there is a second audio
/// track, nobody knows in what" still answers "is there a dub?". `None` when there is no such track.
pub fn track_languages(media_streams: &Value, kind: &str) -> Option<String> {
    let mut out: Vec<String> = vec![];
    for t in media_streams.as_array()?.iter().filter(|t| t["Type"].as_str() == Some(kind)) {
        let code = t["Language"].as_str().map(|l| l.trim().to_ascii_lowercase()).filter(|l| !l.is_empty() && l.len() <= 12 && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')).unwrap_or_else(|| "und".into());
        if !out.contains(&code) {
            out.push(code);
        }
    }
    (!out.is_empty()).then(|| serde_json::json!(out).to_string())
}

/// Keep only the useful parts of Jellyfin's `TranscodingInfo`.
pub fn compact_transcode(t: &Value) -> Option<Value> {
    if !t.is_object() {
        return None;
    }
    Some(json!({
        "video_codec": t["VideoCodec"],
        "audio_codec": t["AudioCodec"],
        "container": t["Container"],
        "is_video_direct": t["IsVideoDirect"].as_bool().unwrap_or(false),
        "is_audio_direct": t["IsAudioDirect"].as_bool().unwrap_or(false),
        "hw_accel": s(&t["HardwareAccelerationType"]).filter(|h| !h.eq_ignore_ascii_case("none")),
        "reasons": normalize_reasons(&t["TranscodeReasons"]),
        "bitrate": t["Bitrate"],
        "width": t["Width"],
        "height": t["Height"],
        "audio_channels": t["AudioChannels"],
    }))
}

/// `TranscodeReasons` is an array on current servers and a comma separated string on old ones.
fn normalize_reasons(v: &Value) -> Vec<String> {
    match v {
        Value::Array(a) => a.iter().filter_map(|x| x.as_str()).map(str::to_string).collect(),
        Value::String(x) => x.split(',').map(str::trim).filter(|r| !r.is_empty()).map(str::to_string).collect(),
        _ => vec![],
    }
}

/// Jellyfin reports "Transcode" for plain remuxes too; call those what they are.
pub fn effective_play_method(reported: Option<&str>, transcode: Option<&Value>) -> String {
    let reported = reported.unwrap_or("DirectPlay");
    if reported == "Transcode" {
        if let Some(t) = transcode {
            if t["is_video_direct"].as_bool() == Some(true) && t["is_audio_direct"].as_bool() == Some(true) {
                return "DirectStream".into();
            }
        }
    }
    reported.to_string()
}

pub fn resolution_label(width: Option<i64>, height: Option<i64>) -> Option<&'static str> {
    let (w, h) = (width.unwrap_or(0), height.unwrap_or(0));
    if w == 0 && h == 0 {
        return None;
    }
    // Width is the reliable dimension: scope films are 1920x800 but still "1080p".
    Some(if w >= 3800 || h >= 2000 {
        "4K"
    } else if w >= 2500 || h >= 1400 {
        "1440p"
    } else if w >= 1900 || h >= 1000 {
        "1080p"
    } else if w >= 1260 || h >= 700 {
        "720p"
    } else if w >= 1000 || h >= 560 {
        "576p"
    } else if w >= 700 || h >= 400 {
        "480p"
    } else {
        "SD"
    })
}

fn channels_label(ch: i64) -> String {
    match ch {
        1 => "1.0".into(),
        2 => "2.0".into(),
        6 => "5.1".into(),
        8 => "7.1".into(),
        n => format!("{n}ch"),
    }
}

/// "HEVC 1080p HDR10"
pub fn video_label(codec: Option<&str>, width: Option<i64>, height: Option<i64>, range: Option<&str>) -> Option<String> {
    let codec = codec?;
    let mut out = codec.to_uppercase();
    if let Some(r) = resolution_label(width, height) {
        out.push(' ');
        out.push_str(r);
    }
    if let Some(r) = range.filter(|r| !r.is_empty()) {
        out.push(' ');
        out.push_str(r);
    }
    Some(out)
}

/// "EAC3 5.1 eng"
pub fn audio_label(codec: Option<&str>, channels: Option<i64>, language: Option<&str>) -> Option<String> {
    let codec = codec?;
    let mut out = codec.to_uppercase();
    if let Some(c) = channels.filter(|c| *c > 0) {
        out.push(' ');
        out.push_str(&channels_label(c));
    }
    if let Some(l) = language.filter(|l| !l.is_empty()) {
        out.push(' ');
        out.push_str(l);
    }
    Some(out)
}

pub fn subtitle_label(codec: Option<&str>, language: Option<&str>) -> Option<String> {
    let language = language?;
    Some(match codec {
        Some(c) => format!("{language} ({c})"),
        None => language.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_selected_tracks() {
        let streams = json!([
            {"Type": "Video", "Codec": "HEVC", "Width": 1920, "Height": 800, "VideoRangeType": "HDR10", "BitRate": 5000000, "Index": 0},
            {"Type": "Audio", "Codec": "aac", "Channels": 2, "Language": "eng", "IsDefault": true, "Index": 1, "BitRate": 128000},
            {"Type": "Audio", "Codec": "eac3", "Channels": 6, "Language": "jpn", "Index": 2},
            {"Type": "Subtitle", "Codec": "subrip", "Language": "eng", "Index": 3}
        ]);
        let st = Streams::extract(&streams, &json!({"AudioStreamIndex": 2, "SubtitleStreamIndex": 3}));
        assert_eq!(st.video_codec.as_deref(), Some("hevc"));
        assert_eq!(st.audio_codec.as_deref(), Some("eac3"));
        assert_eq!(st.audio_channels, Some(6));
        assert_eq!(st.subtitle_language.as_deref(), Some("eng"));
        assert_eq!(st.bitrate, Some(5128000));
        assert_eq!(resolution_label(st.width, st.height), Some("1080p"));

        let st = Streams::extract(&streams, &json!({"SubtitleStreamIndex": -1}));
        assert_eq!(st.audio_codec.as_deref(), Some("aac"));
        assert!(st.subtitle_language.is_none());
    }

    #[test]
    fn remux_is_direct_stream() {
        let t = compact_transcode(&json!({"IsVideoDirect": true, "IsAudioDirect": true, "TranscodeReasons": "ContainerNotSupported, AudioCodecNotSupported"})).unwrap();
        assert_eq!(effective_play_method(Some("Transcode"), Some(&t)), "DirectStream");
        assert_eq!(t["reasons"].as_array().unwrap().len(), 2);
        let t = compact_transcode(&json!({"IsVideoDirect": true, "IsAudioDirect": false})).unwrap();
        assert_eq!(effective_play_method(Some("Transcode"), Some(&t)), "Transcode");
    }

    #[test]
    fn numeric_strings() {
        assert_eq!(int(&json!("1497")), Some(1497));
        assert_eq!(ticks_to_s(&json!("14400000000")), Some(1440));
    }

    #[test]
    fn every_track_language_is_kept_once_and_in_order() {
        let streams = serde_json::json!([
            {"Type": "Video", "Language": "und"},
            {"Type": "Audio", "Language": "JPN"}, {"Type": "Audio", "Language": "eng"}, {"Type": "Audio", "Language": "eng"}, {"Type": "Audio"},
            {"Type": "Subtitle", "Language": "eng"}, {"Type": "Subtitle", "Language": "<script>"},
        ]);
        assert_eq!(track_languages(&streams, "Audio").as_deref(), Some(r#"["jpn","eng","und"]"#));
        assert_eq!(track_languages(&streams, "Subtitle").as_deref(), Some(r#"["eng","und"]"#));
        assert_eq!(track_languages(&serde_json::json!([{"Type": "Video"}]), "Audio"), None);
        assert_eq!(track_languages(&Value::Null, "Audio"), None);
    }
}
