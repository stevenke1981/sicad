# 0010：四階段遷移路線

- **狀態**：Accepted
- **日期**：2026-11-17
- **決策者**：專案負責人
- **影響範圍**：整體專案結構、發佈節奏、`PROJECT_STATUS.md`

## 背景

[0002](0002-desktop-shell-tauri-v2.md) 到 [0009](0009-agent-orchestration.md) 描述的目標架構，與 v0.1.0 的單一 Python 程序相比，改變了外殼、前端、核心語言、文件模型與 LLM 互動方式。一次全部替換的風險過高：v0.1.0 是可運作、有測試、有使用者流程的系統，遷移期間不應失去這些。

需要一個順序，使得：

- 每個階段結束時都有可發佈、可使用的產品。
- 風險最高的不確定性（sidecar 封裝、WebView 相容性）最早被驗證。
- 既有的 REST API、CLI、Docker 模式與 `spec.json` 範例在整個過程中持續可用。

## 決策

採用以下四個階段，依序進行；前一階段的完成條件達成後才開始下一階段的主要工作。

### 第 1 階段：Tauri 包裝既有 FastAPI + 新前端

**內容**

- Tauri v2 外殼，把現有 FastAPI 應用程式（含 CadQuery）封裝為 sidecar（[0002](0002-desktop-shell-tauri-v2.md)、[0006](0006-geometry-kernel-occt-python-sidecar.md)）。
- React + TypeScript + Vite 前端，達到與 `app/static/` 功能等價後取代之（[0003](0003-frontend-react-typescript.md)）。
- Three.js 檢視器：載入 GLB／STL，基本導覽（[0004](0004-3d-viewer-threejs.md)）。
- 建立 schema 下游管線：由 Pydantic 匯出的 JSON Schema 生成 TypeScript 型別（[0008](0008-single-schema-source-of-truth.md) 的過渡安排）。

**不變的部分**：`CadDocument 1.0`、`DesignValidator`、planner、編譯器與 renderer 的邏輯。Rust 在此階段只負責外殼與 sidecar 生命週期。

**完成條件**

- 乾淨的 Windows 機器上安裝後，不需 Python／Conda／Docker 即可完成提示詞到 STEP／STL 的流程。
- 同一份前端建置產物可用於桌面與 Docker 伺服器模式。
- `app/static/` 已移除。
- 已量測並記錄：安裝包大小、sidecar 冷啟動時間、典型模型的網格載入時間。
- conda-pack 與 PyInstaller 的封裝比較已完成並選定其一。

### 第 2 階段：CadDocument 2.0、驗證器與 patch 引擎移至 Rust

**內容**

- Rust core：CadDocument 2.0 型別、元件圖、驗證器、patch 引擎、op log、1.0 → 2.0 adapter（[0005](0005-rust-core-document-model.md)）。
- Schema 來源切換為 Rust；Pydantic 與 TypeScript 型別皆為生成物（[0008](0008-single-schema-source-of-truth.md)）。
- Python 的職責收斂為「規格 → 幾何 → 匯出」（[0006](0006-geometry-kernel-occt-python-sidecar.md)）；編譯器改為接受 2.0 的已驗證規格。
- 前端：元件樹、屬性編輯、檢視器的元件選取與 port 顯示。
- 決定伺服器模式下 Rust core 的宿主形式（新增 ADR）。

**完成條件**

- 所有 `examples/generated/` 的 1.0 文件經 adapter 後輸出等價幾何。
- Python 與 Rust 驗證器對既有測試案例判定一致後，移除 Python 端的設計驗證邏輯。
- CI 的 schema 生成比對檢查已啟用。
- REST API 與 CLI 在伺服器模式下仍可用（可透過相容層接受 1.0 文件）。

### 第 3 階段：約束求解器 + 多 agent 編排

**內容**

- 完成 planegcs 與 libslvs 的對照評估並整合選定者（[0007](0007-constraint-solver.md)，以新 ADR 記錄結果）。
- Rust 端的 agent 編排：tool use、patch 提交、驗證回饋迴圈、供應商介面（[0009](0009-agent-orchestration.md)）。
- 多 agent 角色與協作；前端的 op log 檢視、接受與復原。

**完成條件**

- [`../CONCEPT_DIMENSIONAL_LADDER.md`](../CONCEPT_DIMENSIONAL_LADDER.md) 中的代表性組裝案例可由 agent 以 patch 逐步建構並求解。
- 欠約束／過約束／衝突皆有結構化診斷。
- Agent 的所有修改可於 op log 追溯並復原。
- OpenAI-compatible 端點與本地規則解析器路徑仍可運作。

### 第 4 階段（選擇性）：以直接的 OCCT 綁定移除 Python

**內容**

- 評估並（若通過）以 Rust 直接綁定 OCCT（`cxx` 自行綁定所需子集，或屆時成熟的現成綁定），取代 Python sidecar。
- 簡化 schema 管線（移除 Pydantic 生成）與封裝流程。

**啟動條件**（未滿足則不啟動，維持第 3 階段架構）

- 第 3 階段完成，且 sidecar 的體積或啟動時間仍是使用者回饋中的主要問題；或
- 綁定方案已能通過本專案的幾何回歸測試集。

**完成條件**

- 幾何回歸測試集（布林、fillet／chamfer、STEP、DXF 截面）在新路徑上與 CadQuery 路徑結果等價。
- 桌面安裝包不含 Python 執行環境。

本階段的啟動本身需要一份新 ADR，並將取代 [0006](0006-geometry-kernel-occt-python-sidecar.md)。

### 各階段的元件歸屬

| 元件 | v0.1.0 | 第 1 階段 | 第 2 階段 | 第 3 階段 | 第 4 階段（選擇性） |
|---|---|---|---|---|---|
| 外殼 | 無（瀏覽器） | Tauri v2 ＋ 瀏覽器 | 同左 | 同左 | 同左 |
| 前端 | 靜態 HTML/JS | React/TS ＋ Three.js | ＋ 元件樹、port | ＋ op log、agent 介面 | 同左 |
| 文件模型 | Pydantic 1.0 | Pydantic 1.0 | Rust 2.0（＋1.0 adapter） | 同左 ＋ 約束 | 同左 |
| 驗證器 | Python | Python | Rust | Rust | Rust |
| Patch／op log | 無 | 無 | Rust | Rust | Rust |
| 約束求解 | 無 | 無 | 無 | planegcs 或 libslvs | 同左 |
| LLM 呼叫 | Python planner | Python planner | Python planner | Rust 編排（Python planner 為相容路徑） | Rust 編排 |
| 幾何核心 | CadQuery（同程序啟動 subprocess） | CadQuery sidecar | CadQuery sidecar | CadQuery sidecar | Rust → OCCT |
| Schema 來源 | Pydantic | Pydantic（生成 TS） | Rust（生成 Pydantic、TS） | 同左 | Rust（生成 TS） |

## 考慮過的替代方案

| 方案 | 優點 | 缺點 | 結論 |
|---|---|---|---|
| **先外殼與前端，再核心，再求解與 agent，最後（選擇性）核心替換** | 每階段皆可發佈；最早驗證封裝與 WebView 風險；第 1 階段不動領域邏輯，回歸風險低 | 第 1 階段的前端需在第 2 階段因模型改版而調整；過渡期有重複實作 | 採用 |
| 一次重寫（Big bang） | 沒有過渡期的重複與相容層 | 長時間沒有可發佈版本；所有風險同時出現；無法以現行系統作為回歸基準 | 不採用 |
| 先做 Rust core，最後才做桌面外殼 | 核心模型最早穩定，前端只需做一次 | 使用者長時間看不到成果；sidecar 封裝與 WebView 風險最晚才被驗證 | 不採用 |
| 先在 Python 內實作 2.0 與 patch 引擎，之後再移植到 Rust | 用熟悉的語言快速驗證模型設計 | 同一套邏輯寫兩次；移植時再次承擔行為差異風險 | 不採用；但 2.0 的模型設計可先以文件與範例驗證 |
| 先做 agent 與約束（功能優先），外殼最後 | 最早展示差異化功能 | 缺少 patch 引擎與元件圖，agent 無處著力；仍需回頭重做 | 不採用 |
| 跳過 Python sidecar，直接做 OCCT 綁定 | 省去 sidecar 封裝工作 | 把最不確定、最耗時的工作放在關鍵路徑最前端 | 不採用 |

## 後果

### 正面

- 任何時間點都有可運作的產品，且每個階段都可獨立評估是否繼續。
- 第 4 階段是選擇性的：若 sidecar 的成本可接受，架構可以穩定停留在第 3 階段。
- 既有測試與範例在每個階段都是回歸基準。

### 負面

- 過渡期的重複：第 1 階段為 1.0 建立的前端型別與編輯畫面，在第 2 階段需要改版；第 2 階段 Python 與 Rust 驗證器短暫並存；第 3 階段前有兩條 LLM 路徑。
- 相容層（1.0 文件、既有 REST API）需要維護到明確宣告淘汰為止。
- 三語言工具鏈自第 1 階段（Rust 外殼）起即存在。

### 風險與緩解

| 風險 | 影響 | 緩解措施 |
|---|---|---|
| Sidecar 體積（Python + OCCT 約 300–500 MB） | 第 1 階段的桌面版體驗不佳，影響採用 | 第 1 階段即完成 conda-pack／PyInstaller 比較與裁剪；將量測值列為完成條件；不可接受時提前評估第 4 階段 |
| 跨語言 schema 漂移 | 自第 1 階段起即有 Python 與 TypeScript 兩端 | 第 1 階段就建立生成管線與 CI 比對，而不是等到第 2 階段 |
| Windows WebView2 及其他平台 WebView 差異 | 檢視器或 UI 於特定平台失效 | 第 1 階段的測試矩陣涵蓋三個平台；WebView2 支援 WebGL，啟動時仍做能力偵測 |
| 大型網格的 IPC 負擔 | 互動延遲 | 自第 1 階段起即以檔案／二進位通道傳輸網格，不以 JSON 經 `invoke` 傳遞 |
| 第 2 階段範圍膨脹 | 重寫拖延，長期處於半遷移狀態 | 以「功能等價 + adapter」為界；新能力（約束、agent）嚴格留到第 3 階段 |
| 過渡狀態成為永久狀態 | 重複實作長期並存 | 每個階段的完成條件都包含「移除被取代的舊實作」 |
| 概念文件與實作進度脫節 | 2.0 模型在實作時才發現不可行 | 第 1 階段期間以範例文件與紙上推演驗證 2.0 模型；第 2 階段開始前確認 |

## 驗收或回顧條件

- **驗收**：各階段的「完成條件」即為驗收標準；每個階段結束時更新 `PROJECT_STATUS.md` 與 [`../ARCHITECTURE.md`](../ARCHITECTURE.md)，並逐份檢查 0002–0009 是否仍與實作一致。
- **回顧**：
  - 每個階段結束時。
  - 任何階段的完成條件被證明不可行或需要調整順序時（以新 ADR 取代本文）。
  - 第 1 階段的量測結果顯示 sidecar 成本不可接受時，重新評估第 4 階段的優先順序。

## 相關文件

- [0002](0002-desktop-shell-tauri-v2.md) ～ [0009](0009-agent-orchestration.md)
- [`overview.md`](overview.md)
- [`../ARCHITECTURE.md`](../ARCHITECTURE.md)、[`../CONCEPT_DIMENSIONAL_LADDER.md`](../CONCEPT_DIMENSIONAL_LADDER.md)
- `PROJECT_STATUS.md`

## 回顧紀錄

### 2026-11-26：第 1 階段切片已實作（部分達成完成條件）

已完成的內容：

- **前端**：`web/` 的 React + TypeScript（strict）+ Vite 應用程式，功能對齊 `app/static/`（提示詞與 planner 選擇、生成、JSON 編輯與驗證後重新輸出、驗證報告、產物下載與工作 ZIP、工作歷史、Token 欄位、SVG 預覽）。建置輸出到 `app/web_dist/`；FastAPI 在該目錄含 `index.html` 時於 `/` 提供它，否則維持原本的 `app/static/` 介面。
- **檢視器**：`web/src/viewer/` 的最小 Three.js 檢視器（`STLLoader` + `OrbitControls`、WebGL 能力偵測），核心不依賴 React（[0004](0004-3d-viewer-threejs.md)）。
- **Schema 管線**：`scripts/export_schema.py` 由 Pydantic 匯出 `schemas/promptcad.schema.json`（`--check` 偵測漂移），`npm --prefix web run gen:types` 以 json-schema-to-typescript 生成 `web/src/api/generated/promptcad.d.ts`；CI 比對兩者（[0008](0008-single-schema-source-of-truth.md) 的過渡安排）。
- **桌面外殼**：`src-tauri/` 的 Tauri v2 外殼，啟動時以 PyInstaller 打包的 FastAPI sidecar（`scripts/build_sidecar.py` → `src-tauri/binaries/`）在 `127.0.0.1` 的動態埠啟動，並傳入一次性 Token（[0002](0002-desktop-shell-tauri-v2.md)）；前端只在 `web/src/api/bootstrap.ts` 透過 `backend_info` 取得位址與 Token。
- **2.0 規格凍結**：[`../CAD_DSL_2.md`](../CAD_DSL_2.md) 與 `examples/cad2/` 的兩份手寫範例，滿足本 ADR「第 1 階段期間以範例文件與紙上推演驗證 2.0 模型」的緩解措施；2.0 的程式仍依本 ADR 在第 2 階段以 Rust 實作。

尚未達成的完成條件：

- **不需 Python／Conda／Docker 完成提示詞到 STEP／STL**：未達成。CadQuery 沒有打包進 sidecar，桌面版目前以 `source_only` 執行（產生 `spec.json`、`model.py`、`preview.svg` 等，不產生 STEP／STL）。
- **`app/static/` 已移除**：未達成，刻意保留作為沒有前端建置產物時的備援介面。
- **量測**（安裝包大小、sidecar 冷啟動時間、網格載入時間）：尚未量測。
- **conda-pack 與 PyInstaller 的比較**：尚未進行；目前先以 PyInstaller 建立切片。
- **三平台測試矩陣**：目前只在 Windows（x86_64，MSVC 工具鏈、WebView2）建置。

結論：第 1 階段尚未結束，本 ADR 的階段順序不變。下一步依序為 CadQuery 打包與 conda-pack 比較、量測、移除 `app/static/`，之後才開始第 2 階段的主要工作。

### 2026-11-26：第 1 階段 1b — CadQuery 打包與量測

- **不需 Python／Conda／Docker 完成提示詞到 STEP／STL**：在本機（Windows 11）達成。安裝包內含 PyInstaller onedir 的 sidecar 與 CadQuery 2.8.0；安裝後產生範例固定板得到 `completed／cadquery`，並輸出 STEP／STL／DXF／SVG。尚未在「完全乾淨、從未安裝過 Python 的機器」上驗證（本機 PATH 上仍有 Python，但凍結的 render worker 不使用它）。
- **量測**：已完成，見 [0006](0006-geometry-kernel-occt-python-sidecar.md) 的回顧紀錄（安裝包 107.0 MiB、安裝後 550.9 MiB、sidecar 啟動約 0.6–0.9 s、渲染約 2.2 s）。網格載入時間：STL 由檢視器以 HTTP 取得（範例 125 KB，17 ms），WebGL 實際繪製尚未以畫面確認。
- **conda-pack 與 PyInstaller 的比較**：選定 PyInstaller，conda-pack 以推估理由排除、未建原型，見 0006；是否視為完成原列為待負責人簽核，**負責人已於 2026-11-26 簽核**（見下一筆紀錄）。
- **CI**：新增 `windows-latest` 的 `desktop` job（`cargo fmt`／`clippy`／`test`）；完整安裝包改由手動或 `v*` tag 觸發的 `desktop-bundle.yml` 建置並上傳量測。
- 仍未達成：移除 `app/static/`、三平台測試矩陣。

### 2026-11-26：第 1 階段收尾 — 簽核、移除 `app/static/`、乾淨機器驗證與授權聲明

- **conda-pack 與 PyInstaller 的比較**：**已完成**。負責人於 2026-11-26 核准 PyInstaller onedir（見 [0006](0006-geometry-kernel-occt-python-sidecar.md) 回顧紀錄），上一筆紀錄的「待負責人簽核」就此結案。
- **`app/static/` 已移除**：**已完成**（負責人核准，見 [0003](0003-frontend-react-typescript.md) 回顧紀錄）。`app/web_dist/index.html` 不存在時，`/` 回傳 503 的內嵌說明頁（提示執行 `npm --prefix web run build`，並連到 `/docs`），`/assets` 不掛載，API 不受影響。Dockerfile 改為多階段建置（`node:24-slim` 建置 `web/`，再複製到 `/app/app/web_dist`），CI 新增 `docker` job 實際建置映像並確認 `/` 回傳含 `id="root"` 的新前端。凍結 sidecar 不再內含任何 Web UI（`build_sidecar.py` 的 `DATA_DIRS` 為空），桌面版的 UI 由 Tauri 自身的資產提供。
- **乾淨機器驗證**：本機無法使用 Windows Sandbox，改以 GitHub `windows-latest` runner 上的 `desktop-bundle.yml` 進行：`frozen_smoke.py --isolated-env` 讓 sidecar 只看到系統 `PATH`（`%SystemRoot%\System32;%SystemRoot%`）且移除 `PYTHON*`／`VIRTUAL_ENV`／`CONDA*`，並監看行程樹，任何位於 sidecar 目錄外或名為 `python*.exe`／`py.exe` 的子行程都會使檢查失敗（`measurements.json` 記錄 `"isolated_env": true`、`"foreign_processes": []`）；新的 `frozen_kill_check.py` 以設定 `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` 的 Job Object 啟動 sidecar，在 render worker 執行中關閉 Job，確認 sidecar 與 worker 於 5 秒內結束（補上 1b 記錄「關閉外殼時中斷渲染尚未實測」的缺口）。執行結果：2026-11-26 於 GitHub `windows-latest`（Windows Server 2025，4 CPU）首次執行 [run 45723089074](https://github.com/stevenke1981/sicad/actions/runs/45723089074) 全數通過：建置與安裝後的 sidecar 皆在 isolated env 下通過 smoke（各觀察到 7 個 render worker，`foreign_processes=[]`）；安裝 sidecar 的 Job Object 中斷檢查 `KILL_CHECK_OK`；強制結束已安裝的 `promptcad-studio.exe` 後 sidecar 於 0.007 秒內結束（`job.rs` 實證，此步驟已改為必過）；安裝目錄含 `THIRD_PARTY_NOTICES.md`。量測：安裝檔 107.5 MiB、安裝後 554.1 MiB、READY 0.64–0.66 s、warm render 2.38 s、端到端 generate 2.42 s。
- **授權聲明**：`THIRD_PARTY_NOTICES.md` 由 `scripts/gen_third_party_notices.py` 產生並隨安裝包發佈到 `<安裝目錄>\THIRD_PARTY_NOTICES.md`（見 0006）。
- **2D 預覽可讀性**：`model.svg`（CadQuery 投影，黑色線條）在前端改以米白紙色底（`#f3f1ea`）顯示；`preview.svg` 維持原本的深色底。只改前端，不改編譯器輸出。
- **仍未達成**：三平台測試矩陣（目前只在 Windows x86_64 建置）。乾淨機器驗證的結論以 `desktop-bundle.yml` 的實際執行結果為準。

結論：除三平台測試矩陣與乾淨機器驗證的 CI 結果外，第 1 階段的完成條件均已達成。

### 2026-11-26：三平台矩陣、VTK 精簡與建置環境鎖版

- **三平台測試矩陣**：依 [0011](0011-cross-platform-desktop.md) 實作。`desktop-bundle.yml` 改為 `windows-latest`、`macos-latest`（arm64）、`ubuntu-22.04` 三平台矩陣（`fail-fast: false`）；`ci.yml` 的 `desktop` job 改為三平台的 `cargo fmt`／`clippy`／`test`。macOS 產生 `.app`（ad-hoc 簽章）與 `.dmg`，Linux 產生 `.deb` 與 AppImage；unix 的「關閉外殼即終止 sidecar」以 stdin EOF + 行程群組實作（Windows 維持 Job Object）。首次執行的判定分級見 0011：三平台必過的項目為鎖檔安裝、凍結與精簡、onedir smoke、sidecar 模式終止檢查、授權聲明 `--strict` 與 `.exe`／`.app`／`.deb`；macOS／Linux 的外殼模式終止檢查、安裝後 smoke、`.dmg`、AppImage 先列為參考，連續兩次全綠後改為必過。
- **建置環境鎖版**：`.venv-desktop` 改由 `requirements/desktop-{windows-x86_64,macos-arm64,linux-x86_64}.txt`（`scripts/desktop_lock.py` 以 `uv pip compile --generate-hashes` 在同一台機器產生）以 `uv pip sync --require-hashes` 安裝；`ci.yml` 新增 `lock-check` job。以鎖檔在另一個目錄重建的環境，`uv pip freeze` 與本機 `.venv-desktop` 完全相同（70 個套件，不含 editable 的本專案）。
- **授權聲明**：提交的 `THIRD_PARTY_NOTICES.md` 為 Windows 權威版，可由鎖檔重現，Windows CI 改為 `--check --strict`（必過），上一筆紀錄「CI 不跑 `--check`」的限制就此解除；macOS／Linux 於 CI 以 `--strict` 重生並隨包發佈。`build_sidecar.py` 驗證確實未凍結的套件（trame*、wslink、aiohttp 及其相依、matplotlib、scipy 等）不再列入，Python 套件由 60 個降為 39 個。
- **VTK 精簡**：`build_sidecar.py` 依 CadQuery 實際載入的 `vtkmodules` 與原生相依閉包刪除未使用的 VTK 檔案，Windows onedir 目標 ≤ 460 MiB（量測見 `PROJECT_STATUS.md`）。
- **仍未達成**：macOS／Linux 的實際執行結果尚待 `desktop-bundle.yml` 首次三平台執行；在此之前三平台測試矩陣視為「已實作、待驗證」。

### 2026-11-26：第 2 階段 2a — Rust core crate

已完成的內容：獨立 crate `crates/promptcad-core/`（不依賴 Tauri；`cargo test` 於本機通過 253 個測試，`cargo fmt --check` 與 `cargo clippy -D warnings` 通過）依 [`../CAD_DSL_2.md`](../CAD_DSL_2.md) 實作 CadDocument 2.0 型別、運算式與綁定、port 表與求解器、分層驗證器、patch 引擎（含 §8.5 權限矩陣與復原）、op log、1.0 ↔ 2.0 轉接，以及 `promptcad-core` 命令列工具；JSON Schema 由 Rust 型別生成並提交於 `schemas/cad2/`；`ci.yml` 新增 `core` job（`cargo fmt --check`、`clippy`、`cargo test`、schema 漂移檢查）。細節見 `PROJECT_STATUS.md`。

對照第 2 階段的完成條件：

- **所有 `examples/generated/` 的 1.0 文件經 adapter 後輸出等價幾何**：**部分達成**。`examples/generated/` 目前只有 `plate-four-holes` 一份；它可經 `upgrade_1_to_2` 升級並通過 2.0 驗證，`downgrade_2_to_1` 往返後與原文件一致（`crates/promptcad-core/tests/migrate_roundtrip.rs`、`migrate_upgrade.rs`）。但「1.0 直接輸出與經轉接後輸出在 CadQuery 上幾何等價（體積、包圍盒）」的測試尚未進行，Python 編譯器也尚不接受 2.0，因此幾何等價尚未驗證。
- **Python 與 Rust 驗證器對既有測試案例判定一致後，移除 Python 端的設計驗證邏輯**：**部分達成**。一致性已驗證：49 個案例（`tests/fixtures/v1_parity.json`，由 `gen_v1_parity.py` 實際執行 Python `DesignValidator` 產生）在 Rust 驗證器得到相同判定（`tests/validate_parity_v1.rs`）。移除未做：Rust core 尚未接入執行中的系統，`app/services/validator.py` 仍是實際使用的驗證器。已知差異：升級後的文件若 1.0 螺紋無效，回報 `tapped_hole_without_thread` 而非 1.0 的 `thread_annotation_only`。
- **CI 的 schema 生成比對檢查已啟用**：**部分達成**。`ci.yml` 的 `core` job 含 `cargo run -- schema --out-dir ../../schemas/cad2 --check`，但此 job 尚未在 GitHub 上執行過，且比對的只是 Rust → JSON Schema；Pydantic 與 TypeScript 的生成物尚不存在（見下一項）。
- **REST API 與 CLI 在伺服器模式下仍可用**：**達成（但屬未變動）**。Python 的 REST API、`promptcad` CLI 與 1.0 流程完全未修改；相容層（讓 2.0 流程接受 1.0 文件）尚不存在，因為 Rust core 尚未被伺服器呼叫。

第 2 階段「內容」中尚未開始或未完成的項目：

- **Schema 來源切換為 Rust、Pydantic 與 TypeScript 皆為生成物**：**未達成**。來源仍是 `app/models/cad.py`；2.0 的 Pydantic／TypeScript 生成型別不存在。
- **編譯器改為接受 2.0 的已驗證規格**：**未達成**。
- **前端元件樹、屬性編輯、檢視器的元件選取與 port 顯示**：**未開始**。
- **決定伺服器模式下 Rust core 的宿主形式（新增 ADR）**：**未達成**。
- 核心層（kernel）驗證代碼未實作，屬於 sidecar 的工作。

結論：第 2 階段尚未結束，本 ADR 的階段順序不變。下一步依序為把 Rust core 接進 Tauri 命令並以新 ADR 決定伺服器宿主、切換 schema 來源並生成 Pydantic／TypeScript、讓 Python 編譯器接受 2.0 並完成幾何等價測試，之後才移除 Python 驗證器並開始前端元件樹。

### 2026-11-26：第 2 階段 2b — Rust core 接入 Tauri 與伺服器

2b 的內容與細節見 `PROJECT_STATUS.md` 的「第 2 階段 2b 切片」。逐項對照第 2 階段的完成條件：

- **所有 `examples/generated/` 的 1.0 文件經 adapter 後輸出等價幾何**：**仍為部分達成**。2b 未新增幾何等價測試；Python 編譯器仍不接受 2.0。
- **Python 與 Rust 驗證器判定一致後，移除 Python 端的設計驗證邏輯**：**未達成**。`app/services/validator.py` 仍是 1.0 的實際驗證器；2.0 的驗證走 `POST /api/v1/core`，但 1.0 路徑未改。
- **CI 的 schema 生成比對檢查已啟用**：**部分達成**。Rust → `schemas/cad2/` 的漂移檢查在 `core` job；2.0 的 core 請求／回應 TypeScript 型別由 `web` job 檢查漂移。Pydantic 生成物仍不存在。CI 在 GitHub 上的執行結果本紀錄撰寫時未確認。
- **REST API 與 CLI 在伺服器模式下仍可用**：**已確認的部分**。`docs/API.md` 既有端點與 CLI 的 1.0 流程未變動，並新增 `POST /api/v1/core`。本次未確認應用程式是否對 1.0 文件走 core 的 `upgrade` 操作；core 本身提供該操作。

第 2 階段「內容」的狀態：

- **Rust core 接入 Tauri 與伺服器**：**已完成**（Tauri 命令 `core_request` 與 `POST /api/v1/core`，宿主形式見 [0012](0012-server-host-for-rust-core.md)，待負責人簽核）。
- **伺服器宿主形式（新增 ADR）**：**已記錄**，[0012](0012-server-host-for-rust-core.md) 待簽核。
- **前端**：**部分完成**。唯讀元件樹已完成；屬性編輯、檢視器元件選取與 port 顯示尚未開始。
- **Schema 來源切換為 Rust**：**未達成**。來源仍是 `app/models/cad.py`；2.0 的 Pydantic 生成型別不存在；TypeScript 的 core 請求／回應型別已生成。
- **編譯器改為接受 2.0 的已驗證規格**：**未達成**。2.0 文件的輸出在前端停用。

結論：第 2 階段尚未結束，階段順序不變。2b 之後的剩餘項目與驗證缺口見 `PROJECT_STATUS.md`。
