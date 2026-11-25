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
