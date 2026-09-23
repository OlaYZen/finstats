//! The four ways a message leaves finstats, and nothing else about notifications.
//!
//! A plain **webhook** (JSON, the shape documented in `docs/api.md`), **Discord** (an embed),
//! **ntfy** and **Gotify**. Two rules hold for all of them:
//!
//! - **The address is a secret.** A Discord webhook URL *is* its credential, and an ntfy topic is
//!   whoever knows it, so a URL is never logged, never put in an error and never sent to a browser.
//!   Errors say a status and a kind of failure, the way `services.rs` does, and nothing the other side
//!   returned.
//! - **No redirect is followed.** These requests carry tokens in headers and in the URL itself;
//!   `services::Http` already refuses redirects, so it is the client used here too.
//!
//! Both ntfy and Gotify are published to in their JSON form rather than through headers, because a title
//! is a film title: "Amélie" in an HTTP header is not something to find out about in the field.

use anyhow::Result;
use reqwest::StatusCode;
use reqwest::header::RETRY_AFTER;
use serde_json::{Value, json};

use crate::notify::{ALERT, Message, Target, WARN};
use crate::state::App;

/// Discord's own limits, and sensible ones for the rest.
const TITLE_MAX: usize = 240;
const BODY_MAX: usize = 3_500;
const FIELD_MAX: usize = 900;
const FIELDS_MAX: usize = 20;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    Webhook,
    Discord,
    Ntfy,
    Gotify,
}

impl Channel {
    pub const ALL: [Channel; 4] = [Channel::Webhook, Channel::Discord, Channel::Ntfy, Channel::Gotify];

    pub fn key(self) -> &'static str {
        match self {
            Channel::Webhook => "webhook",
            Channel::Discord => "discord",
            Channel::Ntfy => "ntfy",
            Channel::Gotify => "gotify",
        }
    }

    pub fn from_key(key: &str) -> Option<Channel> {
        Channel::ALL.into_iter().find(|c| c.key() == key)
    }

    pub fn label(self) -> &'static str {
        match self {
            Channel::Webhook => "Webhook",
            Channel::Discord => "Discord",
            Channel::Ntfy => "ntfy",
            Channel::Gotify => "Gotify",
        }
    }

    pub fn what(self) -> &'static str {
        match self {
            Channel::Webhook => "A POST of JSON to any address: your own script, Home Assistant, n8n",
            Channel::Discord => "A channel in your server, through a webhook you create there",
            Channel::Ntfy => "ntfy.sh or your own ntfy, to a topic of your choosing",
            Channel::Gotify => "Your own Gotify server, with an application token",
        }
    }

    pub fn example(self) -> &'static str {
        match self {
            Channel::Webhook => "https://example.com/hooks/finstats",
            Channel::Discord => "https://discord.com/api/webhooks/123456789/xxxxxxxx",
            Channel::Ntfy => "https://ntfy.sh",
            Channel::Gotify => "http://192.168.1.10:8070",
        }
    }

    /// ntfy sends to a topic, which is its own field: it is half the address and half the password.
    pub fn needs_topic(self) -> bool {
        self == Channel::Ntfy
    }

    /// What the secret beside the address is called, when there is one. A Discord webhook carries its
    /// own token in the URL, so there is nothing else to enter.
    pub fn secret_label(self) -> Option<&'static str> {
        match self {
            Channel::Webhook => Some("Authorization header (optional)"),
            Channel::Discord => None,
            Channel::Ntfy => Some("Access token (optional)"),
            Channel::Gotify => Some("Application token"),
        }
    }

    pub fn secret_required(self) -> bool {
        self == Channel::Gotify
    }
}

/// Why a message did not arrive. `retry_after` is what the other side asked for, in seconds.
pub struct SendError {
    pub message: String,
    pub retry_after: Option<i64>,
}

impl SendError {
    fn of(message: impl Into<String>) -> SendError {
        SendError { message: message.into(), retry_after: None }
    }
}

fn cut(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let kept: String = text.chars().take(max.saturating_sub(1)).collect();
    format!("{kept}…")
}

fn fields_json(m: &Message, inline: bool) -> Vec<Value> {
    m.fields
        .iter()
        .take(FIELDS_MAX)
        .map(|(label, value)| json!({ "name": cut(label, 100), "value": cut(value, FIELD_MAX), "inline": inline }))
        .collect()
}

/// Discord's colour bar, and ntfy's and Gotify's priorities: the one place severity turns into a number.
fn colour(severity: &str) -> u32 {
    match severity {
        ALERT => 0xE5484D,
        WARN => 0xE0A300,
        _ => 0x4F8CFF,
    }
}

fn priority(severity: &str) -> i64 {
    match severity {
        ALERT => 5,
        WARN => 4,
        _ => 3,
    }
}

fn tags(severity: &str) -> Vec<&'static str> {
    match severity {
        ALERT => vec!["rotating_light"],
        WARN => vec!["warning"],
        _ => vec!["information_source"],
    }
}

/// What one destination is sent: where to, and the JSON body. Pure, so every shape has a test under it
/// and none of them needs a server to check.
pub fn payload(channel: Channel, base_url: &str, topic: Option<&str>, m: &Message) -> (String, Value) {
    let title = cut(&m.title, TITLE_MAX);
    let (kind_key, at, user) = (m.kind.key(), m.at, m.user.as_deref());
    match channel {
        Channel::Webhook => (
            base_url.to_string(),
            json!({
                "event": kind_key, "severity": m.severity, "at": at, "title": title,
                "body": cut(&m.body, BODY_MAX), "link": m.link, "user": user,
                "fields": m.fields.iter().take(FIELDS_MAX).map(|(l, v)| json!({ "label": l, "value": cut(v, FIELD_MAX) })).collect::<Vec<_>>(),
                "source": concat!("finstats/", env!("CARGO_PKG_VERSION")),
            }),
        ),
        Channel::Discord => (
            base_url.to_string(),
            json!({
                "username": "finstats",
                "embeds": [{
                    "title": title, "description": cut(&m.body, BODY_MAX), "url": m.link, "color": colour(&m.severity),
                    "timestamp": chrono::DateTime::from_timestamp(at, 0).map(|t| t.to_rfc3339()),
                    "fields": fields_json(m, true), "footer": { "text": "finstats" },
                }],
            }),
        ),
        // Both of these take their title in the body rather than in a header: a title is a film title,
        // and a film title is not always ASCII.
        Channel::Ntfy => (
            base_url.to_string(),
            json!({
                "topic": topic.unwrap_or_default(), "title": title, "message": cut(&m.text(), BODY_MAX),
                "priority": priority(&m.severity), "tags": tags(&m.severity), "click": m.link,
            }),
        ),
        Channel::Gotify => (
            format!("{base_url}/message"),
            json!({
                "title": title, "message": cut(&m.text(), BODY_MAX), "priority": priority(&m.severity),
                "extras": { "client::display": { "contentType": "text/plain" },
                            "client::notification": m.link.as_ref().map(|l| json!({ "click": { "url": l } })) },
            }),
        ),
    }
}

/// Send one message to one destination. The address never appears in what comes back from here.
pub async fn send(app: &App, t: &Target, m: &Message) -> Result<(), SendError> {
    // A personal destination may only ever point at a public host, and a name that answered publicly
    // yesterday can be pointed inwards today, so this is checked again on every send, not only on save.
    if t.owner_id.is_some()
        && let Err(e) = crate::notify::must_be_public(t.url()).await
    {
        return Err(SendError::of(format!("{e}")));
    }
    let (url, body) = payload(t.channel, t.url(), t.topic.as_deref(), m);
    let client = app.services_http.client(t.accept_invalid_certs);
    let mut req = client.post(&url).json(&body);
    match t.channel {
        Channel::Gotify => req = req.header("X-Gotify-Key", t.secret()),
        Channel::Webhook | Channel::Ntfy if !t.secret().is_empty() => req = req.header("Authorization", format!("Bearer {}", t.secret())),
        _ => {}
    }
    let resp = req.send().await.map_err(|e| SendError::of(explain(e, t)))?;
    let status = resp.status();
    if status.is_success() {
        return Ok(());
    }
    let retry_after = resp.headers().get(RETRY_AFTER).and_then(|v| v.to_str().ok()).and_then(|v| v.trim().parse::<i64>().ok());
    Err(SendError { message: refuse(status, t), retry_after })
}

/// Why a request never got an answer, in words, and without the URL reqwest would print: that URL is the
/// credential. Errors from here are shown in the UI and written to the log.
fn explain(e: reqwest::Error, t: &Target) -> String {
    let (label, host) = (t.channel.label(), crate::outbound::host_of(t.url()));
    if e.is_timeout() {
        format!("{label} at {host} did not answer in time")
    } else if e.is_connect() {
        format!("Could not connect to {host}. Is it reachable from where finstats runs?")
    } else if e.is_decode() {
        format!("{host} did not answer like {label}")
    } else {
        format!("Could not reach {host}")
    }
}

/// An answer that is not the one asked for: the status and what it usually means, never the body.
fn refuse(status: StatusCode, t: &Target) -> String {
    let (label, host) = (t.channel.label(), crate::outbound::host_of(t.url()));
    if status.is_redirection() {
        return format!("{host} answers with a redirect. finstats follows none, so that a token can never end up somewhere else: enter the address it redirects to");
    }
    match status.as_u16() {
        401 | 403 => match t.channel {
            Channel::Discord => format!("{label} refused the webhook. Has it been deleted?"),
            _ => format!("{label} refused the token"),
        },
        404 => format!("{host} answered 404 Not Found. Is the address right, topic and base path included?"),
        413 => format!("{label} found the message too large"),
        429 => format!("{label} is rate-limiting finstats"),
        _ => format!("{label} answered {status}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notify::INFO;

    fn msg() -> Message {
        Message {
            kind: crate::notify::Kind::Travel,
            at: 1000,
            user: Some("alice".into()),
            title: "Impossible travel for alice".into(),
            body: "Oslo and London, 40 minutes apart".into(),
            link: Some("https://finstats.example/security".into()),
            severity: ALERT.into(),
            fields: vec![("Person".into(), "alice".into()), ("Places".into(), "Oslo → London".into())],
        }
    }

    #[test]
    fn every_channel_is_posted_the_way_it_expects() {
        let m = msg();
        let (url, body) = payload(Channel::Webhook, "https://example.com/hook", None, &m);
        assert_eq!(url, "https://example.com/hook");
        assert_eq!(body["event"], "travel");
        assert_eq!(body["fields"][1]["label"], "Places");

        let (url, body) = payload(Channel::Discord, "https://discord.com/api/webhooks/1/tok", None, &m);
        assert_eq!(url, "https://discord.com/api/webhooks/1/tok", "Discord is posted to exactly the webhook URL");
        assert_eq!(body["embeds"][0]["title"], "Impossible travel for alice");
        assert_eq!(body["embeds"][0]["color"], colour(ALERT));

        let (url, body) = payload(Channel::Ntfy, "https://ntfy.sh", Some("finstats-abc"), &m);
        assert_eq!(url, "https://ntfy.sh", "the topic travels in the body, not in the path");
        assert_eq!(body["topic"], "finstats-abc");
        assert_eq!(body["priority"], 5);
        assert!(body["message"].as_str().unwrap().contains("Person: alice"), "a channel without fields still says everything");

        let (url, body) = payload(Channel::Gotify, "http://nas:8070", None, &m);
        assert_eq!(url, "http://nas:8070/message");
        assert_eq!(body["extras"]["client::notification"]["click"]["url"], "https://finstats.example/security");
    }

    #[test]
    fn a_long_title_is_cut_rather_than_refused() {
        let long = "a".repeat(400);
        let m = Message { kind: crate::notify::Kind::NewItems, at: 1, user: None, title: long, body: "b".repeat(9_000), link: None, severity: INFO.into(), fields: vec![] };
        let (_, body) = payload(Channel::Discord, "https://discord.com/api/webhooks/1/t", None, &m);
        assert!(body["embeds"][0]["title"].as_str().unwrap().chars().count() <= TITLE_MAX);
        assert!(body["embeds"][0]["description"].as_str().unwrap().chars().count() <= BODY_MAX);
    }
}
