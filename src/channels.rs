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

/// One field a channel needs beyond an address, a token and the one beside them. Only email has any:
/// a letter needs a sender, and often a user name that is not the sender.
pub struct Extra {
    pub key: &'static str,
    /// This one holds an e-mail address, and is checked as one before it is stored.
    pub address: bool,
    pub label: &'static str,
    pub help: &'static str,
    pub example: &'static str,
    pub required: bool,
}

const EMAIL_EXTRAS: [Extra; 2] = [
    Extra {
        key: "from",
        address: true,
        label: "From",
        help: "The address the mail is sent as. Many servers only accept one they know.",
        example: "finstats@example.com",
        required: true,
    },
    Extra {
        key: "username",
        address: false,
        label: "User name (optional)",
        help: "Leave empty for a relay that needs no sign-in. Often the same as the From address.",
        example: "finstats@example.com",
        required: false,
    },
];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Channel {
    Discord,
    Telegram,
    Slack,
    Email,
    Ntfy,
    Gotify,
    Pushover,
    Pushbullet,
    Webhook,
}

impl Channel {
    pub const ALL: [Channel; 9] = [
        Channel::Discord, Channel::Telegram, Channel::Slack, Channel::Email, Channel::Ntfy,
        Channel::Gotify, Channel::Pushover, Channel::Pushbullet, Channel::Webhook,
    ];

    pub fn key(self) -> &'static str {
        match self {
            Channel::Webhook => "webhook",
            Channel::Discord => "discord",
            Channel::Slack => "slack",
            Channel::Telegram => "telegram",
            Channel::Email => "email",
            Channel::Ntfy => "ntfy",
            Channel::Gotify => "gotify",
            Channel::Pushover => "pushover",
            Channel::Pushbullet => "pushbullet",
        }
    }

    pub fn from_key(key: &str) -> Option<Channel> {
        Channel::ALL.into_iter().find(|c| c.key() == key)
    }

    pub fn label(self) -> &'static str {
        match self {
            Channel::Webhook => "Webhook",
            Channel::Discord => "Discord",
            Channel::Slack => "Slack",
            Channel::Telegram => "Telegram",
            Channel::Email => "Email",
            Channel::Ntfy => "ntfy",
            Channel::Gotify => "Gotify",
            Channel::Pushover => "Pushover",
            Channel::Pushbullet => "Pushbullet",
        }
    }

    pub fn what(self) -> &'static str {
        match self {
            Channel::Webhook => "A POST of JSON to any address: your own script, Home Assistant, n8n",
            Channel::Discord => "A channel in your server, through a webhook you create there",
            Channel::Slack => "A channel in your workspace, through an incoming webhook",
            Channel::Telegram => "A chat with a bot of your own, or a group or channel it is in",
            Channel::Email => "Any mailbox, through an SMTP server you already have",
            Channel::Ntfy => "ntfy.sh or your own ntfy, to a topic of your choosing",
            Channel::Gotify => "Your own Gotify server, with an application token",
            Channel::Pushover => "Pushover on your phone, with an application of your own",
            Channel::Pushbullet => "Pushbullet, on every device signed in to your account",
        }
    }

    /// Where the service lives, when that is not something anybody should have to type. A channel with
    /// one of these asks only for its token: the address is the service's own and never changes.
    pub fn fixed_url(self) -> Option<&'static str> {
        match self {
            Channel::Telegram => Some("https://api.telegram.org"),
            Channel::Pushover => Some("https://api.pushover.net"),
            Channel::Pushbullet => Some("https://api.pushbullet.com"),
            _ => None,
        }
    }

    /// The address to show in the field, and empty for a channel that fixes its own.
    pub fn example(self) -> &'static str {
        match self {
            Channel::Webhook => "https://example.com/hooks/finstats",
            Channel::Discord => "https://discord.com/api/webhooks/123456789/xxxxxxxx",
            Channel::Slack => "https://hooks.slack.com/services/T00000000/B00000000/xxxxxxxx",
            Channel::Email => "smtps://smtp.example.com:465",
            Channel::Ntfy => "https://ntfy.sh",
            Channel::Gotify => "http://192.168.1.10:8070",
            Channel::Telegram | Channel::Pushover | Channel::Pushbullet => "",
        }
    }

    /// The one field beside the address and the token, when a channel has one: which topic, which chat,
    /// whose key, which mailbox. `None` for a channel that says all of that in the address itself.
    pub fn topic_label(self) -> Option<&'static str> {
        match self {
            Channel::Ntfy => Some("Topic"),
            Channel::Telegram => Some("Chat ID"),
            Channel::Pushover => Some("User or group key"),
            Channel::Email => Some("To"),
            _ => None,
        }
    }

    pub fn topic_help(self) -> &'static str {
        match self {
            Channel::Ntfy => "The ntfy topic to publish to. Anybody who knows it can read your notifications, so make it hard to guess.",
            Channel::Telegram => "Message your bot once, then open https://api.telegram.org/bot<token>/getUpdates and use the chat id it reports. A channel may be given as @name.",
            Channel::Pushover => "Your user key, on the front page of pushover.net — or a group key.",
            Channel::Email => "The mailbox to send to.",
            _ => "",
        }
    }

    pub fn topic_example(self) -> &'static str {
        match self {
            Channel::Ntfy => "finstats-a8f3c1",
            Channel::Telegram => "-1001234567890",
            Channel::Pushover => "uQiRzpo4DXghDmr9QzzfQu27cmVRsG",
            Channel::Email => "me@example.com",
            _ => "",
        }
    }

    pub fn needs_topic(self) -> bool {
        self.topic_label().is_some()
    }

    /// What the secret beside the address is called, when there is one. A Discord or Slack webhook
    /// carries its own token in the URL, so there is nothing else to enter.
    pub fn secret_label(self) -> Option<&'static str> {
        match self {
            Channel::Webhook => Some("Authorization header (optional)"),
            Channel::Discord | Channel::Slack => None,
            Channel::Telegram => Some("Bot token"),
            Channel::Email => Some("Password (optional)"),
            Channel::Ntfy => Some("Access token (optional)"),
            Channel::Gotify => Some("Application token"),
            Channel::Pushover => Some("Application token"),
            Channel::Pushbullet => Some("Access token"),
        }
    }

    pub fn secret_required(self) -> bool {
        matches!(self, Channel::Gotify | Channel::Telegram | Channel::Pushover | Channel::Pushbullet)
    }

    /// Anything else the channel cannot do without. Only email has any.
    pub fn extras(self) -> &'static [Extra] {
        match self {
            Channel::Email => &EMAIL_EXTRAS,
            _ => &[],
        }
    }

    /// Email is not an HTTP request at all: `mail.rs` has its own shape and its own transport.
    pub fn is_mail(self) -> bool {
        self == Channel::Email
    }
}

/// Why a message did not arrive. `retry_after` is what the other side asked for, in seconds.
pub struct SendError {
    pub message: String,
    pub retry_after: Option<i64>,
}

impl SendError {
    pub fn of(message: impl Into<String>) -> SendError {
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

/// Discord's colour bar as Slack wants it: the same number, written the way a web page would.
fn hex(severity: &str) -> String {
    format!("#{:06x}", colour(severity))
}

/// Pushover has its own scale, from -2 (never disturb) to 2 (emergency, which needs an acknowledgement
/// and is never what finstats sends). Quiet for the everyday, the usual for a warning, loud for an alert.
fn pushover_priority(severity: &str) -> i64 {
    match severity {
        ALERT => 1,
        WARN => 0,
        _ => -1,
    }
}

/// How long a message may be on this channel. Pushover measures in its own, much smaller, units, so a
/// play that would fit anywhere else is cut to what it takes rather than refused by it.
fn body_max(channel: Channel) -> usize {
    match channel {
        Channel::Pushover => 1_000,
        Channel::Slack => 3_000,
        _ => BODY_MAX,
    }
}

/// What one destination is sent: where to, and the JSON body. Pure, so every shape has a test under it
/// and none of them needs a server to check. `None` for email, which is not an HTTP request at all —
/// `mail.rs` has its own shape and its own transport.
pub fn payload(channel: Channel, base_url: &str, topic: Option<&str>, secret: &str, m: &Message) -> Option<(String, Value)> {
    let title = cut(&m.title, TITLE_MAX);
    let body = cut(&m.body, body_max(channel));
    let text = cut(&m.text(), body_max(channel));
    let (kind_key, at, user) = (m.kind.key(), m.at, m.user.as_deref());
    Some(match channel {
        Channel::Email => return None,
        Channel::Webhook => (
            base_url.to_string(),
            json!({
                "event": kind_key, "severity": m.severity, "at": at, "title": title,
                "body": body, "link": m.link, "user": user,
                "fields": m.fields.iter().take(FIELDS_MAX).map(|(l, v)| json!({ "label": l, "value": cut(v, FIELD_MAX) })).collect::<Vec<_>>(),
                "source": concat!("finstats/", env!("CARGO_PKG_VERSION")),
            }),
        ),
        Channel::Discord => (
            base_url.to_string(),
            json!({
                "username": "finstats",
                "embeds": [{
                    "title": title, "description": body, "url": m.link, "color": colour(&m.severity),
                    "timestamp": chrono::DateTime::from_timestamp(at, 0).map(|t| t.to_rfc3339()),
                    "fields": fields_json(m, true), "footer": { "text": "finstats" },
                }],
            }),
        ),
        // Slack shows `text` in the list of conversations and the attachment when the message is opened,
        // so the title is said twice on purpose: the coloured card alone would be a blank line in the list.
        Channel::Slack => (
            base_url.to_string(),
            json!({
                "text": title,
                "attachments": [{
                    "color": hex(&m.severity), "title": title, "title_link": m.link, "text": body,
                    "fields": m.fields.iter().take(FIELDS_MAX).map(|(l, v)| json!({ "title": cut(l, 100), "value": cut(v, FIELD_MAX), "short": true })).collect::<Vec<_>>(),
                    "footer": "finstats", "ts": at,
                }],
            }),
        ),
        // The bot token is the path, which is why nothing here is ever logged with its URL. No
        // `parse_mode`: a film title is not markup, and a title with a `_` in it would either break the
        // message or have to be escaped in a way that shows up in the text.
        Channel::Telegram => (
            format!("{base_url}/bot{secret}/sendMessage"),
            json!({
                "chat_id": topic.unwrap_or_default(),
                "text": format!("{title}\n{text}"),
                "disable_web_page_preview": true,
            }),
        ),
        Channel::Pushover => (
            format!("{base_url}/1/messages.json"),
            json!({
                "token": secret, "user": topic.unwrap_or_default(), "title": title, "message": text,
                "priority": pushover_priority(&m.severity), "url": m.link, "url_title": "Open in finstats",
            }),
        ),
        // A push with somewhere to go is a link; one without is a note, rather than a link to nowhere.
        Channel::Pushbullet => (
            format!("{base_url}/v2/pushes"),
            json!({
                "type": if m.link.is_some() { "link" } else { "note" },
                "title": title, "body": text, "url": m.link,
            }),
        ),
        // Both of these take their title in the body rather than in a header: a title is a film title,
        // and a film title is not always ASCII.
        Channel::Ntfy => (
            base_url.to_string(),
            json!({
                "topic": topic.unwrap_or_default(), "title": title, "message": text,
                "priority": priority(&m.severity), "tags": tags(&m.severity), "click": m.link,
            }),
        ),
        Channel::Gotify => (
            format!("{base_url}/message"),
            json!({
                "title": title, "message": text, "priority": priority(&m.severity),
                "extras": { "client::display": { "contentType": "text/plain" },
                            "client::notification": m.link.as_ref().map(|l| json!({ "click": { "url": l } })) },
            }),
        ),
    })
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
    let Some((url, body)) = payload(t.channel, t.url(), t.topic.as_deref(), t.secret(), m) else {
        // Email: a different transport, and the only one that is not a POST of JSON.
        return crate::mail::deliver(
            crate::mail::Account {
                url: t.url(),
                from: t.option("from"),
                to: t.topic.as_deref().unwrap_or_default(),
                username: t.option("username"),
                password: t.secret(),
                lax: t.accept_invalid_certs,
            },
            m,
        )
        .await;
    };
    let client = app.services_http.client(t.accept_invalid_certs);
    let mut req = client.post(&url).json(&body);
    // Where each service wants its token. Telegram's is the path and Pushover's is in the body, both
    // built above; these are the ones that go in a header.
    match t.channel {
        Channel::Gotify => req = req.header("X-Gotify-Key", t.secret()),
        Channel::Pushbullet => req = req.header("Access-Token", t.secret()),
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
        // Telegram and Pushover both answer 400 for a chat or a user key they do not know, and the one
        // the owner just typed is far more likely to be wrong than the token they pasted.
        400 => match t.channel {
            Channel::Telegram => format!("{label} does not know that chat. Send the bot a message first, then use the chat id it reports"),
            Channel::Pushover => format!("{label} did not accept the user key or the application token"),
            _ => format!("{label} did not understand the message"),
        },
        401 | 403 => match t.channel {
            Channel::Discord => format!("{label} refused the webhook. Has it been deleted?"),
            Channel::Telegram => format!("{label} refused the bot token"),
            Channel::Pushbullet => format!("{label} refused the access token"),
            _ => format!("{label} refused the token"),
        },
        404 => match t.channel {
            Channel::Slack => format!("{label} no longer knows that webhook. Has the app or the channel been removed?"),
            _ => format!("{host} answered 404 Not Found. Is the address right, topic and base path included?"),
        },
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

    /// The payload of a channel that posts JSON, which is every one of them but email.
    fn post(channel: Channel, base: &str, topic: Option<&str>, m: &Message) -> (String, Value) {
        payload(channel, base, topic, "s3cret-token", m).expect("an HTTP channel has a payload")
    }

    #[test]
    fn every_channel_is_posted_the_way_it_expects() {
        let m = msg();
        let (url, body) = post(Channel::Webhook, "https://example.com/hook", None, &m);
        assert_eq!(url, "https://example.com/hook");
        assert_eq!(body["event"], "travel");
        assert_eq!(body["fields"][1]["label"], "Places");

        let (url, body) = post(Channel::Discord, "https://discord.com/api/webhooks/1/tok", None, &m);
        assert_eq!(url, "https://discord.com/api/webhooks/1/tok", "Discord is posted to exactly the webhook URL");
        assert_eq!(body["embeds"][0]["title"], "Impossible travel for alice");
        assert_eq!(body["embeds"][0]["color"], colour(ALERT));

        let (url, body) = post(Channel::Slack, "https://hooks.slack.com/services/T0/B0/tok", None, &m);
        assert_eq!(url, "https://hooks.slack.com/services/T0/B0/tok");
        assert_eq!(body["text"], "Impossible travel for alice", "what a Slack notification shows before the card");
        assert_eq!(body["attachments"][0]["color"], "#e5484d");
        assert_eq!(body["attachments"][0]["title_link"], "https://finstats.example/security");
        assert_eq!(body["attachments"][0]["fields"][0]["title"], "Person");

        let (url, body) = post(Channel::Telegram, "https://api.telegram.org", Some("-1001234567890"), &m);
        assert_eq!(url, "https://api.telegram.org/bots3cret-token/sendMessage", "the bot token is the path");
        assert_eq!(body["chat_id"], "-1001234567890");
        assert!(body.get("parse_mode").is_none(), "a film title is not markup, so none of it is parsed as any");
        let text = body["text"].as_str().unwrap();
        assert!(text.starts_with("Impossible travel for alice\n") && text.contains("Person: alice"));

        let (url, body) = post(Channel::Pushover, "https://api.pushover.net", Some("uQiRzpo4DXghDmr9QzzfQu27cmVRsG"), &m);
        assert_eq!(url, "https://api.pushover.net/1/messages.json");
        assert_eq!(body["token"], "s3cret-token", "the application token travels in the body");
        assert_eq!(body["user"], "uQiRzpo4DXghDmr9QzzfQu27cmVRsG");
        assert_eq!(body["priority"], 1, "an alert is the loud one, never Pushover's emergency");
        assert_eq!(body["url"], "https://finstats.example/security");

        let (url, body) = post(Channel::Pushbullet, "https://api.pushbullet.com", None, &m);
        assert_eq!(url, "https://api.pushbullet.com/v2/pushes");
        assert_eq!(body["type"], "link", "with a link it is a link, and a note without one");
        assert_eq!(body["url"], "https://finstats.example/security");
        assert_eq!(body["title"], "Impossible travel for alice");

        let (url, body) = post(Channel::Ntfy, "https://ntfy.sh", Some("finstats-abc"), &m);
        assert_eq!(url, "https://ntfy.sh", "the topic travels in the body, not in the path");
        assert_eq!(body["topic"], "finstats-abc");
        assert_eq!(body["priority"], 5);
        assert!(body["message"].as_str().unwrap().contains("Person: alice"), "a channel without fields still says everything");

        let (url, body) = post(Channel::Gotify, "http://nas:8070", None, &m);
        assert_eq!(url, "http://nas:8070/message");
        assert_eq!(body["extras"]["client::notification"]["click"]["url"], "https://finstats.example/security");

        assert!(payload(Channel::Email, "smtps://smtp.example.com", Some("me@example.com"), "", &m).is_none(), "email is not an HTTP request at all");
    }

    #[test]
    fn a_push_without_a_link_is_a_note_rather_than_a_link_to_nowhere() {
        let m = Message { link: None, ..msg() };
        let (_, body) = post(Channel::Pushbullet, "https://api.pushbullet.com", None, &m);
        assert_eq!(body["type"], "note");
        assert!(body["url"].is_null());
    }

    #[test]
    fn every_channel_says_what_it_asks_for_and_nothing_more() {
        for c in Channel::ALL {
            assert_eq!(Channel::from_key(c.key()), Some(c), "{}: a key round-trips", c.key());
            assert!(!c.label().is_empty() && !c.what().is_empty(), "{}", c.key());
            // A channel whose address is its service's own asks nobody to type one, and the other way round.
            assert_eq!(c.fixed_url().is_some(), c.example().is_empty(), "{}", c.key());
            assert!(c.secret_required() <= c.secret_label().is_some(), "{}: a token that is required is asked for", c.key());
        }
        // Telegram carries the bot token in the path and says which chat in the body: both are asked for.
        assert_eq!(Channel::Telegram.fixed_url(), Some("https://api.telegram.org"));
        assert_eq!(Channel::Telegram.topic_label(), Some("Chat ID"));
        assert!(Channel::Telegram.secret_required());
        assert_eq!(Channel::Slack.topic_label(), None, "a Slack webhook already says which channel it is for");
        assert_eq!(Channel::Pushover.topic_label(), Some("User or group key"));
        assert_eq!(Channel::Email.extras().iter().map(|e| e.key).collect::<Vec<_>>(), ["from", "username"]);
        assert_eq!(
            Channel::ALL.into_iter().filter(|c| !c.extras().is_empty()).collect::<Vec<_>>(),
            vec![Channel::Email],
            "only email needs more than an address, a token and one field beside them"
        );
    }

    #[test]
    fn a_long_title_is_cut_rather_than_refused() {
        let long = "a".repeat(400);
        let m = Message { kind: crate::notify::Kind::NewItems, at: 1, user: None, title: long, body: "b".repeat(9_000), link: None, severity: INFO.into(), fields: vec![] };
        let (_, body) = post(Channel::Discord, "https://discord.com/api/webhooks/1/t", None, &m);
        assert!(body["embeds"][0]["title"].as_str().unwrap().chars().count() <= TITLE_MAX);
        assert!(body["embeds"][0]["description"].as_str().unwrap().chars().count() <= BODY_MAX);
    }
}
