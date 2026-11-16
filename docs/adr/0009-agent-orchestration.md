# 0009：Agent 編排：Claude API tool use + structured outputs，僅輸出 patch JSON

- **狀態**：Accepted
- **日期**：2026-11-17
- **決策者**：專案負責人
- **影響範圍**：`app/services/planners/`、Rust core 的編排模組、金鑰管理、[`../SECURITY.md`](../SECURITY.md)

## 背景

現行規劃流程是單次呼叫：`RuleBasedPlanner` 或 `OpenAICompatiblePlanner` 把提示詞轉為 `LLMIntent`，再正規化為整份 `CadDocument 1.0`。LLM 只填寫受 schema 約束的 JSON，從不輸出程式碼；確定性編譯器負責幾何。LLM 未設定或呼叫失敗時，退回本地規則解析器。OpenAI-compatible planner 支援 `json_schema`、`json_object` 與 `prompt_only` 三種結構化模式，以因應不同供應商。

這個安全原則是專案的核心，必須保留。但單次、整份文件的產生方式無法支撐目標情境（[`../CONCEPT_DIMENSIONAL_LADDER.md`](../CONCEPT_DIMENSIONAL_LADDER.md)）：

- Agent 需要**逐步**建構：新增元件、設定參數、連接 port、加入約束，並在每一步看到驗證結果。
- Agent 需要**查詢**目前文件狀態（元件清單、可用的 port、未滿足的約束），而不是每次重新產生整份文件。
- 多個角色的 agent（規劃、建模、審查）需要協作，每個修改都要可追溯、可復原（[0005](0005-rust-core-document-model.md) 的 op log）。

## 決策

### 1. 互動模型

採用 **Claude API 的 tool use 搭配 structured outputs** 作為主要的 agent 編排機制。

- Agent 可用的工具分兩類：
  - **讀取類**：查詢文件、元件、port、約束與驗證狀態。唯讀。
  - **提交類**：提交一個 patch。Patch 的輸入 schema 來自 [0008](0008-single-schema-source-of-truth.md) 的管線。
- **Agent 只能輸出 schema 約束的 patch JSON**。不提供執行程式碼、存取檔案系統、發出網路請求或執行 shell 的工具。這是現行「LLM 絕不輸出程式碼」原則在 patch 層級的延續。
- 每個 patch 都經 Rust core 驗證後才原子性套用。驗證失敗時，結構化診斷作為 tool result 回傳，由 agent 修正後重試；重試次數有上限。
- Structured output 保證的是「語法上符合 schema」，不是「設計有效」。供應商支援的 JSON Schema 子集有其限制（以官方文件為準），因此 Rust 驗證器永遠是最終閘門，不因啟用 structured output 而放寬。
- 每次 agent 修改都以其身分寫入 op log，使用者可檢視、接受或復原。

### 2. 呼叫位置：預設由 Rust core 以 HTTP 呼叫

兩種可行位置的取捨如下：

| 面向 | 由 Rust core 經 HTTP 呼叫 | 由 Python sidecar 呼叫 |
|---|---|---|
| 與 patch 引擎的距離 | 同程序；工具呼叫直接對應 core 函式 | 每個工具呼叫都要從 Python 回呼 Rust，多一次跨程序往返 |
| 金鑰處理 | 可存放於作業系統的憑證儲存區，只在 Rust 程序內使用 | 金鑰需傳入 sidecar，擴大 sidecar 的權限與攻擊面 |
| Sidecar 的權責 | 維持「規格 → 幾何 → 匯出」，不需對外網路 | 變成同時負責幾何與對外網路，違反 [0006](0006-geometry-kernel-occt-python-sidecar.md) 的隔離目標 |
| 第 4 階段（移除 Python） | 不受影響 | 編排邏輯需重寫 |
| SDK 與現有程式碼 | 需確認是否有官方 Rust SDK；若無，以 HTTP 用戶端直接呼叫 Messages API，自行處理串流、重試與 tool use 迴圈 | 可使用官方 Python SDK；可沿用 `app/services/planners/` 既有程式碼與 `httpx` |
| 迭代速度 | 編排邏輯變更需重新編譯 | 修改較快 |
| 伺服器模式 | 與桌面版共用同一實作 | 與現行架構一致 |

**預設選擇：由 Rust core 以 HTTP 呼叫。** 理由：編排的本質是「讀取文件狀態 → 提交 patch → 取得驗證結果」的迴圈，這三件事的權威都在 Rust core；把迴圈放在同一處可避免雙向跨程序呼叫，讓 sidecar 保持無網路、單一職責，並且不阻礙第 4 階段。代價是失去官方 Python SDK 的便利，需要自行維護一層薄的 API 用戶端。

**過渡安排**：第 1、2 階段沿用現有 Python planner（單次產生整份文件），不做搬遷。Rust 端的編排於第 3 階段隨 patch 引擎就緒後導入。Python planner 屆時保留為相容路徑，直到 Rust 編排涵蓋其功能。

### 3. 供應商可插拔

- 編排層對模型供應商定義一個內部介面：輸入訊息、工具定義與輸出 schema，輸出文字、工具呼叫與用量。
- Claude 是預設且優先支援的實作。
- **保留 OpenAI-compatible 路徑**，包含其三種結構化模式與本地端點（例如 llama.cpp server）的支援；不支援 tool use 的端點退化為單次 patch 產生。
- **保留本地規則解析器**作為無金鑰、離線時的備援，維持 README 所述「沒有 API Key 也能使用」。
- 模型名稱與端點由設定決定，不寫死於程式碼；manifest／op log 記錄每次呼叫實際使用的供應商與模型。

### 4. 多 agent

多 agent 協作（第 3 階段）以**同一份文件、同一個 patch 閘門、同一份 op log** 為協調基礎：agent 之間不直接交換自由格式的修改，而是各自提交 patch。角色劃分、排程與衝突處理的細節在第 3 階段設計，必要時另立 ADR。

## 考慮過的替代方案

| 方案 | 優點 | 缺點 | 結論 |
|---|---|---|---|
| **Tool use + structured outputs，agent 只輸出 patch** | 延續既有安全模型；逐步建構並即時取得驗證回饋；每步可稽核、可復原 | 多輪呼叫的延遲與成本高於單次產生；需設計工具與 patch 詞彙 | 採用 |
| 維持單次產生整份文件 | 簡單、呼叫次數少 | 文件變大後輸出不穩定；無法局部修改；無法支援多 agent | 僅保留為備援路徑 |
| 讓 LLM 直接產生 CadQuery／Python 程式碼 | 表達力最大 | 任意程式碼執行、不可重現、無法於執行前驗證；與專案根本原則牴觸（見 [`../ARCHITECTURE.md`](../ARCHITECTURE.md)「為何加入 DSL」） | 不採用 |
| 使用通用 agent 框架（LangChain、LangGraph 等） | 現成的編排、記憶與多 agent 元件 | 以 Python／JS 為主，與 Rust core 的位置不合；抽象層厚、相依多；本專案的迴圈很單純 | 不採用 |
| 使用可執行檔案與 shell 操作的通用 agent 執行環境 | 能力完整、開發快 | 賦予 agent 遠超所需的權限；與「只輸出 patch」衝突 | 不採用 |
| 以 MCP server 暴露文件工具 | 標準協定；外部 agent 也能操作文件 | 目前只有內建 agent 一個用戶端，協定層是額外成本 | 現在不採用；工具介面設計保持可對應 MCP，日後可加 |
| 由前端（WebView）直接呼叫模型 API | 實作最直接、串流顯示容易 | 金鑰暴露於 WebView；繞過 Rust core 的閘門 | 不採用 |
| 由 Python sidecar 呼叫 | 官方 SDK、沿用現有程式碼 | 見上方取捨表 | 不採用為預設；為第 1、2 階段的現況 |

## 後果

### 正面

- 任何 agent 的任何修改都經過同一個確定性閘門，安全性質不依賴模型的行為。
- 驗證診斷成為 agent 的回饋訊號，可自我修正而不需人工介入每一步。
- 操作紀錄讓使用者能理解並復原 agent 的作為，支撐 README 所要求的人工覆核。
- 供應商介面使離線、本地模型與其他雲端供應商的路徑得以保留。

### 負面

- 多輪工具呼叫增加延遲與 token 成本。
- 需自行維護 Rust 端的 API 用戶端、tool use 迴圈、串流與錯誤處理。
- 不同供應商的能力差異（tool use、structured output 的 schema 限制）使「可插拔」需要明確的能力宣告與降級路徑，測試矩陣擴大。
- 在第 3 階段之前，Python 與 Rust 各有一條 LLM 呼叫路徑。

### 風險與緩解

| 風險 | 影響 | 緩解措施 |
|---|---|---|
| 提示注入（例如來自使用者匯入的文件內容或備註欄位） | Agent 被誘導提交非預期修改 | Agent 沒有 patch 以外的副作用能力；所有 patch 經驗證並記錄；破壞性操作需使用者確認；文件中的自由文字欄位維持長度上限 |
| Agent 陷入驗證失敗的重試迴圈 | 成本失控、無進展 | 每個任務設定重試與 token 上限；超限時停止並向使用者回報診斷 |
| 金鑰外洩 | 帳務與資料風險 | 桌面版存於作業系統憑證儲存區；伺服器版以環境變數注入；延續現行「不把 API Key 寫入 manifest」；金鑰不進入 WebView 與 sidecar |
| LLM 用 schema 與正式 schema 的落差 | 結構合法但被驗證器拒絕的輸出增多 | 見 [0008](0008-single-schema-source-of-truth.md)；以診斷回饋重試；追蹤首次通過率 |
| 供應商 API 變動 | 編排中斷 | 供應商實作隔離於介面之後；固定 API 版本標頭；契約測試 |
| 設計內容送往雲端的隱私疑慮 | 部分使用者無法使用 | 明確告知傳送內容；保留本地模型與規則解析器路徑 |
| 無網路或服務中斷 | Agent 功能不可用 | 手動編輯與本地規則解析器不依賴網路；降級行為明確顯示於 UI |

## 驗收或回顧條件

- **驗收**：
  - Agent 可用的工具清單中沒有任何程式碼執行、檔案、網路或 shell 工具。
  - 一個 agent 任務的所有修改都能在 op log 中逐筆對應，並可整體復原。
  - 刻意製造的無效 patch 不會改變文件，且 agent 能依診斷修正。
  - 切換為 OpenAI-compatible 端點或本地規則解析器時，仍能完成 README 中的範例提示詞。
  - 金鑰不出現在 WebView、sidecar 環境、manifest 或 op log。
- **回顧**：
  - 出現官方且穩定的 Rust SDK 時，評估以其取代自行維護的用戶端。
  - Rust 端編排的迭代成本明顯拖慢 agent 行為的調整時，重新評估呼叫位置。
  - 有外部 agent 操作文件的需求時，評估 MCP。
  - 多 agent 的角色與排程設計確定時（必要時新增 ADR）。

## 相關文件

- [0005](0005-rust-core-document-model.md)、[0006](0006-geometry-kernel-occt-python-sidecar.md)、[0008](0008-single-schema-source-of-truth.md)、[0010](0010-migration-phases.md)
- [`../CONCEPT_DIMENSIONAL_LADDER.md`](../CONCEPT_DIMENSIONAL_LADDER.md)
- 現行實作：`app/services/planners/`；設定見 README「LLM 設定」
- [`../SECURITY.md`](../SECURITY.md)、[`../PROMPT_GUIDE.md`](../PROMPT_GUIDE.md)
- Claude API 官方文件：tool use、structured outputs

## 回顧紀錄

（尚無）
