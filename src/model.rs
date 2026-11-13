//! 데이터 모델. 모든 변경은 "기록(Event)"으로만 쌓이고, 현재 상태는 기록을 순서대로 합산해서 계산한다.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

pub const TEAMS: &[&str] = &[
    "회계등록팀",
    "시설팀",
    "전산팀",
    "데코굿즈팀",
    "문서도서팀",
    "차량팀",
    "양식팀",
    "민원지원팀",
    "의전팀",
    "영상팀",
    "사진인스타팀",
];

/// 물품 담당팀으로 고를 수 있는 값 (팀 목록 + 공용)
pub fn owner_options() -> Vec<String> {
    let mut v: Vec<String> = TEAMS.iter().map(|s| s.to_string()).collect();
    v.push("공용".into());
    v
}

pub fn team_options() -> Vec<String> {
    TEAMS.iter().map(|s| s.to_string()).collect()
}

pub const CATEGORIES: &[&str] = &[
    "음향·영상",
    "전산·전자",
    "주방·식음",
    "레크·게임",
    "데코·굿즈",
    "문서·문구",
    "의료·안전",
    "차량",
    "소모품",
    "기타",
];

pub fn category_options() -> Vec<String> {
    CATEGORIES.iter().map(|s| s.to_string()).collect()
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Op {
    Checkout,
    Return,
    Consume,
    Lost,
    Damage,
    Adjust,
}

impl Op {
    pub fn label(self) -> &'static str {
        match self {
            Op::Checkout => "대여",
            Op::Return => "반납",
            Op::Consume => "소모",
            Op::Lost => "분실",
            Op::Damage => "파손",
            Op::Adjust => "수량조정",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            Op::Checkout => "팀이 물품을 가져갑니다.",
            Op::Return => "가져간 것 중 일부만 돌려받을 때 씁니다. 전부 돌려받을 때는 위의 '반납' 버튼을 누르세요.",
            Op::Consume => "소모품을 사용했습니다. 보유 수량이 줄어듭니다. 팀을 고르면 그 팀 대여분에서 먼저 뺍니다.",
            Op::Lost => "분실로 보유 수량이 줄어듭니다. 팀을 고르면 그 팀 대여분에서 먼저 뺍니다.",
            Op::Damage => "파손으로 보유 수량이 줄어듭니다. 팀을 고르면 그 팀 대여분에서 먼저 뺍니다.",
            Op::Adjust => "실사 결과나 구매로 보유 수량을 바꿉니다. 늘리면 +, 줄이면 -로 입력하세요.",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum Kind {
    Item {
        item: String,
        name: String,
        #[serde(default)]
        category: String,
        #[serde(default)]
        owner: String,
        #[serde(default)]
        box_no: String,
        #[serde(default)]
        note: String,
        /// 단위 (개, 장, 매 …). 비어 있으면 "개"
        #[serde(default, skip_serializing_if = "String::is_empty")]
        unit: String,
        /// 개수를 세지 않는 물품 (테이프, 물티슈처럼 "많음")
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        uncounted: bool,
    },
    ItemDelete {
        item: String,
    },
    Retreat {
        retreat: String,
        name: String,
    },
    Move {
        item: String,
        op: Op,
        qty: i64,
        #[serde(default)]
        to_team: String,
        #[serde(default)]
        person: String,
        #[serde(default)]
        retreat: String,
        #[serde(default)]
        memo: String,
    },
    Pack {
        retreat: String,
        item: String,
        /// "go" = 출발 전 챙김, "back" = 복귀 후 확인
        phase: String,
        done: bool,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Event {
    pub id: String,
    /// 기록 시각 (ms)
    pub at: f64,
    /// 기록한 사람
    pub by: String,
    /// 기록한 사람의 팀
    pub team: String,
    #[serde(flatten)]
    pub kind: Kind,
}

impl Event {
    pub fn item_id(&self) -> Option<&str> {
        match &self.kind {
            Kind::Item { item, .. }
            | Kind::ItemDelete { item }
            | Kind::Move { item, .. }
            | Kind::Pack { item, .. } => Some(item),
            Kind::Retreat { .. } => None,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ItemState {
    pub id: String,
    pub name: String,
    pub category: String,
    pub owner: String,
    pub box_no: String,
    pub note: String,
    pub unit: String,
    pub uncounted: bool,
    pub owned: i64,
    /// 팀별 대여 중 수량
    pub out: BTreeMap<String, i64>,
    /// 팀별 대여해 간 사람
    pub who: BTreeMap<String, Vec<String>>,
    pub deleted: bool,
    /// 물품 등록 기록을 받았는지 (기기 간 시계 차이로 대여 기록이 먼저 보일 수 있음)
    pub known: bool,
}

impl ItemState {
    pub fn out_total(&self) -> i64 {
        self.out.values().filter(|v| **v > 0).sum()
    }
    pub fn available(&self) -> i64 {
        self.owned - self.out_total()
    }
    pub fn out_of(&self, team: &str) -> i64 {
        self.out.get(team).copied().unwrap_or(0)
    }
    pub fn visible(&self) -> bool {
        self.known && !self.deleted
    }
    pub fn unit(&self) -> &str {
        if self.unit.is_empty() {
            "개"
        } else {
            &self.unit
        }
    }
    /// 목록에 보여줄 수량 ("12개", "여러 개")
    pub fn qty_label(&self) -> String {
        if self.uncounted {
            "여러 개".into()
        } else {
            format!("{}{}", self.available(), self.unit())
        }
    }
    /// 가용 수량이 바닥났는지 (수량을 세지 않는 물품은 해당 없음)
    pub fn empty(&self) -> bool {
        !self.uncounted && self.available() <= 0
    }
}

/// 담당팀 표시 순서 (팀 목록 순서, 공용은 맨 뒤)
pub fn owner_rank(owner: &str) -> usize {
    TEAMS
        .iter()
        .position(|t| *t == owner)
        .unwrap_or(if owner == "공용" { TEAMS.len() } else { TEAMS.len() + 1 })
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Inventory {
    pub items: BTreeMap<String, ItemState>,
    /// (id, 이름), 생성 순서
    pub retreats: Vec<(String, String)>,
    pub packs: HashMap<String, bool>,
}

pub fn pack_key(retreat: &str, item: &str, phase: &str) -> String {
    format!("{retreat}|{item}|{phase}")
}

impl Inventory {
    pub fn packed(&self, retreat: &str, item: &str, phase: &str) -> bool {
        self.packs
            .get(&pack_key(retreat, item, phase))
            .copied()
            .unwrap_or(false)
    }
    pub fn name_of(&self, item: &str) -> String {
        self.items
            .get(item)
            .filter(|i| !i.name.is_empty())
            .map(|i| i.name.clone())
            .unwrap_or_else(|| "(알 수 없는 물품)".into())
    }
    pub fn retreat_name(&self, id: &str) -> String {
        self.retreats
            .iter()
            .find(|r| r.0 == id)
            .map(|r| r.1.clone())
            .unwrap_or_default()
    }
}

fn reduce_out(it: &mut ItemState, team: &str, q: i64) {
    let cur = it.out.get(team).copied().unwrap_or(0);
    let next = (cur - q).max(0);
    if next == 0 {
        it.out.remove(team);
        it.who.remove(team);
    } else {
        it.out.insert(team.to_string(), next);
    }
}

/// 기록들을 (시각, id) 순으로 정렬된 상태로 받아 현재 상태를 계산한다.
pub fn compute(events: &[Event]) -> Inventory {
    let mut inv = Inventory::default();
    for e in events {
        match &e.kind {
            Kind::Item {
                item,
                name,
                category,
                owner,
                box_no,
                note,
                unit,
                uncounted,
            } => {
                let it = inv.items.entry(item.clone()).or_default();
                it.id = item.clone();
                it.name = name.clone();
                it.category = category.clone();
                it.owner = owner.clone();
                it.box_no = box_no.clone();
                it.note = note.clone();
                it.unit = unit.clone();
                it.uncounted = *uncounted;
                it.known = true;
                it.deleted = false;
            }
            Kind::ItemDelete { item } => {
                if let Some(it) = inv.items.get_mut(item) {
                    it.deleted = true;
                }
            }
            Kind::Retreat { retreat, name } => {
                if let Some(r) = inv.retreats.iter_mut().find(|r| r.0 == *retreat) {
                    r.1 = name.clone();
                } else {
                    inv.retreats.push((retreat.clone(), name.clone()));
                }
            }
            Kind::Move {
                item,
                op,
                qty,
                to_team,
                person,
                ..
            } => {
                let it = inv.items.entry(item.clone()).or_default();
                it.id = item.clone();
                let q = *qty;
                match op {
                    Op::Checkout => {
                        *it.out.entry(to_team.clone()).or_default() += q;
                        let p = person.trim();
                        if !p.is_empty() {
                            let w = it.who.entry(to_team.clone()).or_default();
                            if !w.iter().any(|x| x == p) {
                                w.push(p.to_string());
                            }
                        }
                    }
                    Op::Return => reduce_out(it, to_team, q),
                    Op::Consume | Op::Lost | Op::Damage => {
                        it.owned -= q;
                        if !to_team.is_empty() {
                            reduce_out(it, to_team, q);
                        }
                    }
                    Op::Adjust => it.owned += q,
                }
            }
            Kind::Pack {
                retreat,
                item,
                phase,
                done,
            } => {
                inv.packs.insert(pack_key(retreat, item, phase), *done);
            }
        }
    }
    inv
}

/// 기록 한 건을 사람이 읽는 문장으로
pub fn describe(e: &Event, inv: &Inventory) -> String {
    match &e.kind {
        Kind::Item { name, .. } => format!("물품 등록·수정: {name}"),
        Kind::ItemDelete { item } => format!("물품 삭제: {}", inv.name_of(item)),
        Kind::Retreat { name, .. } => format!("수련회 생성: {name}"),
        Kind::Move {
            item,
            op,
            qty,
            to_team,
            person,
            memo,
            ..
        } => {
            let mut s = format!("{} {} {}개", inv.name_of(item), op.label(), qty);
            let who = [to_team.as_str(), person.as_str()]
                .iter()
                .filter(|x| !x.is_empty())
                .cloned()
                .collect::<Vec<_>>()
                .join(" ");
            if !who.is_empty() {
                s.push_str(&format!(" · {who}"));
            }
            if !memo.is_empty() {
                s.push_str(&format!(" · {memo}"));
            }
            s
        }
        Kind::Pack {
            item, phase, done, ..
        } => {
            let p = if phase == "go" { "출발" } else { "복귀" };
            let d = if *done { "체크" } else { "체크 해제" };
            format!("{} {p} {d}", inv.name_of(item))
        }
    }
}

// ---------- CSV ----------

fn csv_field(s: &str) -> String {
    if s.contains([',', '"', '\n']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

pub fn stock_csv(inv: &Inventory) -> String {
    let mut out = String::from("이름,분류,담당팀,박스,보유,대여중,가용,대여팀,메모\n");
    for it in inv.items.values().filter(|i| i.visible()) {
        let teams = it
            .out
            .iter()
            .map(|(t, n)| format!("{t} {n}"))
            .collect::<Vec<_>>()
            .join(" / ");
        let row = [
            it.name.clone(),
            it.category.clone(),
            it.owner.clone(),
            it.box_no.clone(),
            if it.uncounted { "여러 개".into() } else { format!("{}{}", it.owned, it.unit()) },
            it.out_total().to_string(),
            if it.uncounted { "-".into() } else { it.available().to_string() },
            teams,
            it.note.clone(),
        ];
        out.push_str(
            &row.iter()
                .map(|s| csv_field(s))
                .collect::<Vec<_>>()
                .join(","),
        );
        out.push('\n');
    }
    out
}

pub fn log_csv(events: &[Event], inv: &Inventory) -> String {
    let mut out = String::from("시각,기록자,기록자팀,수련회,내용\n");
    for e in events {
        let retreat = match &e.kind {
            Kind::Move { retreat, .. } | Kind::Pack { retreat, .. } => inv.retreat_name(retreat),
            _ => String::new(),
        };
        let row = [
            crate::store::fmt_time(e.at),
            e.by.clone(),
            e.team.clone(),
            retreat,
            describe(e, inv),
        ];
        out.push_str(
            &row.iter()
                .map(|s| csv_field(s))
                .collect::<Vec<_>>()
                .join(","),
        );
        out.push('\n');
    }
    out
}

pub struct ImportRow {
    pub name: String,
    pub category: String,
    pub owner: String,
    /// 숫자가 아니면(빈칸, "많음") 개수를 세지 않는 물품
    pub qty: Option<i64>,
    pub box_no: String,
    pub note: String,
}

/// "이름,분류,담당팀,수량,박스,메모" 형식. 구글 시트·엑셀에서 복사한 탭 구분도 받는다.
pub fn parse_import(text: &str) -> Vec<ImportRow> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() {
                return None;
            }
            let cols: Vec<String> = if line.contains('\t') {
                line.split('\t')
            } else {
                line.split(',')
            }
            .map(|c| c.trim().trim_matches('"').trim().to_string())
            .collect();
            let get = |i: usize| cols.get(i).cloned().unwrap_or_default();
            let name = get(0);
            if name.is_empty() || name == "이름" {
                return None;
            }
            Some(ImportRow {
                name,
                category: get(1),
                owner: get(2),
                qty: get(3).replace(',', "").parse().ok(),
                box_no: get(4),
                note: get(5),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(at: f64, kind: Kind) -> Event {
        Event {
            id: format!("e{at}"),
            at,
            by: "t".into(),
            team: "전산팀".into(),
            kind,
        }
    }

    fn mv(at: f64, op: Op, qty: i64, team: &str) -> Event {
        ev(
            at,
            Kind::Move {
                item: "mic".into(),
                op,
                qty,
                to_team: team.into(),
                person: "민수".into(),
                retreat: String::new(),
                memo: String::new(),
            },
        )
    }

    #[test]
    fn checkout_return_and_loss() {
        let events = vec![
            ev(
                1.0,
                Kind::Item {
                    item: "mic".into(),
                    name: "무선마이크".into(),
                    category: "음향·영상".into(),
                    owner: "영상팀".into(),
                    box_no: "A1".into(),
                    note: String::new(),
                    unit: String::new(),
                    uncounted: false,
                },
            ),
            mv(2.0, Op::Adjust, 10, ""),
            mv(3.0, Op::Checkout, 4, "의전팀"),
            mv(4.0, Op::Return, 2, "의전팀"),
            mv(5.0, Op::Lost, 1, "의전팀"),
        ];
        let inv = compute(&events);
        let it = &inv.items["mic"];
        assert_eq!(it.owned, 9);
        assert_eq!(it.out_of("의전팀"), 1);
        assert_eq!(it.available(), 8);
        assert!(it.visible());
    }

    #[test]
    fn event_json_roundtrip() {
        let e = mv(1.0, Op::Checkout, 2, "차량팀");
        let s = serde_json::to_string(&e).unwrap();
        assert!(s.contains("\"t\":\"move\""));
        let back: Event = serde_json::from_str(&s).unwrap();
        assert_eq!(back, e);
    }

    #[test]
    fn import_tab_and_comma() {
        let rows = parse_import("이름,분류,담당팀,수량\n마이크,음향·영상,영상팀,4,A1\n건전지\t소모품\t공용\t1,200");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].qty, Some(4));
        assert_eq!(rows[0].box_no, "A1");
        assert_eq!(rows[1].qty, Some(1200));
        assert_eq!(parse_import("테이프,기타,공용,많음")[0].qty, None);
    }
}
