# 수련회 물품관리

휴대폰과 노트북에서 쓰는 수련회 물품 대여·반납·짐싸기 관리 앱입니다.
Rust(Leptos)로 만든 웹앱을 GitHub Pages에 올리고, 데이터는 비공개 GitHub 저장소에 JSON 기록으로 저장합니다.

- 서버 비용 없음 (GitHub만 사용)
- 인터넷이 끊겨도 기록되고, 연결되면 자동으로 올라감
- 기기마다 자기 기록 파일(`events/<기기ID>.jsonl`)에만 덧붙여서, 여러 명이 동시에 써도 충돌하지 않음
- 홈 화면에 추가하면 앱처럼 실행 (PWA)

## 화면

| 탭 | 내용 |
|---|---|
| 현황 | 물품 검색, 담당팀·분류 필터, 보유/대여중/가용 수량, 물품 추가 |
| 대여중 | 팀별 대여 물품과 가져간 사람, 전체 반납 |
| 짐싸기 | 수련회별 출발·복귀 체크리스트 (박스별) |
| 기록 | 모든 기록 (누가, 언제, 무엇을) |
| 설정 | 내 정보, 수련회 선택·생성, 동기화, CSV 내보내기·가져오기 |

물품을 누르면 대여 / 반납 / 소모 / 분실 / 파손 / 수량조정을 기록할 수 있습니다.

## 처음 준비 (관리자 1회)

1. **앱 저장소** (`95thCMF/inventory`, 공개): 이 코드를 올립니다.
   Settings → Pages → Source를 **GitHub Actions**로 바꾸면 `main`에 push할 때마다 자동 배포됩니다.
   주소: https://95thcmf.github.io/inventory/
2. **데이터 저장소** (`95thCMF/inventory-data`, **비공개**): README만 있는 빈 저장소로 만듭니다.
3. **토큰**: GitHub → Settings → Developer settings → Personal access tokens → Fine-grained tokens
   - Repository access: `inventory-data` 하나만
   - Permissions: **Contents: Read and write** (나머지는 없음)
   - 만료일: 수련회가 끝난 뒤로
4. 앱 설정 화면의 "다른 사람에게 보낼 주소"를 봉사자에게 보내고, 토큰은 따로 전달합니다.
   토큰이 새면 GitHub에서 폐기하고 새로 발급하면 됩니다.

## 개발

```bash
rustup target add wasm32-unknown-unknown
cargo install trunk --locked
trunk serve          # http://127.0.0.1:8080
cargo test           # 재고 계산 테스트
```

## 데이터 형식

`events/<기기ID>.jsonl` 한 줄이 기록 한 건입니다. 현재 수량은 모든 기록을 시간순으로 합산해 계산합니다.

```json
{"id":"…","at":1784530980000,"by":"김민수","team":"의전팀","t":"move","item":"…","op":"checkout","qty":2,"to_team":"의전팀","person":"김민수","retreat":"…","memo":""}
```

종류(`t`): `item`(물품 등록·수정), `item_delete`, `retreat`(수련회 생성), `move`(대여·반납·소모·분실·파손·수량조정), `pack`(짐싸기 체크)
