//! GitHub REST API로 데이터 저장소의 events/ 폴더를 읽고 쓴다.

use base64::Engine;
use gloo_net::http::{Request, RequestBuilder, Response};
use serde::{Deserialize, Serialize};

const API: &str = "https://api.github.com";

fn auth(req: RequestBuilder, token: &str) -> RequestBuilder {
    req.header("Authorization", &format!("Bearer {}", token.trim()))
        .header("X-GitHub-Api-Version", "2022-11-28")
}

fn net_err(e: gloo_net::Error) -> String {
    format!("네트워크 연결 안 됨 ({e})")
}

async fn http_err(r: Response) -> String {
    let status = r.status();
    let body = r.text().await.unwrap_or_default();
    let msg = serde_json::from_str::<serde_json::Value>(&body)
        .ok()
        .and_then(|v| v.get("message").and_then(|m| m.as_str()).map(String::from))
        .unwrap_or_default();
    let what = match status {
        401 => "토큰이 올바르지 않거나 만료되었습니다",
        403 => "권한이 없거나 요청 한도를 넘었습니다",
        404 => "저장소를 찾을 수 없거나 토큰에 접근 권한이 없습니다",
        409 | 422 => "다른 기기와 동시에 저장되어 충돌했습니다. 다시 시도합니다",
        _ => "GitHub 오류",
    };
    format!("{what} ({status} {msg})")
}

#[derive(Deserialize)]
struct RepoInfo {
    full_name: String,
    #[serde(default)]
    private: bool,
    #[serde(default)]
    permissions: Option<Perms>,
}

#[derive(Deserialize)]
struct Perms {
    #[serde(default)]
    push: bool,
}

/// 연결 테스트: 저장소 접근과 쓰기 권한 확인
pub async fn check_repo(repo: &str, token: &str) -> Result<String, String> {
    let r = auth(Request::get(&format!("{API}/repos/{}", repo.trim())), token)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(net_err)?;
    if r.status() != 200 {
        return Err(http_err(r).await);
    }
    let info: RepoInfo = r.json().await.map_err(|e| e.to_string())?;
    let can_write = info.permissions.map(|p| p.push).unwrap_or(false);
    if !can_write {
        return Err(format!(
            "{}: 읽기만 가능합니다. 토큰에 Contents 읽기/쓰기 권한을 주세요",
            info.full_name
        ));
    }
    Ok(format!(
        "{} 연결 성공{}",
        info.full_name,
        if info.private { " (비공개 저장소)" } else { " (주의: 공개 저장소)" }
    ))
}

#[derive(Deserialize, Clone)]
pub struct Entry {
    pub name: String,
    pub sha: String,
    #[serde(rename = "type")]
    pub kind: String,
}

/// events/ 폴더 목록. 폴더가 아직 없으면 빈 목록.
pub async fn list_events(repo: &str, token: &str) -> Result<Vec<Entry>, String> {
    let url = format!(
        "{API}/repos/{}/contents/events?nocache={}",
        repo.trim(),
        js_sys::Date::now() as u64
    );
    let r = auth(Request::get(&url), token)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(net_err)?;
    match r.status() {
        200 => r.json::<Vec<Entry>>().await.map_err(|e| e.to_string()),
        404 => {
            // 폴더가 없는 것인지 저장소 자체에 접근 못 하는 것인지 구분
            check_repo(repo, token).await?;
            Ok(vec![])
        }
        _ => Err(http_err(r).await),
    }
}

/// 파일 내용을 sha로 받는다 (내용이 바뀌면 sha도 바뀌므로 캐시 문제가 없다)
pub async fn blob(repo: &str, token: &str, sha: &str) -> Result<String, String> {
    let url = format!("{API}/repos/{}/git/blobs/{sha}", repo.trim());
    let r = auth(Request::get(&url), token)
        .header("Accept", "application/vnd.github.raw+json")
        .send()
        .await
        .map_err(net_err)?;
    if r.status() != 200 {
        return Err(http_err(r).await);
    }
    r.text().await.map_err(|e| e.to_string())
}

#[derive(Serialize)]
struct PutBody<'a> {
    message: &'a str,
    content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    sha: Option<&'a str>,
}

#[derive(Deserialize)]
struct PutResp {
    content: PutContent,
}

#[derive(Deserialize)]
struct PutContent {
    sha: String,
}

/// 파일을 만들거나 덮어쓰고 새 sha를 돌려준다
pub async fn put_file(
    repo: &str,
    token: &str,
    path: &str,
    text: &str,
    sha: Option<&str>,
    message: &str,
) -> Result<String, String> {
    let url = format!("{API}/repos/{}/contents/{path}", repo.trim());
    let body = PutBody {
        message,
        content: base64::engine::general_purpose::STANDARD.encode(text.as_bytes()),
        sha,
    };
    let r = auth(Request::put(&url), token)
        .header("Accept", "application/vnd.github+json")
        .json(&body)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(net_err)?;
    if !(r.status() == 200 || r.status() == 201) {
        return Err(http_err(r).await);
    }
    let resp: PutResp = r.json().await.map_err(|e| e.to_string())?;
    Ok(resp.content.sha)
}
