//! The playback row, as written by both the live collector and the importer.

use anyhow::Result;
use serde_json::Value;

use crate::db::rusqlite::{Connection, named_params};
use crate::media::Streams;

#[derive(Debug, Clone, Default)]
pub struct PlayRecord {
    pub source: &'static str,
    pub source_id: Option<String>,
    pub active: bool,
    pub user_id: String,
    pub user_name: String,
    pub item_id: String,
    pub item_name: String,
    pub item_type: String,
    pub series_id: Option<String>,
    pub series_name: Option<String>,
    pub season_id: Option<String>,
    pub season_number: Option<i64>,
    pub episode_number: Option<i64>,
    pub started_at: i64,
    pub ended_at: i64,
    pub duration_s: i64,
    pub paused_s: i64,
    pub position_s: Option<i64>,
    pub runtime_s: Option<i64>,
    pub client: Option<String>,
    pub device_name: Option<String>,
    pub device_id: Option<String>,
    pub app_version: Option<String>,
    pub remote_ip: Option<String>,
    pub play_method: String,
    pub container: Option<String>,
    pub streams: Streams,
    pub transcode: Option<Value>,
}

const COLUMNS: &str = "source, source_id, active, user_id, user_name, item_id, item_name, item_type,
    series_id, series_name, season_id, season_number, episode_number, library_id,
    started_at, ended_at, duration_s, paused_s, position_s, runtime_s,
    client, device_name, device_id, app_version, remote_ip, play_method, container, bitrate,
    video_codec, width, height, video_range, bit_depth,
    audio_codec, audio_channels, audio_language, subtitle_codec, subtitle_language, transcode";

const VALUES: &str = ":source, :source_id, :active, :user_id, :user_name, :item_id, :item_name, :item_type,
    :series_id, :series_name, :season_id, :season_number, :episode_number,
    COALESCE((SELECT library_id FROM items WHERE id = :item_id), (SELECT library_id FROM items WHERE id = :series_id)),
    :started_at, :ended_at, :duration_s, :paused_s, :position_s, :runtime_s,
    :client, :device_name, :device_id, :app_version, :remote_ip, :play_method, :container, :bitrate,
    :video_codec, :width, :height, :video_range, :bit_depth,
    :audio_codec, :audio_channels, :audio_language, :subtitle_codec, :subtitle_language, :transcode";

impl PlayRecord {
    /// Inserts the row. Returns `None` when `source_id` already exists (duplicate import).
    pub fn insert(&self, conn: &Connection) -> Result<Option<i64>> {
        let sql = format!("INSERT OR IGNORE INTO playbacks ({COLUMNS}) VALUES ({VALUES})");
        let mut stmt = conn.prepare_cached(&sql)?;
        let transcode = self.transcode.as_ref().map(|t| t.to_string());
        let n = stmt.execute(named_params! {
            ":source": self.source,
            ":source_id": self.source_id,
            ":active": self.active,
            ":user_id": self.user_id,
            ":user_name": self.user_name,
            ":item_id": self.item_id,
            ":item_name": self.item_name,
            ":item_type": self.item_type,
            ":series_id": self.series_id,
            ":series_name": self.series_name,
            ":season_id": self.season_id,
            ":season_number": self.season_number,
            ":episode_number": self.episode_number,
            ":started_at": self.started_at,
            ":ended_at": self.ended_at,
            ":duration_s": self.duration_s,
            ":paused_s": self.paused_s,
            ":position_s": self.position_s,
            ":runtime_s": self.runtime_s,
            ":client": self.client,
            ":device_name": self.device_name,
            ":device_id": self.device_id,
            ":app_version": self.app_version,
            ":remote_ip": self.remote_ip,
            ":play_method": self.play_method,
            ":container": self.container,
            ":bitrate": self.streams.bitrate,
            ":video_codec": self.streams.video_codec,
            ":width": self.streams.width,
            ":height": self.streams.height,
            ":video_range": self.streams.video_range,
            ":bit_depth": self.streams.bit_depth,
            ":audio_codec": self.streams.audio_codec,
            ":audio_channels": self.streams.audio_channels,
            ":audio_language": self.streams.audio_language,
            ":subtitle_codec": self.streams.subtitle_codec,
            ":subtitle_language": self.streams.subtitle_language,
            ":transcode": transcode,
        })?;
        Ok((n > 0).then(|| conn.last_insert_rowid()))
    }

    /// Refresh the parts of a live row that change while it plays.
    pub fn update_progress(&self, conn: &Connection, row_id: i64) -> Result<()> {
        let transcode = self.transcode.as_ref().map(|t| t.to_string());
        conn.prepare_cached(
            "UPDATE playbacks SET active = :active, ended_at = :ended_at, duration_s = :duration_s, paused_s = :paused_s,
                 position_s = :position_s, play_method = :play_method, remote_ip = :remote_ip,
                 audio_codec = :audio_codec, audio_channels = :audio_channels, audio_language = :audio_language,
                 subtitle_codec = :subtitle_codec, subtitle_language = :subtitle_language,
                 transcode = COALESCE(:transcode, transcode)
             WHERE id = :id",
        )?
        .execute(named_params! {
            ":id": row_id,
            ":active": self.active,
            ":ended_at": self.ended_at,
            ":duration_s": self.duration_s,
            ":paused_s": self.paused_s,
            ":position_s": self.position_s,
            ":play_method": self.play_method,
            ":remote_ip": self.remote_ip,
            ":audio_codec": self.streams.audio_codec,
            ":audio_channels": self.streams.audio_channels,
            ":audio_language": self.streams.audio_language,
            ":subtitle_codec": self.streams.subtitle_codec,
            ":subtitle_language": self.streams.subtitle_language,
            ":transcode": transcode,
        })?;
        Ok(())
    }
}
