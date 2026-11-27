# Project Status — v0.1.0（含未發佈的第 1、2 階段切片）

版本號仍為 0.1.0。下方「v0.1.0 可用功能」是已發佈的 MVP；「第 1 階段切片（Unreleased）」與「第 2 階段切片（Unreleased / Phase 2a、2b、2c、2d）」是依 [ADR 0010](docs/adr/0010-migration-phases.md) 第 1、2 階段加入、尚未發佈的部分。

## v0.1.0 可用功能

- Prompt → 本地規則／OpenAI-compatible LLM → CadDocument 1.0。
- Web UI、REST API、CLI。
- 可編輯 JSON 後重新驗證與輸出。
- Plate、cylinder、ring、L bracket、open enclosure。
- Through、blind、clearance、tapped approximation、counterbore、countersink。
- X／Y／Z 軸孔、fillet、chamfer。
- CadQuery STEP／STL／DXF／SVG 與 OpenSCAD STL fallback。
- 幾何驗證閘門、Token、下載路徑保護與工作 ZIP。

## 第 1 階段切片（Unreleased / Phase 1a）

| 項目 | 位置 | 狀態 |
|---|---|---|
| React + TypeScript + Vite 前端（已取代 `app/static/`） | `web/` | 已實作；建置輸出到 `app/web_dist/` |
| FastAPI 提供新前端 | `app/main.py`、`app/core/config.py`（`frontend_dir`） | `app/web_dist/index.html` 存在時於 `/` 提供；否則 `/` 回傳 503 說明頁（`app/static/` 已移除） |
| Three.js STL 檢視器（最小版，不依賴 React） | `web/src/viewer/` | 已實作；GLB 與元件選取未實作 |
| Pydantic → JSON Schema → TypeScript 型別管線 | `scripts/export_schema.py`、`schemas/promptcad.schema.json`、`web/src/api/generated/` | 已實作；CI 檢查漂移 |
| Tauri v2 桌面外殼 + FastAPI sidecar | `src-tauri/`、`app/sidecar.py`、`scripts/build_sidecar.py` | 已實作（Windows／MSVC）；sidecar 以 PyInstaller 打包 |
| CadDocument 2.0 規格凍結 | `docs/CAD_DSL_2.md`、`examples/cad2/`、`tests/test_cad2_examples.py` | 已凍結；程式依 ADR 0010 在第 2 階段以 Rust 實作 |
| 桌面版內含 CadQuery（第 1 階段 1b） | `app/render_worker.py`、`app/services/renderer.py`、`scripts/{desktop_env,build_sidecar,frozen_smoke}.py`、`src-tauri/` | 已實作：PyInstaller onedir sidecar 經 Tauri `resources` 安裝；凍結後以 `--render-worker` 執行 `model.py`，桌面版可輸出 STEP／STL／DXF／SVG |
| 移除 `app/static/`（負責人核准） | `app/main.py`、`Dockerfile`、`.github/workflows/ci.yml`（`docker` job） | 已完成；Dockerfile 多階段建置前端，CI 實際建置映像並確認 `/` 回傳新前端 |
| 乾淨機器驗證（隔離環境 smoke、Job Object 終止檢查） | `scripts/frozen_smoke.py --isolated-env`、`scripts/frozen_kill_check.py`、`.github/workflows/desktop-bundle.yml` | 已實作；在 GitHub `windows-latest` 上執行 |
| 第三方授權聲明 | `scripts/gen_third_party_notices.py`、`THIRD_PARTY_NOTICES.md`、`src-tauri/tauri.conf.json` | 已完成；隨安裝包安裝到 `<安裝目錄>\THIRD_PARTY_NOTICES.md` |
| 2D 預覽可讀性 | `web/src/components/SvgPreview.tsx`、`web/src/styles.css` | 已完成；`model.svg` 以紙色底顯示，`preview.svg` 不變 |
| 三平台桌面版（ADR 0011） | `src-tauri/tauri.{macos,linux}.conf.json`、`src-tauri/src/sidecar.rs`、`app/parent_watch.py`、`.github/workflows/desktop-bundle.yml` | 已實作、待 CI 驗證：Windows NSIS、macOS `.app`／`.dmg`（ad-hoc 簽章）、Linux `.deb`／AppImage；unix 以 stdin EOF + 行程群組終止 sidecar |
| 建置環境鎖版 | `scripts/desktop_lock.py`、`scripts/desktop_env.py`、`requirements/desktop-*.txt`、`ci.yml`（`lock-check`） | 已完成：三平台雜湊鎖檔，`uv pip sync --require-hashes`；以鎖檔重建的環境與 `.venv-desktop` 的 `uv pip freeze` 完全相同 |

### 第 1 階段狀態（2026-11-26）

已結案：

- **conda-pack 與 PyInstaller**：選定 PyInstaller onedir，**負責人已於 2026-11-26 簽核**（見 ADR 0006、0010 回顧紀錄）。
- **`app/static/` 已移除**（負責人核准）：未建置前端時 `/` 回傳 503 說明頁；Docker 映像以多階段建置內含前端。
- **第三方授權聲明**：`THIRD_PARTY_NOTICES.md` 由 `scripts/gen_third_party_notices.py --strict` 產生並隨安裝包發佈；LGPL 元件（OCCT／OCP、casadi、NLopt）可在 `<安裝目錄>\sidecar\_internal\` 替換。提交的檔案為 Windows 權威版，可由鎖檔重現，Windows CI 以 `--check --strict` 把關；macOS／Linux 於 CI 重新產生並隨包發佈（ADR 0011）。`build_sidecar.py` 確認未凍結的套件不再列入（Python 套件 60 → 39 個）。
- **CadQuery 投影 SVG 在深色主題下不易閱讀**：`model.svg` 改以紙色底顯示。
- **關閉外殼時中斷進行中的渲染**：新增 `scripts/frozen_kill_check.py` 實測（Job Object 關閉後 sidecar 與 render worker 須在 5 秒內結束；`--shell-exe` 另以已安裝的外殼驗證，首次執行通過後已改為必過），在 `desktop-bundle.yml` 中執行；結果併入下方的乾淨機器驗證。

尚未達成：

- **三平台測試矩陣**：已實作（ADR 0011）：`desktop-bundle.yml` 為 `windows-latest`／`macos-latest`／`ubuntu-22.04` 矩陣，`ci.yml` 的 `desktop` job 為三平台 `cargo fmt`／`clippy`／`test`。macOS／Linux 的外殼模式終止檢查、安裝後 smoke、`.dmg`、AppImage 先列為參考（`continue-on-error`），連續兩次全綠後改為必過。
  - 跨平台 desktop-bundle 執行結果：2026-11-26 [run 45748851297](https://github.com/stevenke1981/sicad/actions/runs/45748851297) 三平台全數通過（Windows x86_64、macOS arm64、Linux x86_64／ubuntu-22.04）：各平台打包後與安裝後的 sidecar 都通過四種格式的 smoke 測試；sidecar 模式的中斷檢查 `KILL_CHECK_OK`（Unix 兩種變體各 3 次）；強制結束已安裝的外殼後 sidecar 分別於 0.016 s（Windows）、0.241 s（macOS）、0.218 s（Linux）內結束。實際磁碟大小（`du`）：Windows onedir 404.6 MiB／安裝後 409 MiB；macOS onedir 603 MB／.app 997 MB（.dmg 216 MiB）；Linux onedir 783 MB／安裝後 1.3 GB（.deb 309 MiB、AppImage 282 MiB）。macOS／Linux 安裝後變大是因為 Tauri 打包資源時把 onedir 內的符號連結展開成實體檔案。
- **乾淨機器驗證**：已改由 GitHub `windows-latest` 上的 `desktop-bundle.yml` 執行（隔離環境 smoke、Job Object 終止檢查、安裝後檢查授權聲明存在且大於 10 KB），尚待第一次完整執行的結果。
  - desktop-bundle 執行結果：2026-11-26 於 GitHub `windows-latest`（Windows Server 2025，4 CPU）首次執行 [run 45723089074](https://github.com/stevenke1981/sicad/actions/runs/45723089074) 全數通過：建置與安裝後的 sidecar 皆在 isolated env 下通過 smoke（各觀察到 7 個 render worker，`foreign_processes=[]`）；安裝 sidecar 的 Job Object 中斷檢查 `KILL_CHECK_OK`；強制結束已安裝的 `promptcad-studio.exe` 後 sidecar 於 0.007 秒內結束（`job.rs` 實證，此步驟已改為必過）；安裝目錄含 `THIRD_PARTY_NOTICES.md`。量測：安裝檔 107.5 MiB、安裝後 554.1 MiB、READY 0.64–0.66 s、warm render 2.38 s、端到端 generate 2.42 s。
- 3D 檢視器的實際繪製尚未以畫面確認（STL 已成功取得）。
- 檢視器只支援 STL；GLB、元件選取與 port 圖層延到第 2 階段。

### 桌面版量測（2026-11-26，Windows 11）

| 項目 | 數值 |
|---|---|
| sidecar onedir 大小／檔案數 | 547.5 MiB／1,168 個 |
| NSIS 安裝包（LZMA） | 107.0 MiB |
| 安裝後大小 | 550.9 MiB |
| sidecar 啟動到 READY（第一次／暖啟動中位數） | 0.85 s／0.55 s |
| render worker，`plate-four-holes`（建置後第一次／暖中位數） | 9.65 s／2.21 s（第一次含防毒掃描新檔） |
| `POST /generate` 端到端（rule planner，4 種格式） | 約 2.2 s |

最大的組成：`vtk.libs` 264 MiB、`OCP` 90 MiB、`cadquery_ocp.libs` 62 MiB、`vtkmodules` 42 MiB、`casadi` 24 MiB。VTK 約佔 56%，是之後縮小體積最大的槓桿。

## 第 2 階段切片（Unreleased / Phase 2a：Rust core）

獨立的 library crate `crates/promptcad-core/`（不依賴 Tauri，自有 `Cargo.lock`；相依 serde、serde_json、schemars、sha2，開發相依 proptest），依 [`docs/CAD_DSL_2.md`](docs/CAD_DSL_2.md) 實作 [ADR 0010](docs/adr/0010-migration-phases.md) 第 2 階段的 Rust core。2a 切片本身只有 library crate 與命令列工具；接入 Tauri 與 FastAPI 的部分見下方「第 2 階段 2b 切片」。

| 項目 | 位置 | 狀態 |
|---|---|---|
| 文件模型（CadDocument 2.0／1.0、id 與參照、24 個軸對齊朝向、issue 與 §9.2 代碼表、patch 型別、`resolved.json` 型別；serde + schemars、`deny_unknown_fields`）、欄位處理、正規化序列化與 SHA-256 | `crates/promptcad-core/src/model/`、`fields.rs`、`canonical.rs` | 已實作 |
| 封閉文法的運算式解析與綁定求值（§4） | `src/expr.rs`、`src/bindings.rs` | 已實作 |
| port 表（§3.4）、朝向與位移的不動點求解器、孔方向與切削體（§5、§7） | `src/ports.rs`、`src/solver/` | 已實作；對 `examples/cad2/` 兩份範例的輸出與 `expected_resolved` 一致 |
| 分層驗證器（schema → 結構 → 綁定 → 求解 → 設計規則） | `src/validate/` | 已實作；1.0 規則對 Python `DesignValidator` 做 49 個案例的差異測試 |
| patch 引擎（原子套用、§8.5 權限矩陣、拒絕原因、provenance 標記、復原）與 op log（append-only JSONL、重播） | `src/patch_engine/`、`src/oplog.rs` | 已實作 |
| 1.0 ↔ 2.0 轉接（`upgrade_1_to_2` 為全函式、`downgrade_2_to_1` 為部分函式） | `src/migrate.rs` | 已實作；往返不變式有屬性測試 |
| 命令列工具 `promptcad-core`（`schema --out-dir … [--check]`，另有 `validate`、`solve`、`upgrade`、`downgrade`、`normalize`、`apply-patch`；用法見 `crates/promptcad-core/README.md`） | `src/bin/promptcad-core.rs` | 已實作 |
| 由 Rust 型別生成的 JSON Schema | `schemas/cad2/`（`cad-document-1`／`cad-document-2`／`patch`／`resolved`／`validation-report` 五份 `*.schema.json`） | 已實作；`cargo run -- schema --check` 檢查漂移 |
| CI `core` job（`cargo fmt --check`、`clippy -D warnings`、`cargo test`、schema 漂移檢查） | `.github/workflows/ci.yml` | 已加入；已於 PR #5 在 GitHub 上執行並通過 |

### 第 2 階段 2a 狀態（2026-11-26）

此切片已實作的是 Rust core 本身；第 2 階段尚未結案。

尚未完成：

- （已於 2b 處理）Rust core 接進 Tauri 命令與 FastAPI 伺服器；伺服器宿主形式由 [ADR 0012](docs/adr/0012-server-host-for-rust-core.md) 記錄，待負責人簽核。
- **Schema 來源尚未切換**：執行中的應用程式仍以 `app/models/cad.py` 為來源；2.0 的 Pydantic 與 TypeScript 生成型別尚不存在（目前只有 `schemas/cad2/` 的 JSON Schema 與漂移檢查）。
- Python 編譯器不接受 2.0；Python `DesignValidator` 未移除。
- 前端的屬性編輯、檢視器元件選取與 port 顯示尚未開始（唯讀元件樹已於 2b 加入）。
- 1.0 直接輸出與經轉接後輸出的幾何等價測試（CadQuery 上的體積、包圍盒比對）尚未進行。
- 核心層（kernel）驗證代碼未實作，屬於 sidecar 的工作。

規格留白處的實作決定與已知限制：

- 已綁定欄位的快取值不一致以 `binding_eval_error` 回報（§9.2 沒有專用代碼）。
- schema 違規以拒絕原因代碼 `schema_invalid` 的 issue 呈現，包含組裝巢狀深度超過 4。
- `extrude`、`revolve` 與沉頭孔錐體的解析檢查使用保守的外包圖元：可能漏報，不會憑空產生 issue。
- 復原是在 op log 追加一筆還原前一修訂號的新紀錄；沒有 redo。
- `human` 來源不受權限矩陣與 `annotation_only_removal` 限制；角色 §8.3 子集之外的 op 以 `schema_invalid` 拒絕。
- `resolves` 的每一項必須是 §9.2 的代碼（否則 `schema_invalid`）；元件工匠的 `set_parameter` 修正 patch 另須有至少一項代碼確實出現在套用前的驗證報告中（否則 `permission_denied`），比 §8.5 的字面更嚴格。
- 孔本身有 `fixed` 時姿態以該 `frame` 為準（§5.4 第 1 步先於第 3 步）；以 Dot 定位的孔才跟隨所屬零件 `body[0]` 的姿態（含衍生 Volume）。
- 區域無法解出的 `inside`／`clearance` 約束（例如不屬於任何零件的貫穿孔）不產生問題。
- `arc` 沒有 plane port，`circle` 的法線假設為 `+z`，`revolve` 只有 `origin` 與外包區域。
- `Resolved.ports` 只含被約束引用的 port 與各元件的 frame port。
- 升級後的文件若 1.0 螺紋無效，回報 `tapped_hole_without_thread`，而非 1.0 的 `thread_annotation_only`（§10.1 正規化已將 thread 設為 null）。

## 第 2 階段 2b 切片（Unreleased / Phase 2b：接入 Tauri 與伺服器）

Rust core 已被應用程式呼叫：一個與傳輸無關的請求／回應契約（`promptcad_core::api`），由三個宿主使用。伺服器宿主形式見 [ADR 0012](docs/adr/0012-server-host-for-rust-core.md)（待簽核）。

| 項目 | 位置 | 狀態 |
|---|---|---|
| 契約 `CoreRequest`（以 `op` 區分：validate、solve、normalize、upgrade、downgrade、apply_patch、describe、version）與 `CoreResponse`（以 `status` 區分：ok／error）；`handle`、`handle_json`（請求上限 8 MiB） | `crates/promptcad-core/src/api.rs` | 已實作 |
| 命令列 `promptcad-core rpc`（stdin 讀一個 JSON 請求，stdout 寫一行回應；寫出回應時結束碼為 0） | `crates/promptcad-core/src/bin/promptcad-core.rs` | 已實作 |
| 請求與回應 JSON Schema（由 Rust 型別生成） | `schemas/cad2/core-request.schema.json`、`schemas/cad2/core-response.schema.json` | 已實作 |
| 桌面：Tauri 命令 `core_request`，行程內於阻塞執行緒呼叫 core；權限 `allow-core-request` | `src-tauri/src/core.rs`、`src-tauri/capabilities/default.json` | 已實作 |
| 伺服器：`POST /api/v1/core`，每次呼叫啟動一次 `promptcad-core rpc`，位元組原樣轉交；ok 與 error 皆回 HTTP 200，傳輸問題回 413／422／502／503／504 | `app/api/routes.py`、`app/services/core_bridge.py`、`app/core/config.py` | 已實作 |
| 伺服器設定 `PROMPTCAD_CORE_BIN`、`PROMPTCAD_CORE_TIMEOUT_SECONDS`、`PROMPTCAD_CORE_MAX_REQUEST_BYTES`；子行程只保留白名單環境變數（不含 Token 與 API 金鑰） | `app/core/config.py`、`app/services/core_bridge.py` | 已實作 |
| `/api/v1/capabilities` 新增 `core_available`、`core_crate_version`、`core_api_version`、`core_schema_versions` | `app/api/routes.py` | 已實作 |
| TypeScript 型別由兩份 core schema 生成，CI 檢查漂移 | `web/scripts/gen-types.mjs`、`web/src/api/generated/core-request.d.ts`、`core-response.d.ts` | 已實作 |
| 前端傳輸層：Tauri invoke 與 HTTP 兩種實作；只有 `bootstrap.ts` 接觸 Tauri 全域 | `web/src/api/coreClient.ts`、`web/src/api/bootstrap.ts` | 已實作 |
| 編輯器持有 2.0 文件時，「驗證 JSON」經 core；1.0 流程不變 | `web/src/App.tsx` | 已實作 |
| 唯讀元件樹：組裝、零件、body 操作、元件（欄位、綁定、port）、約束；不可編輯 | `web/src/components/ComponentTree.tsx` | 已實作 |
| 2.0 文件的輸出（生成）停用，因幾何編譯器尚不接受 2.0 | `web/src/App.tsx` | 已停用 |
| Docker：新增 `rust:1.90.0-slim-trixie` 建置階段，把 `promptcad-core` 複製到 `/usr/local/bin/promptcad-core` | `Dockerfile` | 已加入；**本機未驗證**（開發機沒有 Docker），待 CI `docker` job 於 PR 執行時驗證 |
| CI：`test` job 建置 core 執行檔並跑整合測試；`docker` job 斷言 `/api/v1/core`；`web` job 檢查生成型別 | `.github/workflows/ci.yml` | 已加入；GitHub 上的執行結果本文件撰寫時未確認 |

### 第 2 階段 2b 驗證（本機）

- `crates/promptcad-core` 的 `cargo test`：317 個測試通過。
- `src-tauri` 的 `cargo test`：26 個測試通過。
- `pytest`：381 項通過、3 項略過；伺服器整合測試對實際的 `promptcad-core` 執行，並重播 49 個 parity 案例（Python `DesignValidator`）。
- 同一請求經 Tauri 命令與 `promptcad-core rpc` 得到逐位元組相同的回應（測試）。
- 前端 vitest：68 項通過。
- 瀏覽器端到端（本機伺服器）：載入內建 2.0 範例 → 「驗證 JSON」→ 元件樹顯示。

### 第 2 階段 2b 資源上限（獨立審查後加入）

- core 回應上限 24 MiB（`api::MAX_RESPONSE_BYTES`），超過時回 `error` 信封，代碼 `response_too_large`；Tauri 與 `rpc` 共用同一實作。
- `describe` 的樹：每個組裝與零件只在第一次出現時展開，其後以 `repeated: true` 的節點表示，節點總數上限 4096（`api::MAX_TREE_NODES`）。修正前，約 1.6 KB、5 層重複子項的文件會產生 1.8 GB 回應。
- 各操作的最壞回應大小已以達到 schema 上限的文件量測並有測試（`crates/promptcad-core/tests/api_regressions.rs`）：`validate` 最大約 10.6 MB、`apply_patch` 最大約 14.5 MB（兩者因 `reserve_overlap` 成對回報而為平方成長），其餘操作為線性。
- 伺服器：core 呼叫使用專用執行緒池，同時最多 `PROMPTCAD_CORE_MAX_CONCURRENCY`（預設 4）個子行程、最多 4 倍數量排隊；取不到名額時於 `PROMPTCAD_CORE_QUEUE_WAIT_SECONDS`（預設 3 秒）內回 `503` 並附 `Retry-After`，不啟動行程。取消請求或關閉伺服器時會結束子行程。
- 伺服器尋找執行檔時不看工作目錄，Windows 只接受 `.exe`；`PROMPTCAD_ENV=production` 與凍結模式不使用 `crates/promptcad-core/target/` 的開發後備路徑。
- 桌面版 release profile 為 `panic = "abort"`，因此 `api::handle` 內的 `catch_unwind` 在桌面版不生效；目前沒有已知可觸發的 panic，是否調整 profile 待負責人決定。
- 前端經 Tauri 傳送文件時必須先解析成物件，超過 2^53 的整數會失真；此時驗證面板會列出「輸入警告」。經 HTTP 傳送時原文直接嵌入請求，不失真。

### 第 2 階段 2b 未驗證

- Tauri `invoke` 在真實 WebView 內的往返。
- 完整的 `tauri build`。
- Docker 映像建置（CI `docker` job）。

### 第 2 階段 2b 尚未完成

- Schema 來源仍為 `app/models/cad.py`；2.0 的 Pydantic 生成模型尚不存在。
- Python 編譯器不接受 2.0。
- Python `DesignValidator` 未移除。
- 無 patch／op log UI；2.0 文件與 `oplog.jsonl` 未在 jobs 中持久化。
- 3D 檢視器無元件選取。
- 1.0 直接輸出與經轉接輸出的幾何等價測試（CadQuery 體積、包圍盒）未做。

## 第 2 階段 2c 切片（Unreleased / Phase 2c：驗證器改由 Rust core 執行）

所有 1.0 驗證改經 Rust core 的 `validate` 操作執行，Python 端不再含任何設計規則。`/api/v1/plan`、`/generate`、`/generate-from-spec`、`/validate`、CLI `promptcad validate|render|generate` 與桌面 sidecar 皆走此路徑。依 [ADR 0010](docs/adr/0010-migration-phases.md) 第 2 階段的完成條件，「移除 Python 端的設計驗證邏輯」已達成；Pydantic 生成僅涵蓋 validation report 與 2.0 spike，1.0 `CadDocument` 仍為手寫（見下方「尚未完成」）。

| 項目 | 位置 | 狀態 |
|---|---|---|
| 1.0 驗證經 Rust core：`CoreValidator` 透過 `CoreBridge` 呼叫 `validate`；`JobService.validate` 改為 async | `app/services/core_validator.py`、`app/services/job_service.py`、`app/services/core_bridge.py` | 已實作；core 不可用時回 503，逾時 504，core 失敗 502（見 `docs/API.md`） |
| Python 設計規則移除 | `app/services/validator.py`（`DesignValidator`）、`crates/promptcad-core/tests/fixtures/gen_v1_parity.py` | 已刪除；`crates/promptcad-core/tests/fixtures/v1_parity.json` 保留為凍結的對照基準 |
| 1.0 issue 的 `feature_index` 回填（WP1） | core 的 `validate` 操作（`crates/promptcad-core/src/validate/`） | 已實作：`e_h{n}` 的 issue 為 0-based 孔索引；`fillet_too_large`、`chamfer_too_large` 為 `part.fillets`／`part.chamfers` 的索引（1.0、2.0 皆然）；`overlapping_holes` 取配對中第一個孔；其餘為 null。無 wire／schema 變動，`API_VERSION` 仍為 1 |
| validation report 的 Pydantic 由 Rust 生成（ADR 0008） | `scripts/gen_pydantic.py`、`schemas/cad2/validation-report.schema.json`、`app/models/generated/validation_report.py`、`app/models/cad.py` | 已實作：生成器 `datamodel-code-generator==0.103.0`（`pyproject.toml` 的 `schema` extra 固定版本）；輸出 LF、含 DO-NOT-EDIT 標頭、無時間戳；`app/models/cad.py` 改為重新匯出 `ValidationReport`、`ValidationIssue`、`ValidationSeverity` |
| 生成前置轉換 | `scripts/gen_pydantic.py` | 已實作：schemars 的字串 `const` 型 `oneOf` 先轉為 `enum`，`Severity`、`Role`、`Layer` 成為真正的 StrEnum（成員為小寫，如 `error`、`warning`、`info`） |
| 生成檢查 | `Makefile`（`schema-py`、`schema-py-check`）、`.github/workflows/ci.yml` 的 `test` job | 已實作：CI 執行 `python scripts/gen_pydantic.py --check` |
| 2.0 文件的 Pydantic 生成（spike） | `app/models/generated/cad_document_2.py`（根類別 `CadDocument2`） | spike：兩份 `examples/cad2/` 文件可精確來回；**尚未被應用程式使用**。已知瑕疵：`AxisDir` 成員為 `field_x`／`field_x_1`、形狀聯集名稱帶編號（`DotShape1`、`VolumeShape7`）、`RootModel` 包裝、非 discriminated 聯集 |
| 1.0 `CadDocument` 模型 | `app/models/cad.py` | 仍為手寫；仍是 `schemas/promptcad.schema.json` 的來源 |
| 桌面 sidecar 內含 core | `scripts/build_sidecar.py`（`--core-bin`）、`scripts/frozen_smoke.py`、`desktop-bundle.yml` | 已實作：`promptcad-core(.exe)` 複製到 `<onedir>/_internal/`，缺少執行檔即建置失敗；smoke 檢查 `core_available=true` 與 `POST /api/v1/core {"op":"version"}`；`desktop-bundle.yml` 凍結前以 cargo 建置 core |
| Docker 映像內含 core；CI 斷言 | `Dockerfile`、`.github/workflows/ci.yml` 的 `docker` job | 映像已含 core；`docker` job 另斷言 `POST /api/v1/validate` 對 `examples/generated/plate-four-holes/spec.json` 回傳含非 null `target` 的 issue |
| 前端驗證面板的 zh-TW 短標籤 | `web/src/components/issueCodeLabels.ts` | 已實作：60 個 issue 代碼各一個短標籤，顯示於訊息之前 |
| 第三方授權聲明 | `THIRD_PARTY_NOTICES.md` | 已重新產生：Rust crate 228 → 230（補上 `schemars_derive`、`serde_derive_internals`） |

### 第 2 階段 2c 的 1.0 行為變更（已接受）

- issue 訊息改為 core 的英文 id 模板（例如 `hole e_h1 …`）。
- `assumption` 的訊息不再回顯假設文字。
- issue 新增 `target`、`related`、`owner`、`layer` 欄位；舊的四欄 manifest 仍可讀取。
- issue 順序改變。
- 2.0 專屬代碼可能出現在 1.0 流程（例如 `empty_cut_no_effect`）。
- 無效的螺紋字串回報 `tapped_hole_without_thread`，取代 `thread_annotation_only`。

### 第 2 階段 2c 驗證（本機）

- `pytest`：423 項通過、3 項略過（移除 Python 驗證器後；回歸基準為 `tests/fixtures/v1_validation_golden.json`）。
- `crates/promptcad-core` 的 `cargo test`：323 個測試通過（2b 為 317）。
- `src-tauri` 的 `cargo test`：26 個測試通過。
- 前端 vitest：71 項通過（10 個檔案）。
- **差異閘門 G1**（移除 Python 驗證器前，`tests/test_validation_shadow.py`）：83 個輸入（49 個 parity 語料、6 個 `frozen_smoke` 案例、3 個 `kill_check` 規格、25 個規則 planner 提示詞，取自 `tests/README`、`PROMPT_GUIDE`）中，77 個在 (code, severity, feature_index) 多重集合、`valid`、`review_required` 上一致。
  - 允許的 6 個差異：`plate_thread_invalid`（螺紋正規化）；`ring_hole_in_inner_void`、`lbracket_z_blind_too_deep`、`enclosure_z_blind_too_deep`（多出已知的 `empty_cut_no_effect`）；`frozen_smoke` 的 `l_bracket` 與 `enclosure`（多出 `e_h2` 的 `empty_cut_no_effect`：盲孔起點在無材料處）。
  - core 結果存為黃金基準 `tests/fixtures/v1_validation_golden.json`（以 `PROMPTCAD_UPDATE_GOLDEN=1` 重新產生），移除後作為回歸依據。
- **延遲**：經 bridge 的 `validate` 中位數約 20 ms（p95 約 21 ms、最大約 33 ms，共 83 次呼叫，本機 Windows）。
- **桌面閘門 G2**（以新 core 重建的 sidecar 與隔離環境 smoke）：於本機 Windows 11 通過。onedir 由 402.4 MiB 增為 406.1 MiB（+3.7 MiB，低於 460 MiB 閘門）；隔離環境 smoke 的 `foreign_processes=[]`。

### 第 2 階段 2c 未驗證

- GitHub CI 對本切片的執行結果（本文件撰寫時尚待確認）。
- macOS／Linux 桌面包內巢狀的 core 執行檔（含 macOS ad-hoc 簽章）。
- Tauri WebView 內的真實往返。

### 第 2 階段 2c 尚未完成

- Python 編譯器尚不接受 2.0；1.0 直接輸出與經轉接輸出的幾何等價測試（CadQuery）尚未進行。
- 1.0 `CadDocument` 的 Pydantic 模型尚未由 Rust 生成（仍手寫）；2.0 的 Pydantic 僅為 spike。
- 前端的屬性編輯、檢視器元件選取與 port 顯示。
- [ADR 0012](docs/adr/0012-server-host-for-rust-core.md) 待負責人簽核。

## 已驗證

v0.1.0 發行包：

- 26 個 pytest 測試通過。
- Python `compileall` 通過。
- Web JavaScript `node --check` 通過（`app/static/` 已於第 1 階段移除，此項不再適用）。
- Editable package 安裝與 `promptcad --help` 通過。
- Source-only 端到端 smoke generation 通過。

第 1 階段切片另以下列檢查驗收（見 `.github/workflows/ci.yml`）：

- `pytest`：原有 26 個測試不修改，另加 schema 匯出、前端服務、sidecar 啟動與 2.0 範例的測試。
- `ruff check .`。
- `python scripts/export_schema.py --check` 與 `npm --prefix web run gen:types` 後 `git diff --exit-code`（schema 漂移檢查）。
- `npm --prefix web run typecheck`、`test`、`build`。
- `src-tauri` 的 `cargo fmt --check`、`cargo clippy -D warnings`、`cargo test`（CI 的 `desktop` job，`windows-latest`／`macos-latest`／`ubuntu-22.04` 三平台）。
- 桌面安裝包：`scripts/frozen_smoke.py` 以凍結後的 sidecar 對五種基體 × 六種孔型實際渲染 STEP／STL／DXF／SVG（手動或 `v*` tag 觸發 `.github/workflows/desktop-bundle.yml`）。
- 前端服務：`tests/test_frontend_serving.py` 檢查建置產物的提供、路徑穿越防護，以及未建置時的 503 說明頁；CI 的 `docker` job 建置映像並確認 `/` 回傳含 `id="root"` 的新前端。
- 授權聲明：`tests/test_third_party_notices.py`（純函式、假資料，不需 CadQuery；含 unix 原生函式庫路徑的辨識與正規化）；`desktop-bundle.yml` 在 Windows 以 `--check --strict` 比對提交的檔案，在 macOS／Linux 以 `--strict` 重新產生。
- 建置環境鎖檔：`tests/test_desktop_lock.py`（純函式）；`ci.yml` 的 `lock-check` job 執行 `scripts/desktop_lock.py --check`。
- 桌面驗證腳本：`tests/test_frozen_kill_check.py` 等純函式測試在 `ci.yml` 執行；實際的隔離環境 smoke 與 Job Object 終止檢查只在 `desktop-bundle.yml` 對已安裝的 sidecar 執行。

第 2 階段 2a 切片在 `crates/promptcad-core/` 內以 `cargo test` 驗收（2026-11-26 於本機 Windows 實際執行）：

- 共 253 個測試通過、0 失敗（單元測試加 19 個整合測試檔，含命令列測試 `tests/cli_commands.rs` 與審查後的回歸測試 `tests/solver_regressions.rs`、`tests/patch_regressions.rs`）。屬性測試使用 proptest。
- `cargo fmt --check` 與 `cargo clippy --all-targets -- -D warnings` 通過。
- Python 差異測試：`tests/fixtures/v1_parity.json` 的 49 個案例由 `tests/fixtures/gen_v1_parity.py` 實際執行 Python `DesignValidator` 產生，`tests/validate_parity_v1.rs` 比對 Rust 驗證器的判定。
- CI 的 `core` job 已於 PR #5 在 GitHub 上執行並通過。

## 封裝環境限制

建立 v0.1.0 發行包的執行環境沒有 Docker、CadQuery、OpenSCAD 或 Conda，因此沒有在該環境中實際執行 OpenCascade 生成 STEP／STL／DXF。專案的 Dockerfile 與 Conda 環境已固定 CadQuery 2.8.0。桌面版自第 1 階段 1b 起以 pip 安裝的 `cadquery==2.8.0`（`.venv-desktop`）打包，已在 Windows 上實際產生 STEP／STL／DXF；`.venv-desktop` 現由 `requirements/desktop-*.txt` 的雜湊鎖檔安裝（cadquery-ocp 7.9.3.1.1、vtk 9.6.2、casadi 3.8.2、nlopt 2.11.0、numpy 2.5.5、PyInstaller 6.22.3，三平台相同）。

## 第 2 階段 2d 切片（Unreleased / Phase 2d：2.0 文件端到端輸出）

2d 讓 2.0 文件能經 REST、CLI 與桌面版輸出幾何。新的 `Cad2Compiler` 讀取 Rust core 的 `validate` 結果（`document` 與 `resolved`），產生 `model.py`；Python 端不做任何求解，只做 `resolved.components` 的剛體放置、`resolved.holes` 的主孔切刀，以及 `<id>.entry` port 上的 §7.2 沉頭、埋頭算術。不支援的構造會拋出 `CompileUnsupported` 並列出每一個（id, 原因），不會被靜默略過。1.0 流程仍使用舊的 `CadQueryCompiler`，行為無變更。

為何不採用「core 降級為 1.0 文件後交給 1.0 編譯器」：降級會讓等價測試變成同義反覆，降級也不保留幾何（沿軸向的入口會被投影到 0），且無法表達多零件文件（enclosure-with-lid 為 `expressible: false`）。降級仍保留為測試參照（oracle）。決策紀錄見 [0006](docs/adr/0006-geometry-kernel-occt-python-sidecar.md) 的回顧紀錄。

| 項目 | 位置 | 狀態 |
|---|---|---|
| 2.0 編譯器 `Cad2Compiler`：產生與 1.0 相同契約的 `model.py`（`build() -> (result, warnings)`、`export_all`、`--output-dir`／`--formats`、`render-warnings.json`） | `app/services/compiler2.py` | 已實作。輸出確定性；文件文字不進入產生的程式碼（以索引命名、數值經 `float` 輸出、軸向以固定表格對應） |
| 2.0 生成端點 `POST /api/v1/generate-from-document`（`GenerateFromDocumentRequest`） | `app/api/routes.py`、`app/services/job_service.py`、`app/services/core_validator.py` | 已實作；回應與錯誤見 `docs/API.md`。2.0 不回退到 OpenSCAD |
| 無 CadQuery 時的 2.0 工作：`Renderer.render(allow_openscad=False)` 回 `source_only` 並附固定警告 | `app/services/renderer.py` | 已實作 |
| `JobManifest` 新增 `spec_version`（預設 `"1.0"`）與 `document`，`spec` 改為可為 null；舊 manifest 仍可讀取 | `app/models/api.py` | 已實作 |
| CLI：`promptcad render`、`promptcad validate` 偵測 `schema_version` 並接受 2.0；文件被拒絕時 stderr 輸出 `document rejected: …`，結束碼 2；core 傳輸錯誤結束碼 3 | `app/cli.py` | 已實作 |
| 前端：2.0 文件的「套用 JSON 並重新輸出」已啟用（呼叫 `generateFromDocument`，移除 `expected_resolved`）；2.0 工作完成後編輯器顯示 `manifest.document`；失敗工作顯示「輸出失敗（renderer_used）：error」；含大整數時顯示精度警告 | `web/src/App.tsx`、`web/src/api/client.ts`、`web/src/api/httpClient.ts` | 已實作（vitest） |
| 桌面版：沿用同一 sidecar；`cad2_smoke_cases()` 對兩份 `examples/cad2` 文件呼叫新端點 | `scripts/frozen_smoke.py` | 已實作；隔離環境下的結果見「未驗證」 |
| 幾何等價 harness：比較 1.0 `CadQueryCompiler`（A）與 2.0 `Cad2Compiler`（B） | `scripts/geometry_equivalence.py`、`make geometry-check` | 已實作；本機結果見下 |
| CI：docker job 在映像內執行 harness，並對新端點 POST `plate-four-holes.cad2`（預期 `completed` 且含 `model.step`）；`desktop-bundle.yml` 在三個平台執行 harness | `.github/workflows/ci.yml`、`.github/workflows/desktop-bundle.yml` | 已加入；GitHub 上的執行結果尚待確認 |

### 第 2 階段 2d 的支援與不支援構造

支援（2.0 子集）：

- Volumes：`plate`、`cylinder`、`ring`、`l_bracket`、`enclosure`，任一 24 種方向。
- body 操作：`cut` 的 Empty 形體（hole、box、cylinder）；`intersect` 的 shortcut Volume，或 envelope 的 box、cylinder。
- 多零件文件：以世界座標輸出為一個 compound。
- 外購零件（purchased）納入輸出，並附警告。
- 圓角、倒角：僅限 `body[0]` 為 identity 方向的零件。

不支援（拋出 `CompileUnsupported`）：`extrude`、`revolve`、Empty 的 `of`、旋轉零件上的圓角、gated 或 empty 的 `resolved`。

延後（未實作）：零件座標系建置加上 `cq.Location` 組裝（§6.2）、2.0 的 `model.scad`／`preview.svg`、`kernel-validation.json`、`bom.json`、op log 持久化。

### 第 2 階段 2d 的已知偏差

- 2.0 幾何在世界座標建立，不採用 [CAD_DSL_2](docs/CAD_DSL_2.md) §6.2 的零件座標系。
- 非盲孔的主切刀由負端沿正向軸建立（與 1.0 相同的形式）。改用另一種參數化時，`cylinder_y_axis_outside` 的 OCC 體積積分相對差異為 3.3e-6，因此未採用。

### 第 2 階段 2d 驗證（本機）

- **幾何等價**（`make geometry-check`；Windows 11、`.venv-desktop`、CadQuery 2.8.0）：共 54 個唯一案例，取自 `examples/generated`、49 個 parity 規格與 `frozen_smoke`（去除 2 個重複）。結果：54 個 equal，0 個 both_failed，0 個 mismatch，允許差異 0 個。最大體積差 0.0；最大對稱差體積 0.0 mm³。`plate_fillet_all_too_large`、`ring_chamfer_all_too_large` 兩個編譯器都產生無效實體，因此略過對稱差檢查（其餘指標仍比較）。執行時間約 7–12 秒。判準：體積相對誤差 1e-6、bbox 與質心絕對誤差 1e-6、對稱差體積 < 1e-3 mm³、警告數相同。
- **2.0 原生檢查**：
  - `plate-four-holes.cad2.json`（移除 `expected_resolved`）的 `Cad2Compiler` 輸出，與 1.0 `CadQueryCompiler` 在 `downgrade` 輸出上的結果一致（對稱差 0.0）。
  - `enclosure-with-lid`：bbox [-50,-35,0]–[50,35,35]；體積 100·70·33 − 96·66·31 + 100·70·2；2 個實體。
  - `l_bracket` 的 24 種方向：bbox 與解析旋轉後的局部 bbox 一致，質心一致。
- **實機輸出**（CLI，`.venv-desktop`）：`plate-four-holes.cad2` 3.4 秒，`completed／cadquery`（`model.step` 46 KB、`model.stl` 128 KB，另有 DXF、SVG）；`enclosure-with-lid.cad2` 3.1 秒，`completed／cadquery`（`model.step` 47 KB）。
- **CadQuery 2.8.0 實測的呼叫**：`cq.Plane(origin=, xDir=, normal=).location`、`Shape.moved(loc)`、`Workplane('XY').newObject([shape])`（plane 的 yDir 等於 core 的 z×x）、`Workplane.cut／union／intersect`（接受 Solid 或 Workplane）、`cq.Compound.makeCompound(list)` 包成 `newObject` 後匯出 STEP，並對 compound 呼叫 `.section(z)`、`Solid.makeCylinder`、`Solid.makeCone(r1, r2, h, pnt, dir)`、`Shape.Volume()`、`BoundingBox()`、`Center()`、`isValid()`。
- **桌面閘門 G2**（以本切片重建的 sidecar，`frozen_smoke.py --isolated-env`，Windows 11）：通過。onedir 406.1 MiB（890 個檔案）；`cad2_smoke_cases()` 兩份 2.0 範例經 `POST /api/v1/generate-from-document` 皆為 `completed`（`enclosure-with-lid` 1.83 秒、`plate-four-holes` 1.88 秒）；`foreign_processes=[]`。
- **其他**：pytest 472 項通過、3 項略過；vitest 75 項通過（10 個檔案）；無 Rust 變更（無 schema 變更，`API_VERSION` 仍為 1）；無新增相依套件（lock 檔與 `THIRD_PARTY_NOTICES.md` 未變）。

### 第 2 階段 2d 未驗證

- GitHub CI 對本切片的執行結果（docker job 的 harness 與新端點檢查、`desktop-bundle.yml` 三平台的 harness）。撰寫時尚待確認。
- Tauri WebView 內的真實往返（含前端「套用 JSON 並重新輸出」的 2.0 流程）。

### 第 2 階段 2d 尚未完成

- 1.0 流程改用 `Cad2Compiler`，並移除舊的 `CadQueryCompiler`（於 CI 綠燈後進行）。
- 零件座標系建置（§6.2）與上方的已知偏差。
- 2.0 的 `model.scad`、`preview.svg`，以及 `kernel-validation.json`、`bom.json`、op log 持久化。
- 前端的屬性編輯、檢視器元件選取與 port 顯示。
- 1.0 `CadDocument` 的 Pydantic 由 Rust 生成。
- [ADR 0012](docs/adr/0012-server-host-for-rust-core.md) 待負責人簽核。

## 下一階段

依 [ADR 0010](docs/adr/0010-migration-phases.md)：

1. 完成第 1 階段：取得 `desktop-bundle.yml` 三平台矩陣的執行結果，依 CI 的 `--strict` 失敗補齊 unix 原生函式庫分類，macOS／Linux 的參考項目連續兩次全綠後改為必過（ADR 0011）。
2. 完成第 2 階段：Rust core（CadDocument 2.0、驗證器、patch 引擎、op log、1.0 → 2.0 adapter）已於 `crates/promptcad-core/` 實作（見 Phase 2a），並已接入 Tauri 與伺服器（見 Phase 2b）。剩餘項目：
   - 負責人簽核 [ADR 0012](docs/adr/0012-server-host-for-rust-core.md)；驗證 2b、2c、2d 未驗證項目（Tauri 真實 WebView 往返、`tauri build`、CI `docker` job、2c 與 2d 的 GitHub CI 執行、2d 的桌面 sidecar 2.0 smoke、macOS／Linux 桌面包內的 core 執行檔）。
   - 1.0 `CadDocument` 與 2.0 文件的 Pydantic 由 Rust 生成（TypeScript 的 core 請求／回應型別已於 2b 生成；validation report 的 Pydantic 已於 2c 生成；2.0 僅有 spike），`app/models/cad.py` 僅保留 1.0 相容用途或被生成檔取代（ADR 0008）。
   - （已於 2d 完成）Python 編譯器接受已驗證的 2.0 規格（`Cad2Compiler`），並完成 1.0 直接輸出與經轉接輸出的幾何等價測試（`scripts/geometry_equivalence.py`），見上方「第 2 階段 2d 切片」。
   - 1.0 流程改用 `Cad2Compiler` 並移除舊編譯器（`CadQueryCompiler`），於 CI 綠燈後進行。
   - 前端的屬性編輯、檢視器元件選取與 port 顯示（唯讀元件樹已於 2b 完成）。
   - （已於 2c 完成）移除 Python 端的設計驗證邏輯，見上方「第 2 階段 2c 切片」。
3. 第 3 階段：約束求解器評估（[ADR 0007](docs/adr/0007-constraint-solver.md)）與多 agent 編排。

仍保留的 1.0 功能構想（將併入 2.0 的對應階段）：

- 2D 草圖 DSL：線、圓弧、約束、拉伸、旋轉與陣列（併入 2.0 的 Line／Plane 升維鏈）。
- 多孔群與 pattern／mirror。
- 正式工程圖、尺寸、公差、BOM 與標題欄。
- BREP validity、最小壁厚、製程規則與干涉檢查（2.0 的核心層驗證）。
- 非同步 renderer worker、sandbox、配額與多租戶。
- 圖片／手繪草圖／DXF 輸入。
