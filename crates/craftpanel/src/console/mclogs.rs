use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::http::StatusCode;
use serde::{Deserialize, Serialize};

use crate::auth::error::{Failure, Result};
use crate::model::Id;

pub const BASE: &str = "https://api.mclo.gs";

const AGENT: &str = concat!("craftpanel/", env!("CARGO_PKG_VERSION"));
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const READ_TIMEOUT: Duration = Duration::from_secs(30);
const WHOLE_TIMEOUT: Duration = Duration::from_secs(60);
const CACHE_FOR: Duration = Duration::from_secs(10 * 60);
const BLOCKED_FOR: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Key {
    Log { server: Id, modified: i64, size: u64 },
    Buffer { server: Id, seq: u64, lines: usize },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EntryLine {
    pub number: i64,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub level: i64,
    pub time: Option<String>,
    pub prefix: String,
    #[serde(default)]
    pub lines: Vec<EntryLine>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Solution {
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Problem {
    pub message: String,
    pub counter: i64,
    pub entry: Entry,
    #[serde(default)]
    pub solutions: Vec<Solution>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Information {
    pub message: String,
    pub counter: i64,
    pub label: String,
    pub value: String,
    pub entry: Entry,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Analysis {
    #[serde(default)]
    pub problems: Vec<Problem>,
    #[serde(default)]
    pub information: Vec<Information>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CrashAnalysis {
    pub id: String,
    pub name: Option<String>,
    #[serde(rename = "type")]
    pub kind: String,
    pub version: Option<String>,
    pub title: String,
    pub analysis: Analysis,
}

pub struct Mclogs {
    http: reqwest::Client,
    base: String,
    cache: Mutex<HashMap<Key, (Instant, CrashAnalysis)>>,
    blocked_until: Mutex<Option<Instant>>,
}

impl Mclogs {
    pub fn new() -> Self {
        Self::with_base(BASE)
    }

    pub fn with_base(base: impl Into<String>) -> Self {
        let http = reqwest::Client::builder()
            .user_agent(AGENT)
            .connect_timeout(CONNECT_TIMEOUT)
            .read_timeout(READ_TIMEOUT)
            .build()
            .unwrap_or_default();
        Self {
            http,
            base: base.into().trim_end_matches('/').to_owned(),
            cache: Mutex::new(HashMap::new()),
            blocked_until: Mutex::new(None),
        }
    }

    pub async fn analyse(&self, key: Key, content: &str) -> Result<CrashAnalysis> {
        let now = Instant::now();
        if let Some(known) = self.remembered(key, now) {
            return Ok(known);
        }
        if self.blocked(now) {
            return Err(rate_limited());
        }

        let sent = self
            .http
            .post(format!("{}/1/analyse", self.base))
            .header(reqwest::header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(form("content", content))
            .timeout(WHOLE_TIMEOUT)
            .send()
            .await
            .map_err(|err| unavailable(err.to_string()))?;

        let status = sent.status();
        if status == StatusCode::TOO_MANY_REQUESTS {
            *self.blocked_until.lock().expect("the mclo.gs lock") =
                Some(Instant::now() + BLOCKED_FOR);
            return Err(rate_limited());
        }

        let body = sent.bytes().await.map_err(|err| unavailable(err.to_string()))?;
        if !status.is_success() {
            return Err(unavailable(format!("mclo.gs answered {status}")));
        }

        let analysis = read(&body)?;
        self.cache
            .lock()
            .expect("the mclo.gs lock")
            .insert(key, (Instant::now(), analysis.clone()));
        Ok(analysis)
    }

    fn remembered(&self, key: Key, now: Instant) -> Option<CrashAnalysis> {
        let mut cache = self.cache.lock().expect("the mclo.gs lock");
        cache.retain(|_, (fetched, _)| now.duration_since(*fetched) < CACHE_FOR);
        cache.get(&key).map(|(_, analysis)| analysis.clone())
    }

    fn blocked(&self, now: Instant) -> bool {
        let mut until = self.blocked_until.lock().expect("the mclo.gs lock");
        match *until {
            Some(moment) if moment > now => true,
            Some(_) => {
                *until = None;
                false
            }
            None => false,
        }
    }
}

impl Default for Mclogs {
    fn default() -> Self {
        Self::new()
    }
}

fn read(body: &[u8]) -> Result<CrashAnalysis> {
    let answered: serde_json::Value =
        serde_json::from_slice(body).map_err(|err| unavailable(err.to_string()))?;

    if answered.get("success") == Some(&serde_json::Value::Bool(false)) {
        let why = answered.get("error").and_then(serde_json::Value::as_str).unwrap_or("no reason");
        return Err(unavailable(format!("mclo.gs refused the log: {why}")));
    }

    serde_json::from_value(answered)
        .map_err(|err| unavailable(format!("mclo.gs answered something unreadable: {err}")))
}

fn form(field: &str, value: &str) -> String {
    let mut body = String::with_capacity(value.len() + field.len() + 8);
    body.push_str(field);
    body.push('=');
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                body.push(byte as char);
            }
            b' ' => body.push('+'),
            other => body.push_str(&format!("%{other:02X}")),
        }
    }
    body
}

fn rate_limited() -> Failure {
    Failure::new(
        StatusCode::TOO_MANY_REQUESTS,
        "upstream_rate_limited",
        "mclo.gs is rate limiting this panel; the analysis can be asked for again shortly",
    )
}

fn unavailable(why: String) -> Failure {
    Failure::new(StatusCode::BAD_GATEWAY, "upstream_unavailable", format!("mclo.gs: {why}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ANSWER: &str = r#"{
        "success": true,
        "id": "abc123",
        "name": null,
        "type": "Vanilla",
        "version": null,
        "title": "Minecraft Server",
        "analysis": {
            "problems": [{
                "message": "FAILED TO BIND TO PORT",
                "counter": 1,
                "entry": { "level": 2, "time": null, "prefix": "", "lines": [
                    { "number": 12, "content": "***" }
                ]},
                "solutions": [{ "message": "Change the port" }]
            }],
            "information": []
        },
        "entries": [{ "level": 0, "time": null, "prefix": "", "lines": [] }]
    }"#;

    #[test]
    fn the_answer_is_trimmed_to_what_the_layout_reads() {
        let analysis = read(ANSWER.as_bytes()).expect("a readable answer");
        assert_eq!(analysis.name, None, "the real API answers null and Modrinth's type is wrong");
        assert_eq!(analysis.version, None);
        assert_eq!(analysis.kind, "Vanilla");
        assert_eq!(analysis.analysis.problems[0].solutions[0].message, "Change the port");

        let out = serde_json::to_value(&analysis).expect("json");
        assert!(out.get("entries").is_none(), "6.3: `entries` is dropped");
        assert!(out.get("success").is_none(), "6.3: `success` is dropped");
        assert_eq!(out["type"], "Vanilla", "the field is called `type` on the wire");
        assert!(out["name"].is_null());
    }

    #[test]
    fn a_refusal_from_mclogs_is_a_bad_gateway_and_says_why() {
        let refused = read(br#"{"success":false,"error":"No content"}"#).unwrap_err();
        assert_eq!(refused.code(), "upstream_unavailable");
        assert_eq!(refused.status(), StatusCode::BAD_GATEWAY);
        assert!(format!("{refused}").contains("No content"));

        assert_eq!(read(b"<html>").unwrap_err().code(), "upstream_unavailable");
    }

    #[test]
    fn the_body_is_one_form_field_and_nothing_else() {
        assert_eq!(form("content", "a b&c=d\n"), "content=a+b%26c%3Dd%0A");
        assert_eq!(form("content", "plain-log_1.0~x"), "content=plain-log_1.0~x");
    }

    #[test]
    fn an_answer_is_kept_for_ten_minutes_and_a_block_expires_by_itself() {
        let client = Mclogs::with_base("http://127.0.0.1:1");
        let key = Key::Log { server: Id::new(), modified: 7, size: 12 };
        let analysis = read(ANSWER.as_bytes()).unwrap();

        let now = Instant::now();
        client.cache.lock().unwrap().insert(key, (now, analysis.clone()));
        assert_eq!(client.remembered(key, now), Some(analysis));

        let later = now + CACHE_FOR + Duration::from_secs(1);
        assert_eq!(client.remembered(key, later), None, "and the entry is gone with it");
        assert!(client.cache.lock().unwrap().is_empty());

        *client.blocked_until.lock().unwrap() = Some(now + BLOCKED_FOR);
        assert!(client.blocked(now));
        assert!(!client.blocked(now + BLOCKED_FOR + Duration::from_secs(1)));
    }
}
