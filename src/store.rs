//! 기기 내부 저장 (localStorage)

use crate::model::Event;
use gloo_storage::{LocalStorage, Storage};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Config {
    /// "owner/name" 형식의 데이터 저장소
    #[serde(default)]
    pub repo: String,
    #[serde(default)]
    pub token: String,
    #[serde(default)]
    pub user: String,
    #[serde(default)]
    pub team: String,
    /// 이 기기 고유 id (기록 파일 이름)
    #[serde(default)]
    pub device: String,
    /// 이 기기에서 보고 있는 수련회
    #[serde(default)]
    pub retreat: String,
}

impl Config {
    pub fn load() -> Self {
        let mut c: Config = LocalStorage::get("cfg").unwrap_or_default();
        if c.device.is_empty() {
            c.device = new_id();
        }
        c
    }
    pub fn save(&self) {
        let _ = LocalStorage::set("cfg", self);
    }
    pub fn ready(&self) -> bool {
        !self.user.trim().is_empty() && !self.team.is_empty()
    }
    pub fn can_sync(&self) -> bool {
        valid_repo(&self.repo) && !self.token.trim().is_empty()
    }
    pub fn own_file(&self) -> String {
        format!("{}.jsonl", self.device)
    }
}

pub fn valid_repo(s: &str) -> bool {
    let mut parts = s.trim().split('/');
    matches!((parts.next(), parts.next(), parts.next()), (Some(a), Some(b), None) if !a.is_empty() && !b.is_empty())
}

/// GitHub에서 받아온 기록 파일 하나
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct FileCache {
    pub sha: String,
    pub events: Vec<Event>,
}

pub fn load_files() -> HashMap<String, FileCache> {
    LocalStorage::get("files").unwrap_or_default()
}

pub fn load_pending() -> Vec<Event> {
    LocalStorage::get("pending").unwrap_or_default()
}

pub fn new_id() -> String {
    let r1 = (js_sys::Math::random() * 4_294_967_295.0) as u32;
    let r2 = (js_sys::Math::random() * 65_535.0) as u32;
    format!("{:x}{:08x}{:04x}", js_sys::Date::now() as u64, r1, r2)
}

pub fn now() -> f64 {
    js_sys::Date::now()
}

pub fn fmt_time(ms: f64) -> String {
    let d = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(ms));
    format!(
        "{}/{} {:02}:{:02}",
        d.get_month() + 1,
        d.get_date(),
        d.get_hours(),
        d.get_minutes()
    )
}

/// 모든 기록 파일 + 아직 못 보낸 기록을 합쳐 중복 없이 시간순 정렬
pub fn merge(files: &HashMap<String, FileCache>, pending: &[Event]) -> Vec<Event> {
    let mut seen = HashSet::new();
    let mut all: Vec<Event> = files
        .values()
        .flat_map(|f| f.events.iter())
        .chain(pending.iter())
        .filter(|e| seen.insert(e.id.clone()))
        .cloned()
        .collect();
    all.sort_by(|a, b| a.at.total_cmp(&b.at).then_with(|| a.id.cmp(&b.id)));
    all
}

pub fn parse_jsonl(text: &str) -> Vec<Event> {
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

pub fn to_jsonl(events: &[Event]) -> String {
    let mut s = String::new();
    for e in events {
        if let Ok(line) = serde_json::to_string(e) {
            s.push_str(&line);
            s.push('\n');
        }
    }
    s
}
