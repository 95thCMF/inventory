use crate::github;
use crate::model::*;
use crate::store::*;
use gloo_storage::{LocalStorage, Storage};
use leptos::prelude::*;
use leptos::task::spawn_local;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::Duration;

// ================= 상태 =================

#[derive(Clone, Default, PartialEq)]
pub struct SyncStatus {
    pub busy: bool,
    pub last_ok: Option<f64>,
    pub error: Option<String>,
}

#[derive(Clone, Copy, PartialEq)]
pub enum Tab {
    Stock,
    Out,
    Pack,
    Log,
    Settings,
}

#[derive(Clone, PartialEq)]
pub enum Sheet {
    None,
    Item(String),
    NewItem,
    EditItem(String),
}

#[derive(Clone, Copy)]
pub struct St {
    pub cfg: RwSignal<Config>,
    pub files: RwSignal<HashMap<String, FileCache>>,
    pub pending: RwSignal<Vec<Event>>,
    pub status: RwSignal<SyncStatus>,
    again: RwSignal<bool>,
    pub events: Memo<Vec<Event>>,
    pub inv: Memo<Inventory>,
    pub toast: RwSignal<Option<String>>,
    pub tab: RwSignal<Tab>,
    pub sheet: RwSignal<Sheet>,
}

impl St {
    fn new() -> Self {
        let files = RwSignal::new(load_files());
        let pending = RwSignal::new(load_pending());
        let events = Memo::new(move |_| merge(&files.get(), &pending.get()));
        let inv = Memo::new(move |_| compute(&events.get()));
        St {
            cfg: RwSignal::new(Config::load()),
            files,
            pending,
            status: RwSignal::new(SyncStatus::default()),
            again: RwSignal::new(false),
            events,
            inv,
            toast: RwSignal::new(None),
            tab: RwSignal::new(Tab::Stock),
            sheet: RwSignal::new(Sheet::None),
        }
    }

    /// 기록을 추가하고 동기화를 시작한다
    pub fn record(&self, kinds: Vec<Kind>) {
        let c = self.cfg.get_untracked();
        let t = now();
        self.pending.update(|p| {
            for (i, kind) in kinds.into_iter().enumerate() {
                p.push(Event {
                    id: new_id(),
                    at: t + i as f64,
                    by: c.user.clone(),
                    team: c.team.clone(),
                    kind,
                });
            }
        });
        request_sync(*self);
    }

    pub fn notify(&self, msg: impl Into<String>) {
        let msg = msg.into();
        self.toast.set(Some(msg.clone()));
        let toast = self.toast;
        set_timeout(
            move || {
                if toast.get_untracked().as_deref() == Some(msg.as_str()) {
                    toast.set(None);
                }
            },
            Duration::from_millis(2500),
        );
    }

    /// 지금 선택된 수련회 (id, 이름). 선택이 없으면 가장 최근 수련회.
    pub fn retreat(&self) -> Option<(String, String)> {
        let sel = self.cfg.with(|c| c.retreat.clone());
        self.inv.with(|inv| {
            inv.retreats
                .iter()
                .find(|r| r.0 == sel)
                .or(inv.retreats.last())
                .cloned()
        })
    }

    pub fn retreat_id_untracked(&self) -> String {
        let sel = self.cfg.get_untracked().retreat;
        self.inv.with_untracked(|inv| {
            inv.retreats
                .iter()
                .find(|r| r.0 == sel)
                .or(inv.retreats.last())
                .map(|r| r.0.clone())
                .unwrap_or_default()
        })
    }
}

pub const DEFAULT_REPO: &str = "panagyric/retreat-inventory-data";

fn use_st() -> St {
    expect_context::<St>()
}

// ================= 동기화 =================

pub fn request_sync(st: St) {
    if !st.cfg.with_untracked(|c| c.can_sync()) {
        return;
    }
    if st.status.with_untracked(|s| s.busy) {
        st.again.set(true);
        return;
    }
    st.status.update(|s| s.busy = true);
    spawn_local(async move {
        loop {
            let r = sync_once(st).await;
            st.status.update(|s| match r {
                Ok(()) => {
                    s.last_ok = Some(now());
                    s.error = None;
                }
                Err(e) => s.error = Some(e),
            });
            if st.again.get_untracked() {
                st.again.set(false);
                continue;
            }
            break;
        }
        st.status.update(|s| s.busy = false);
    });
}

async fn sync_once(st: St) -> Result<(), String> {
    let cfg = st.cfg.get_untracked();
    let (repo, token) = (cfg.repo.as_str(), cfg.token.as_str());

    // 1) 다른 기기들이 쓴 기록 받기 (바뀐 파일만)
    let entries = github::list_events(repo, token).await?;
    let mut files = st.files.get_untracked();
    let mut changed = false;
    for e in entries
        .iter()
        .filter(|e| e.kind == "file" && e.name.ends_with(".jsonl"))
    {
        if files.get(&e.name).is_some_and(|f| f.sha == e.sha) {
            continue;
        }
        let text = github::blob(repo, token, &e.sha).await?;
        files.insert(
            e.name.clone(),
            FileCache {
                sha: e.sha.clone(),
                events: parse_jsonl(&text),
            },
        );
        changed = true;
    }
    if changed {
        st.files.set(files.clone());
    }

    // 2) 내 기록 올리기 (내 파일에만 덧붙이므로 다른 기기와 충돌하지 않음)
    let pending = st.pending.get_untracked();
    if pending.is_empty() {
        return Ok(());
    }
    let own = cfg.own_file();
    let mine = files.get(&own).cloned().unwrap_or_default();
    let known: HashSet<&str> = mine.events.iter().map(|e| e.id.as_str()).collect();
    let mut combined = mine.events.clone();
    combined.extend(pending.iter().filter(|e| !known.contains(e.id.as_str())).cloned());
    let sha = (!mine.sha.is_empty()).then_some(mine.sha.as_str());
    let msg = format!("{} ({}) 기록 {}건", cfg.user, cfg.team, pending.len());
    let new_sha = github::put_file(
        repo,
        token,
        &format!("events/{own}"),
        &to_jsonl(&combined),
        sha,
        &msg,
    )
    .await?;
    files.insert(
        own,
        FileCache {
            sha: new_sha,
            events: combined,
        },
    );
    let sent: HashSet<String> = pending.into_iter().map(|e| e.id).collect();
    // 파일 반영과 대기열 정리를 한 번에 (기다리는 동안 새로 생긴 기록은 남김)
    st.files.set(files);
    st.pending.update(|p| p.retain(|e| !sent.contains(&e.id)));
    Ok(())
}

// ================= 루트 =================

#[component]
pub fn App() -> impl IntoView {
    let st = St::new();
    provide_context(st);

    Effect::new(move |_| st.cfg.with(|c| c.save()));
    Effect::new(move |_| {
        st.files.with(|f| {
            let _ = LocalStorage::set("files", f);
        })
    });
    Effect::new(move |_| {
        st.pending.with(|p| {
            let _ = LocalStorage::set("pending", p);
        })
    });

    request_sync(st);
    set_interval(move || request_sync(st), Duration::from_secs(60));
    let _ = window_event_listener_untyped("online", move |_| request_sync(st));
    let _ = window_event_listener_untyped("focus", move |_| request_sync(st));

    view! {
        <Show when=move || st.cfg.with(|c| c.ready()) fallback=|| view! { <Setup/> }>
            <Header/>
            <main class="content">
                {move || match st.tab.get() {
                    Tab::Stock => view! { <StockView/> }.into_any(),
                    Tab::Out => view! { <OutView/> }.into_any(),
                    Tab::Pack => view! { <PackView/> }.into_any(),
                    Tab::Log => view! { <LogView/> }.into_any(),
                    Tab::Settings => view! { <SettingsView/> }.into_any(),
                }}
            </main>
            <Nav/>
            <SheetHost/>
        </Show>
        {move || st.toast.get().map(|m| view! { <div class="toast">{m}</div> })}
    }
}

#[component]
fn Header() -> impl IntoView {
    let st = use_st();
    let retreat = move || {
        st.retreat()
            .map(|r| r.1)
            .unwrap_or_else(|| "수련회를 설정에서 만들어 주세요".into())
    };
    let badge = move || {
        let s = st.status.get();
        let p = st.pending.with(|p| p.len());
        let can = st.cfg.with(|c| c.can_sync());
        if !can {
            ("이 기기에만 저장".to_string(), "warn")
        } else if s.busy {
            ("동기화 중…".into(), "busy")
        } else if p > 0 {
            (format!("미전송 {p}건"), "warn")
        } else if s.error.is_some() {
            ("연결 안 됨".into(), "warn")
        } else {
            ("동기화됨".into(), "ok")
        }
    };
    view! {
        <header class="top">
            <div>
                <div class="title">"수련회 물품관리"</div>
                <div class="sub">{retreat}</div>
            </div>
            <button
                class=move || format!("sync {}", badge().1)
                on:click=move |_| {
                    if st.cfg.with_untracked(|c| c.can_sync()) {
                        request_sync(st)
                    } else {
                        st.tab.set(Tab::Settings)
                    }
                }
            >
                {move || badge().0}
            </button>
        </header>
    }
}

#[component]
fn Nav() -> impl IntoView {
    let st = use_st();
    let out_count = move || {
        st.inv.with(|inv| {
            inv.items
                .values()
                .filter(|i| i.visible())
                .map(|i| i.out.len())
                .sum::<usize>()
        })
    };
    let tabs = [
        (Tab::Stock, "현황", "📦"),
        (Tab::Out, "대여중", "🤝"),
        (Tab::Pack, "짐싸기", "🧳"),
        (Tab::Log, "기록", "🕒"),
        (Tab::Settings, "설정", "⚙️"),
    ];
    view! {
        <nav class="bottom">
            {tabs
                .into_iter()
                .map(|(t, label, icon)| {
                    view! {
                        <button class:active=move || st.tab.get() == t on:click=move |_| st.tab.set(t)>
                            <span class="ic">{icon}</span>
                            <span>{label}</span>
                            {(t == Tab::Out)
                                .then(|| {
                                    view! {
                                        <Show when=move || { out_count() > 0 }>
                                            <span class="dot">{out_count}</span>
                                        </Show>
                                    }
                                })}
                        </button>
                    }
                })
                .collect_view()}
        </nav>
    }
}

// ================= 공통 입력 =================

#[component]
fn Choice(
    value: RwSignal<String>,
    options: Vec<String>,
    /// 비어 있지 않으면 맨 앞에 "선택 안 함" 항목(값 "")을 둔다
    #[prop(default = "")]
    empty: &'static str,
) -> impl IntoView {
    view! {
        <select
            prop:value=move || value.get()
            on:change=move |ev| value.set(event_target_value(&ev))
        >
            {(!empty.is_empty())
                .then(|| view! { <option value="" selected=move || value.get().is_empty()>{empty}</option> })}
            {options
                .into_iter()
                .map(|o| {
                    let o2 = o.clone();
                    let o3 = o.clone();
                    view! { <option value=o2 selected=move || value.get() == o3>{o}</option> }
                })
                .collect_view()}
        </select>
    }
}

#[component]
fn Field(label: &'static str, children: Children) -> impl IntoView {
    view! {
        <label class="field">
            <span class="lbl">{label}</span>
            {children()}
        </label>
    }
}

#[component]
fn TextInput(
    value: RwSignal<String>,
    #[prop(default = "")] placeholder: &'static str,
    #[prop(default = "text")] kind: &'static str,
) -> impl IntoView {
    view! {
        <input
            type=kind
            placeholder=placeholder
            prop:value=move || value.get()
            on:input=move |ev| value.set(event_target_value(&ev))
        />
    }
}

fn csv_href(csv: String) -> String {
    let s: String = js_sys::encode_uri_component(&format!("\u{FEFF}{csv}")).into();
    format!("data:text/csv;charset=utf-8,{s}")
}

// ================= 첫 설정 =================

fn query_param(name: &str) -> Option<String> {
    let search = window().location().search().ok()?;
    search.trim_start_matches('?').split('&').find_map(|kv| {
        let (k, v) = kv.split_once('=')?;
        (k == name).then(|| {
            js_sys::decode_uri_component(v)
                .map(String::from)
                .unwrap_or_else(|_| v.to_string())
        })
    })
}

#[component]
fn Setup() -> impl IntoView {
    let st = use_st();
    let c = st.cfg.get_untracked();
    let user = RwSignal::new(c.user.clone());
    let team = RwSignal::new(c.team.clone());
    let repo = RwSignal::new(if c.repo.is_empty() {
        query_param("repo").unwrap_or_else(|| DEFAULT_REPO.to_string())
    } else {
        c.repo.clone()
    });
    let token = RwSignal::new(c.token.clone());
    let start = move |_| {
        if user.get_untracked().trim().is_empty() || team.get_untracked().is_empty() {
            st.notify("이름과 팀을 입력하세요");
            return;
        }
        st.cfg.update(|c| {
            c.user = user.get_untracked().trim().to_string();
            c.team = team.get_untracked();
            c.repo = repo.get_untracked().trim().to_string();
            c.token = token.get_untracked().trim().to_string();
        });
        request_sync(st);
    };
    view! {
        <div class="setup">
            <h1>"수련회 물품관리"</h1>
            <p class="muted">"이 기기에서 처음 한 번만 설정합니다."</p>
            <Field label="이름">
                <TextInput value=user placeholder="예: 김민수"/>
            </Field>
            <Field label="소속 팀">
                <Choice value=team options=team_options() empty="팀 선택"/>
            </Field>
            <Field label="데이터 저장소 (아이디/저장소이름)">
                <TextInput value=repo placeholder="panagyric/retreat-inventory-data"/>
            </Field>
            <Field label="접근 토큰">
                <TextInput value=token kind="password" placeholder="관리자에게 받은 토큰"/>
            </Field>
            <p class="muted small">
                "토큰이 없으면 이 기기에만 저장되고, 나중에 설정에서 입력하면 그때 동기화됩니다."
            </p>
            <button class="btn primary block" on:click=start>
                "시작하기"
            </button>
        </div>
    }
}

// ================= 현황 =================

#[component]
fn StockView() -> impl IntoView {
    let st = use_st();
    let q = RwSignal::new(String::new());
    let team = RwSignal::new(String::new());
    let cat = RwSignal::new(String::new());
    let low_only = RwSignal::new(false);

    let list = move || {
        let q = q.get().trim().to_lowercase();
        let team = team.get();
        let cat = cat.get();
        let low = low_only.get();
        st.inv.with(|inv| {
            let mut v: Vec<ItemState> = inv
                .items
                .values()
                .filter(|i| i.visible())
                .filter(|i| {
                    q.is_empty()
                        || i.name.to_lowercase().contains(&q)
                        || i.box_no.to_lowercase().contains(&q)
                        || i.note.to_lowercase().contains(&q)
                })
                .filter(|i| team.is_empty() || i.owner == team)
                .filter(|i| cat.is_empty() || i.category == cat)
                .filter(|i| !low || i.available() <= 0)
                .cloned()
                .collect();
            v.sort_by(|a, b| a.category.cmp(&b.category).then(a.name.cmp(&b.name)));
            v
        })
    };

    view! {
        <div class="toolbar">
            <input
                class="search"
                type="search"
                placeholder="물품 이름·박스·메모 검색"
                prop:value=move || q.get()
                on:input=move |ev| q.set(event_target_value(&ev))
            />
            <div class="row">
                <Choice value=team options=owner_options() empty="전체 담당팀"/>
                <Choice value=cat options=category_options() empty="전체 분류"/>
            </div>
            <label class="check">
                <input
                    type="checkbox"
                    prop:checked=move || low_only.get()
                    on:change=move |ev| low_only.set(event_target_checked(&ev))
                />
                "가용 0개 이하만"
            </label>
        </div>
        <button class="btn primary block" on:click=move |_| st.sheet.set(Sheet::NewItem)>
            "+ 물품 추가"
        </button>
        {move || {
            let v = list();
            if v.is_empty() {
                view! { <p class="empty">"표시할 물품이 없습니다."</p> }.into_any()
            } else {
                let count = v.len();
                view! {
                    <p class="muted small">{format!("{count}개 품목")}</p>
                    <ul class="list">{v.into_iter().map(|i| item_row(st, i)).collect_view()}</ul>
                }
                    .into_any()
            }
        }}
    }
}

fn item_row(st: St, i: ItemState) -> impl IntoView {
    let id = i.id.clone();
    let out = i.out_total();
    let avail = i.available();
    let zero = avail <= 0;
    let meta = [i.category.as_str(), i.owner.as_str(), i.box_no.as_str()]
        .iter()
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect::<Vec<_>>()
        .join(" · ");
    view! {
        <li class="card item" on:click=move |_| st.sheet.set(Sheet::Item(id.clone()))>
            <div class="main">
                <div class="name">{i.name.clone()}</div>
                <div class="meta">{meta}</div>
            </div>
            <div class="nums">
                <div class="avail" class:zero=zero>
                    {avail}
                    <span class="unit">"가용"</span>
                </div>
                <div class="small muted">{format!("보유 {}", i.owned)}</div>
                {(out > 0).then(|| view! { <div class="tag out">{format!("대여 {out}")}</div> })}
            </div>
        </li>
    }
}

// ================= 물품 상세 / 작업 =================

#[component]
fn SheetHost() -> impl IntoView {
    let st = use_st();
    move || match st.sheet.get() {
        Sheet::None => ().into_any(),
        Sheet::Item(id) => view! {
            <Modal>
                <ItemDetail id=id/>
            </Modal>
        }
        .into_any(),
        Sheet::NewItem => view! {
            <Modal>
                <ItemForm id=None/>
            </Modal>
        }
        .into_any(),
        Sheet::EditItem(id) => view! {
            <Modal>
                <ItemForm id=Some(id)/>
            </Modal>
        }
        .into_any(),
    }
}

#[component]
fn Modal(children: Children) -> impl IntoView {
    let st = use_st();
    view! {
        <div class="overlay" on:click=move |_| st.sheet.set(Sheet::None)>
            <div class="sheet" on:click=|ev| ev.stop_propagation()>
                <button class="close" on:click=move |_| st.sheet.set(Sheet::None)>
                    "✕"
                </button>
                {children()}
            </div>
        </div>
    }
}

#[component]
fn ItemDetail(id: String) -> impl IntoView {
    let st = use_st();
    let my_team = st.cfg.get_untracked().team;
    let idv = StoredValue::new(id);
    let item = Memo::new(move |_| st.inv.with(|inv| inv.items.get(&idv.get_value()).cloned()));

    let op = RwSignal::new(Op::Checkout);
    let qty = RwSignal::new("1".to_string());
    let team = RwSignal::new(my_team);
    let person = RwSignal::new(st.cfg.get_untracked().user);
    let memo = RwSignal::new(String::new());

    let submit = move |_| {
        let Some(it) = item.get_untracked() else { return };
        let o = op.get_untracked();
        let n: i64 = qty.get_untracked().trim().parse().unwrap_or(0);
        if n == 0 || (o != Op::Adjust && n < 0) {
            st.notify("수량을 확인하세요");
            return;
        }
        let t = team.get_untracked();
        if o.needs_team() && t.is_empty() {
            st.notify("팀을 선택하세요");
            return;
        }
        match o {
            Op::Checkout if n > it.available() => {
                st.notify(format!("가용 수량({}개)보다 많습니다", it.available()));
                return;
            }
            Op::Return if n > it.out_of(&t) => {
                st.notify(format!("{t} 대여 수량은 {}개입니다", it.out_of(&t)));
                return;
            }
            _ => {}
        }
        st.record(vec![Kind::Move {
            item: it.id.clone(),
            op: o,
            qty: n,
            to_team: t,
            person: person.get_untracked().trim().to_string(),
            retreat: st.retreat_id_untracked(),
            memo: memo.get_untracked().trim().to_string(),
        }]);
        st.notify(format!("{} {} {}개 기록했습니다", it.name, o.label(), n));
        qty.set("1".into());
        memo.set(String::new());
    };

    let delete = move |_| {
        let Some(it) = item.get_untracked() else { return };
        let ok = window()
            .confirm_with_message(&format!("'{}'을(를) 삭제할까요? 기록은 남습니다.", it.name))
            .unwrap_or(false);
        if ok {
            st.record(vec![Kind::ItemDelete { item: it.id.clone() }]);
            st.sheet.set(Sheet::None);
            st.notify("삭제했습니다");
        }
    };

    let history = move || {
        let id = idv.get_value();
        let inv = st.inv.get();
        st.events.with(|evs| {
            evs.iter()
                .rev()
                .filter(|e| e.item_id() == Some(id.as_str()))
                .take(40)
                .map(|e| {
                    view! {
                        <li>
                            <span class="time">{fmt_time(e.at)}</span>
                            <span>{describe(e, &inv)}</span>
                            <span class="muted small">{format!(" — {}", e.by)}</span>
                        </li>
                    }
                })
                .collect_view()
        })
    };

    move || {
        let Some(it) = item.get() else {
            return view! { <p>"물품을 찾을 수 없습니다."</p> }.into_any();
        };
        let id_edit = it.id.clone();
        let outs = it.out.clone();
        let who = it.who.clone();
        view! {
            <h2>{it.name.clone()}</h2>
            <p class="muted">
                {[it.category.clone(), it.owner.clone(), it.box_no.clone()]
                    .into_iter()
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
                    .join(" · ")}
            </p>
            {(!it.note.is_empty()).then(|| view! { <p class="note">{it.note.clone()}</p> })}
            <div class="stats">
                <div>
                    <b>{it.owned}</b>
                    <span>"보유"</span>
                </div>
                <div>
                    <b>{it.out_total()}</b>
                    <span>"대여중"</span>
                </div>
                <div class:zero=it.available() <= 0>
                    <b>{it.available()}</b>
                    <span>"가용"</span>
                </div>
            </div>
            {(!outs.is_empty())
                .then(|| {
                    view! {
                        <ul class="outs">
                            {outs
                                .into_iter()
                                .map(|(t, n)| {
                                    let people = who.get(&t).map(|v| v.join(", ")).unwrap_or_default();
                                    view! {
                                        <li>
                                            <b>{t}</b>
                                            {format!(" {n}개 ")}
                                            <span class="muted small">{people}</span>
                                        </li>
                                    }
                                })
                                .collect_view()}
                        </ul>
                    }
                })}

            <h3>"기록하기"</h3>
            <div class="ops">
                {Op::ALL
                    .into_iter()
                    .map(|o| {
                        view! {
                            <button class:active=move || op.get() == o on:click=move |_| op.set(o)>
                                {o.label()}
                            </button>
                        }
                    })
                    .collect_view()}
            </div>
            <p class="muted small">{move || op.get().hint()}</p>
            <div class="row">
                <Field label="수량">
                    <input
                        type="number"
                        inputmode="numeric"
                        prop:value=move || qty.get()
                        on:input=move |ev| qty.set(event_target_value(&ev))
                    />
                </Field>
                <Field label="팀">
                    <Choice value=team options=team_options() empty="팀 없음"/>
                </Field>
            </div>
            <div class="row">
                <Field label="담당자">
                    <TextInput value=person placeholder="이름"/>
                </Field>
                <Field label="메모">
                    <TextInput value=memo placeholder="선택"/>
                </Field>
            </div>
            <button class="btn primary block" on:click=submit>
                {move || format!("{} 기록", op.get().label())}
            </button>

            <h3>"이력"</h3>
            <ul class="history">{history}</ul>

            <div class="row end">
                <button class="btn" on:click=move |_| st.sheet.set(Sheet::EditItem(id_edit.clone()))>
                    "정보 수정"
                </button>
                <button class="btn danger" on:click=delete>
                    "삭제"
                </button>
            </div>
        }
            .into_any()
    }
}

#[component]
fn ItemForm(id: Option<String>) -> impl IntoView {
    let st = use_st();
    let existing = id
        .as_ref()
        .and_then(|i| st.inv.with_untracked(|inv| inv.items.get(i).cloned()));
    let is_new = existing.is_none();
    let e = existing.unwrap_or_default();
    let name = RwSignal::new(e.name.clone());
    let category = RwSignal::new(if e.category.is_empty() {
        CATEGORIES[0].to_string()
    } else {
        e.category.clone()
    });
    let owner = RwSignal::new(if e.owner.is_empty() {
        st.cfg.get_untracked().team
    } else {
        e.owner.clone()
    });
    let qty = RwSignal::new("1".to_string());
    let box_no = RwSignal::new(e.box_no.clone());
    let note = RwSignal::new(e.note.clone());
    let idv = StoredValue::new(id);

    let save = move |_| {
        let n = name.get_untracked().trim().to_string();
        if n.is_empty() {
            st.notify("이름을 입력하세요");
            return;
        }
        let item_id = idv.get_value().unwrap_or_else(new_id);
        let mut kinds = vec![Kind::Item {
            item: item_id.clone(),
            name: n.clone(),
            category: category.get_untracked(),
            owner: owner.get_untracked(),
            box_no: box_no.get_untracked().trim().to_string(),
            note: note.get_untracked().trim().to_string(),
        }];
        if is_new {
            let q: i64 = qty.get_untracked().trim().parse().unwrap_or(0);
            if q != 0 {
                kinds.push(Kind::Move {
                    item: item_id.clone(),
                    op: Op::Adjust,
                    qty: q,
                    to_team: String::new(),
                    person: String::new(),
                    retreat: st.retreat_id_untracked(),
                    memo: "초기 수량".into(),
                });
            }
        }
        st.record(kinds);
        st.notify(if is_new { "등록했습니다" } else { "수정했습니다" });
        st.sheet.set(Sheet::Item(item_id));
    };

    view! {
        <h2>{if is_new { "물품 추가" } else { "물품 정보 수정" }}</h2>
        <Field label="이름">
            <TextInput value=name placeholder="예: 무선 마이크"/>
        </Field>
        <div class="row">
            <Field label="분류">
                <Choice value=category options=category_options()/>
            </Field>
            <Field label="담당팀">
                <Choice value=owner options=owner_options()/>
            </Field>
        </div>
        <div class="row">
            {is_new
                .then(|| {
                    view! {
                        <Field label="보유 수량">
                            <input
                                type="number"
                                inputmode="numeric"
                                prop:value=move || qty.get()
                                on:input=move |ev| qty.set(event_target_value(&ev))
                            />
                        </Field>
                    }
                })}
            <Field label="박스/보관 위치">
                <TextInput value=box_no placeholder="예: A-3"/>
            </Field>
        </div>
        <Field label="메모">
            <TextInput value=note placeholder="선택"/>
        </Field>
        {(!is_new)
            .then(|| view! { <p class="muted small">"수량은 상세 화면의 '수량조정'으로 바꿉니다."</p> })}
        <button class="btn primary block" on:click=save>
            "저장"
        </button>
    }
}

// ================= 대여중 =================

#[component]
fn OutView() -> impl IntoView {
    let st = use_st();
    let mine_only = RwSignal::new(false);

    let groups = move || {
        let my = st.cfg.with(|c| c.team.clone());
        let mine = mine_only.get();
        st.inv.with(|inv| {
            let mut g: BTreeMap<String, Vec<(ItemState, i64)>> = BTreeMap::new();
            for it in inv.items.values().filter(|i| i.visible()) {
                for (t, n) in &it.out {
                    if *n > 0 && (!mine || *t == my) {
                        g.entry(t.clone()).or_default().push((it.clone(), *n));
                    }
                }
            }
            g
        })
    };

    let return_all = move |item: String, team: String, n: i64, name: String| {
        let ok = window()
            .confirm_with_message(&format!("{team}의 {name} {n}개를 모두 반납 처리할까요?"))
            .unwrap_or(false);
        if ok {
            st.record(vec![Kind::Move {
                item,
                op: Op::Return,
                qty: n,
                to_team: team,
                person: st.cfg.get_untracked().user,
                retreat: st.retreat_id_untracked(),
                memo: String::new(),
            }]);
            st.notify("반납 처리했습니다");
        }
    };

    view! {
        <div class="toolbar">
            <label class="check">
                <input
                    type="checkbox"
                    prop:checked=move || mine_only.get()
                    on:change=move |ev| mine_only.set(event_target_checked(&ev))
                />
                "우리 팀만 보기"
            </label>
        </div>
        {move || {
            let g = groups();
            if g.is_empty() {
                return view! { <p class="empty">"대여 중인 물품이 없습니다. 👍"</p> }.into_any();
            }
            g.into_iter()
                .map(|(team, rows)| {
                    let total: i64 = rows.iter().map(|r| r.1).sum();
                    view! {
                        <section class="group">
                            <h3>{team.clone()} <span class="muted small">{format!(" {total}개")}</span></h3>
                            <ul class="list">
                                {rows
                                    .into_iter()
                                    .map(|(it, n)| {
                                        let people = it.who.get(&team).map(|v| v.join(", ")).unwrap_or_default();
                                        let id = it.id.clone();
                                        let (id2, t2, name2) = (it.id.clone(), team.clone(), it.name.clone());
                                        view! {
                                            <li class="card item">
                                                <div class="main" on:click=move |_| st.sheet.set(Sheet::Item(id.clone()))>
                                                    <div class="name">{it.name.clone()}</div>
                                                    <div class="meta">{people}</div>
                                                </div>
                                                <div class="nums">
                                                    <div class="avail">{n}<span class="unit">"개"</span></div>
                                                    <button
                                                        class="btn small"
                                                        on:click=move |_| return_all(id2.clone(), t2.clone(), n, name2.clone())
                                                    >
                                                        "전체 반납"
                                                    </button>
                                                </div>
                                            </li>
                                        }
                                    })
                                    .collect_view()}
                            </ul>
                        </section>
                    }
                })
                .collect_view()
                .into_any()
        }}
    }
}

// ================= 짐싸기 =================

#[component]
fn PackView() -> impl IntoView {
    let st = use_st();
    let team = RwSignal::new(String::new());
    let remaining = RwSignal::new(false);

    move || {
        let Some((rid, rname)) = st.retreat() else {
            return view! {
                <div class="empty">
                    <p>"아직 수련회가 없습니다."</p>
                    <button class="btn primary" on:click=move |_| st.tab.set(Tab::Settings)>
                        "설정에서 수련회 만들기"
                    </button>
                </div>
            }
            .into_any();
        };
        let rid = StoredValue::new(rid);
        let toggle = move |item: String, phase: &'static str, done: bool| {
            st.record(vec![Kind::Pack {
                retreat: rid.get_value(),
                item,
                phase: phase.into(),
                done,
            }]);
        };
        let content = move || {
            let team = team.get();
            let rem = remaining.get();
            let r = rid.get_value();
            st.inv.with(|inv| {
                let all: Vec<&ItemState> = inv
                    .items
                    .values()
                    .filter(|i| i.visible())
                    .filter(|i| team.is_empty() || i.owner == team)
                    .collect();
                let total = all.len();
                let go = all.iter().filter(|i| inv.packed(&r, &i.id, "go")).count();
                let back = all.iter().filter(|i| inv.packed(&r, &i.id, "back")).count();
                let mut boxes: BTreeMap<String, Vec<(ItemState, bool, bool)>> = BTreeMap::new();
                for i in all {
                    let g = inv.packed(&r, &i.id, "go");
                    let b = inv.packed(&r, &i.id, "back");
                    if rem && g && b {
                        continue;
                    }
                    let key = if i.box_no.is_empty() { "박스 미지정".to_string() } else { i.box_no.clone() };
                    boxes.entry(key).or_default().push(((*i).clone(), g, b));
                }
                let pct = |n: usize| if total == 0 { 0 } else { n * 100 / total };
                view! {
                    <div class="progress">
                        <div>
                            <span>{format!("출발 {go}/{total}")}</span>
                            <div class="bar"><i style=format!("width:{}%", pct(go))></i></div>
                        </div>
                        <div>
                            <span>{format!("복귀 {back}/{total}")}</span>
                            <div class="bar back"><i style=format!("width:{}%", pct(back))></i></div>
                        </div>
                    </div>
                    {boxes
                        .into_iter()
                        .map(|(bx, rows)| {
                            view! {
                                <section class="group">
                                    <h3>{bx}</h3>
                                    <ul class="list">
                                        {rows
                                            .into_iter()
                                            .map(|(it, g, b)| {
                                                let (i1, i2) = (it.id.clone(), it.id.clone());
                                                view! {
                                                    <li class="card pack">
                                                        <div class="main">
                                                            <div class="name">{it.name.clone()}</div>
                                                            <div class="meta">{format!("{} · {}개", it.owner, it.owned)}</div>
                                                        </div>
                                                        <label class="tick" class:on=g>
                                                            <input type="checkbox" prop:checked=g on:change=move |ev| toggle(i1.clone(), "go", event_target_checked(&ev))/>
                                                            "출발"
                                                        </label>
                                                        <label class="tick back" class:on=b>
                                                            <input type="checkbox" prop:checked=b on:change=move |ev| toggle(i2.clone(), "back", event_target_checked(&ev))/>
                                                            "복귀"
                                                        </label>
                                                    </li>
                                                }
                                            })
                                            .collect_view()}
                                    </ul>
                                </section>
                            }
                        })
                        .collect_view()}
                }
            })
        };
        view! {
            <h2 class="page">{format!("{rname} 체크리스트")}</h2>
            <div class="toolbar">
                <Choice value=team options=owner_options() empty="전체 담당팀"/>
                <label class="check">
                    <input
                        type="checkbox"
                        prop:checked=move || remaining.get()
                        on:change=move |ev| remaining.set(event_target_checked(&ev))
                    />
                    "남은 것만 보기"
                </label>
            </div>
            {content}
        }
        .into_any()
    }
}

// ================= 기록 =================

#[component]
fn LogView() -> impl IntoView {
    let st = use_st();
    let team = RwSignal::new(String::new());
    let limit = RwSignal::new(100usize);
    let rows = move || {
        let team = team.get();
        let inv = st.inv.get();
        let pending: HashSet<String> = st.pending.with(|p| p.iter().map(|e| e.id.clone()).collect());
        st.events.with(|evs| {
            evs.iter()
                .rev()
                .filter(|e| team.is_empty() || e.team == team)
                .take(limit.get())
                .map(|e| {
                    let waiting = pending.contains(&e.id);
                    view! {
                        <li class:waiting=waiting>
                            <div>{describe(e, &inv)}</div>
                            <div class="muted small">
                                {format!("{} · {} {}", fmt_time(e.at), e.team, e.by)}
                                {waiting.then_some(" · 미전송")}
                            </div>
                        </li>
                    }
                })
                .collect_view()
        })
    };
    view! {
        <div class="toolbar">
            <Choice value=team options=team_options() empty="전체 팀"/>
        </div>
        <ul class="log">{rows}</ul>
        <button class="btn block" on:click=move |_| limit.update(|l| *l += 200)>
            "더 보기"
        </button>
    }
}

// ================= 설정 =================

#[component]
fn SettingsView() -> impl IntoView {
    let st = use_st();
    let c = st.cfg.get_untracked();
    let user = RwSignal::new(c.user.clone());
    let team = RwSignal::new(c.team.clone());
    let repo = RwSignal::new(c.repo.clone());
    let token = RwSignal::new(c.token.clone());
    let test_msg = RwSignal::new(String::new());
    let new_retreat = RwSignal::new(String::new());
    let import_text = RwSignal::new(String::new());
    let retreat_sel = RwSignal::new(st.retreat_id_untracked());

    Effect::new(move |_| {
        let r = retreat_sel.get();
        if st.cfg.with_untracked(|c| c.retreat != r) {
            st.cfg.update(|c| c.retreat = r);
        }
    });

    let save_me = move |_| {
        if user.get_untracked().trim().is_empty() || team.get_untracked().is_empty() {
            st.notify("이름과 팀을 입력하세요");
            return;
        }
        st.cfg.update(|c| {
            c.user = user.get_untracked().trim().to_string();
            c.team = team.get_untracked();
        });
        st.notify("저장했습니다");
    };

    let save_sync = move |_| {
        let r = repo.get_untracked().trim().to_string();
        if !r.is_empty() && !valid_repo(&r) {
            st.notify("저장소는 '아이디/저장소이름' 형식입니다");
            return;
        }
        st.cfg.update(|c| {
            c.repo = r;
            c.token = token.get_untracked().trim().to_string();
        });
        st.notify("저장했습니다");
        request_sync(st);
    };

    let test = move |_| {
        let (r, t) = (repo.get_untracked(), token.get_untracked());
        test_msg.set("확인 중…".into());
        spawn_local(async move {
            let msg = match github::check_repo(&r, &t).await {
                Ok(m) => format!("✅ {m}"),
                Err(e) => format!("❌ {e}"),
            };
            test_msg.set(msg);
        });
    };

    let add_retreat = move |_| {
        let n = new_retreat.get_untracked().trim().to_string();
        if n.is_empty() {
            st.notify("수련회 이름을 입력하세요");
            return;
        }
        let id = new_id();
        st.record(vec![Kind::Retreat {
            retreat: id.clone(),
            name: n,
        }]);
        new_retreat.set(String::new());
        retreat_sel.set(id);
        st.notify("수련회를 만들었습니다");
    };

    let do_import = move |_| {
        let rows = parse_import(&import_text.get_untracked());
        if rows.is_empty() {
            st.notify("가져올 줄이 없습니다");
            return;
        }
        let existing: HashSet<String> = st.inv.with_untracked(|inv| {
            inv.items
                .values()
                .filter(|i| i.visible())
                .map(|i| i.name.clone())
                .collect()
        });
        let my_team = st.cfg.get_untracked().team;
        let retreat = st.retreat_id_untracked();
        let mut kinds = vec![];
        let mut skipped = 0;
        for r in rows {
            if existing.contains(&r.name) {
                skipped += 1;
                continue;
            }
            let id = new_id();
            let category = if CATEGORIES.contains(&r.category.as_str()) { r.category } else { "기타".into() };
            let owner = if owner_options().contains(&r.owner) { r.owner } else { my_team.clone() };
            kinds.push(Kind::Item {
                item: id.clone(),
                name: r.name,
                category,
                owner,
                box_no: r.box_no,
                note: r.note,
            });
            if r.qty != 0 {
                kinds.push(Kind::Move {
                    item: id,
                    op: Op::Adjust,
                    qty: r.qty,
                    to_team: String::new(),
                    person: String::new(),
                    retreat: retreat.clone(),
                    memo: "가져오기".into(),
                });
            }
        }
        let added = kinds.iter().filter(|k| matches!(k, Kind::Item { .. })).count();
        if !kinds.is_empty() {
            st.record(kinds);
        }
        import_text.set(String::new());
        st.notify(format!("{added}개 등록, 이름이 같은 {skipped}개는 건너뜀"));
    };

    let refetch = move |_| {
        let ok = window()
            .confirm_with_message("GitHub에서 모든 기록을 다시 받습니다. 미전송 기록은 유지됩니다.")
            .unwrap_or(false);
        if ok {
            st.files.set(HashMap::new());
            request_sync(st);
        }
    };

    let share_link = move || {
        let loc = window().location();
        let base = format!(
            "{}{}",
            loc.origin().unwrap_or_default(),
            loc.pathname().unwrap_or_default()
        );
        let r = st.cfg.with(|c| c.repo.clone());
        if r.is_empty() {
            base
        } else {
            format!("{base}?repo={r}")
        }
    };

    view! {
        <section class="card pad">
            <h3>"내 정보"</h3>
            <Field label="이름">
                <TextInput value=user/>
            </Field>
            <Field label="소속 팀">
                <Choice value=team options=team_options()/>
            </Field>
            <button class="btn" on:click=save_me>
                "저장"
            </button>
        </section>

        <section class="card pad">
            <h3>"수련회"</h3>
            {move || {
                let opts: Vec<String> = st.inv.with(|i| i.retreats.iter().map(|r| r.0.clone()).collect());
                if opts.is_empty() {
                    view! { <p class="muted">"아직 없습니다. 아래에서 만드세요."</p> }.into_any()
                } else {
                    let names: Vec<(String, String)> = st.inv.with(|i| i.retreats.clone());
                    view! {
                        <Field label="이 기기에서 볼 수련회">
                            <select
                                prop:value=move || retreat_sel.get()
                                on:change=move |ev| retreat_sel.set(event_target_value(&ev))
                            >
                                {names
                                    .into_iter()
                                    .map(|(id, name)| {
                                        let id2 = id.clone();
                                        view! { <option value=id selected=move || retreat_sel.get() == id2>{name}</option> }
                                    })
                                    .collect_view()}
                            </select>
                        </Field>
                    }
                        .into_any()
                }
            }}
            <div class="row">
                <TextInput value=new_retreat placeholder="예: 2027 여름수련회"/>
                <button class="btn" on:click=add_retreat>
                    "새로 만들기"
                </button>
            </div>
        </section>

        <section class="card pad">
            <h3>"동기화"</h3>
            <p class="muted small">
                {move || {
                    let s = st.status.get();
                    let p = st.pending.with(|p| p.len());
                    let last = s.last_ok.map(fmt_time).unwrap_or_else(|| "없음".into());
                    format!("마지막 동기화: {last} · 미전송 {p}건")
                }}
            </p>
            {move || st.status.get().error.map(|e| view! { <p class="error small">{e}</p> })}
            <Field label="데이터 저장소 (아이디/저장소이름)">
                <TextInput value=repo placeholder="panagyric/retreat-inventory-data"/>
            </Field>
            <Field label="접근 토큰">
                <TextInput value=token kind="password"/>
            </Field>
            <div class="row">
                <button class="btn" on:click=test>
                    "연결 테스트"
                </button>
                <button class="btn primary" on:click=save_sync>
                    "저장 후 동기화"
                </button>
            </div>
            <p class="small">{move || test_msg.get()}</p>
            <Field label="다른 사람에게 보낼 주소 (토큰은 포함되지 않음)">
                <input readonly prop:value=share_link/>
            </Field>
        </section>

        <section class="card pad">
            <h3>"내보내기 / 가져오기"</h3>
            <div class="row">
                <a class="btn" download="수련회_재고.csv" href=move || st.inv.with(|i| csv_href(stock_csv(i)))>
                    "재고 CSV"
                </a>
                <a
                    class="btn"
                    download="수련회_기록.csv"
                    href=move || st.inv.with(|i| st.events.with(|e| csv_href(log_csv(e, i))))
                >
                    "기록 CSV"
                </a>
            </div>
            <Field label="물품 한꺼번에 등록 (이름, 분류, 담당팀, 수량, 박스, 메모)">
                <textarea
                    rows="6"
                    placeholder="무선마이크,음향·영상,영상팀,4,A-1\n건전지 AA,소모품,공용,40,B-2\n\n구글 시트나 엑셀에서 복사해 붙여넣어도 됩니다."
                    prop:value=move || import_text.get()
                    on:input=move |ev| import_text.set(event_target_value(&ev))
                ></textarea>
            </Field>
            <button class="btn" on:click=do_import>
                "등록"
            </button>
        </section>

        <section class="card pad">
            <h3>"기기"</h3>
            <p class="muted small">{format!("기기 ID: {}", c.device)}</p>
            <button class="btn" on:click=refetch>
                "전체 다시 받기"
            </button>
        </section>
    }
}
