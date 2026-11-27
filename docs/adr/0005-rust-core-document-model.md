# 0005：文件模型、驗證器與 patch／操作紀錄引擎以 Rust 實作

- **狀態**：Accepted
- **日期**：2026-11-17
- **決策者**：專案負責人
- **影響範圍**：新增 Rust core crate；`app/models/cad.py`、`app/services/validator.py` 的權責移轉；[`../CAD_DSL.md`](../CAD_DSL.md)

## 背景

現行文件模型是 `app/models/cad.py` 中的 Pydantic `CadDocument 1.0`：單一 `base` 加上 `holes`／`fillets`／`chamfers` 三個扁平清單，所有 model `extra="forbid"`，數值有上限（`MAX_DIMENSION_MM`）、清單有長度上限。驗證分兩層：Pydantic schema 與 `DesignValidator`；只要有 `error`，renderer 就不啟動。編輯方式是整份 `spec.json` 取代後重新驗證。

目標模型（[`../CONCEPT_DIMENSIONAL_LADDER.md`](../CONCEPT_DIMENSIONAL_LADDER.md)）是 CadDocument 2.0：以 Empty／Dot／Line／Plane／Volume 為基本元件，由 agent 參數化、透過 port 與約束連接並組裝。這帶來 1.0 沒有的需求：

- **元件圖**與**約束圖**：需要參照完整性、循環偵測、相依順序。
- **增量編輯**：多個 agent 與使用者輪流修改，必須以 patch 為單位驗證、套用、復原，而非整份覆寫。
- **操作紀錄（op log）**：記錄誰（哪個 agent／使用者）在何時套用了哪個 patch，作為復原、稽核與重播的依據。
- **多宿主**：同一套邏輯要在 Tauri 桌面程序內與伺服器模式執行（見 [0002](0002-desktop-shell-tauri-v2.md)）。

若這些邏輯留在 Python，桌面版的每次按鍵式編輯都要跨程序往返 sidecar，而且 Python 永遠無法被移除（見 [0010](0010-migration-phases.md) 第 4 階段）。

## 決策

我們以 **Rust** 實作核心（以下稱 Rust core），範圍為：

1. **文件模型**：CadDocument 2.0 的型別定義，使用 `serde` 序列化、`schemars` 匯出 JSON Schema（見 [0008](0008-single-schema-source-of-truth.md)）。延續 1.0 的嚴格性：拒絕未知欄位（`deny_unknown_fields`）、數值與集合大小有界、單位固定為 mm。
2. **元件圖與約束圖**：元件、port、連接與約束的資料結構，以及參照完整性、循環偵測與拓樸排序。
3. **驗證器**：承接 `DesignValidator` 的閘門角色，分為 schema 層、圖結構層與設計規則層。語意不變：存在 `error` 則不得送往幾何核心；`warning` 不阻擋但標記 `review_required`。
4. **Patch 引擎**：patch 是 schema 約束的操作清單。套用為原子性：先在副本上套用並完整驗證，全數通過才提交，否則整個 patch 拒絕並回傳結構化診斷。
5. **操作紀錄**：僅附加（append-only）的 op log，每筆記錄來源（使用者／agent 身分）、patch 內容、前後修訂識別碼與驗證結果；支援復原與重播。
6. **1.0 → 2.0 轉接**：讀取 1.0 文件並轉為 2.0 的 adapter，使既有 `spec.json` 與範例可繼續使用。

邊界與限制：

- Rust core 是**不依賴 Tauri、不做 I/O 以外副作用**的函式庫 crate。桌面模式由 Tauri command 呼叫；伺服器模式由一個薄宿主暴露相同操作。伺服器宿主的具體形式（獨立 HTTP 服務，或以 PyO3 供 FastAPI 載入）留待第 2 階段開始時決定，並以新 ADR 記錄。
- Rust core **不包含幾何核心**。它輸出已驗證的規格給 Python sidecar（見 [0006](0006-geometry-kernel-occt-python-sidecar.md)），並接收產物與診斷。
- 需要幾何核心才能判斷的檢查（實際 B-rep 有效性、布林運算失敗、干涉）不屬於 Rust 驗證器，而是 sidecar 回傳的診斷，以同一種 issue 結構呈現。
- 約束**求解**不在此範圍（見 [0007](0007-constraint-solver.md)）；Rust core 只擁有約束圖的表示與一致性檢查。

## 考慮過的替代方案

| 方案 | 優點 | 缺點 | 結論 |
|---|---|---|---|
| **Rust core（serde + schemars）** | 可直接內嵌 Tauri；代數資料型別與窮舉比對適合表達元件／操作的 tagged union；無 GC、可預期的效能；可編譯為 WASM 供瀏覽器端使用；為第 4 階段移除 Python 鋪路 | 團隊需具備 Rust 能力；既有 Pydantic 模型與驗證器需重寫；跨語言邊界增加 | 採用 |
| 維持 Python／Pydantic 為核心 | 無需重寫；與 CadQuery 同程序 | 桌面版所有編輯都要經 sidecar；無法嵌入 WebView；Python 永遠是必要相依；大型圖的驗證效能受限 | 不採用 |
| TypeScript 核心（前後端共用） | 前端零成本共用；單一語言涵蓋 UI 與模型 | 伺服器與 CLI 需要 Node 執行環境；與 Tauri 外殼仍有程序／語言邊界；數值與圖演算法的工具較弱 | 不採用 |
| C++ 核心（與 OCCT 同語言） | 第 4 階段可與 OCCT 直接整合 | 記憶體安全與建置複雜度；schema 生成與序列化工具不如 serde 生態 | 不採用 |
| 採用現成 CRDT 文件模型（Automerge／Yjs）作為核心 | 內建合併與歷史 | 目前沒有多人即時協作需求；CRDT 的自動合併可能繞過「先驗證再提交」的閘門；仍需自建領域型別與驗證 | 不採用；有協作需求時再評估 |
| JSON Patch（RFC 6902）作為 patch 格式 | 標準、工具多 | 以路徑為基礎的通用操作難以表達領域意圖，也難以針對操作種類做權限與驗證；LLM 容易產生無效路徑 | 不採用：改用領域特定的具型別操作 |

## 後果

### 正面

- 文件編輯、驗證與復原在桌面程序內完成，回饋即時，不受 sidecar 啟動或幾何運算時間影響。
- 「LLM 只能輸出 schema 約束的資料、由確定性程式執行」的既有安全原則被延伸到 patch 層級：任何來源的修改都走同一條驗證路徑。
- op log 提供稽核軌跡與可重現性，呼應現行 manifest 的 provenance 設計。
- 核心與宿主分離後，可獨立做屬性測試與 fuzzing。

### 負面

- 第 2 階段是實質重寫：`app/models/cad.py` 的型別、`app/services/validator.py` 的規則，以及 rule／LLM planner 對 1.0 模型的依賴都需要調整。
- 專案從單一語言變為三種語言；貢獻門檻提高。
- 遷移期間 Python 與 Rust 兩套驗證邏輯會短暫並存。

### 風險與緩解

| 風險 | 影響 | 緩解措施 |
|---|---|---|
| 跨語言 schema 漂移 | Rust、Python、TypeScript 對同一文件的解讀不一致 | 單一事實來源與 CI 檢查（見 [0008](0008-single-schema-source-of-truth.md)） |
| 驗證規則移植時行為改變 | 原本被擋下的無效設計通過，或反之 | 以現有測試與 `examples/generated/` 作為黃金案例，對 Python 與 Rust 驗證器做差異測試，結果一致後才移除 Python 版 |
| 1.0 → 2.0 轉接遺失資訊 | 既有文件無法重現相同幾何 | 對所有範例做 1.0 直接輸出與經轉接後輸出的幾何比對（體積、包圍盒） |
| Patch 操作集合設計不當 | 過細則 agent 容易出錯，過粗則難以復原與審查 | 先以少量操作涵蓋核心情境，op log 記錄 schema 版本以支援日後演進 |
| op log 無限成長 | 檔案與載入時間膨脹 | 支援快照與截斷策略；快照保留修訂識別碼 |

## 驗收或回顧條件

- **驗收**：
  - Rust core crate 可在沒有 Tauri 與 Python 的環境下建置並通過測試。
  - `examples/generated/` 的所有 1.0 文件可經 adapter 轉為 2.0 並通過驗證，輸出幾何與原流程一致。
  - 無效 patch 不會改變文件狀態，且回傳可供 agent 修正的結構化診斷。
  - 任一已套用的 patch 可復原，復原後文件與套用前逐位元組相同（正規化序列化後）。
  - `DesignValidator` 的既有測試案例在 Rust 驗證器上得到相同的 error／warning 判定。
- **回顧**：
  - 出現多人即時協作需求（重新評估 CRDT）。
  - 伺服器宿主形式決定時（新增 ADR）。
  - Rust 的開發速度明顯成為功能交付的瓶頸。

## 相關文件

- [0006](0006-geometry-kernel-occt-python-sidecar.md)、[0007](0007-constraint-solver.md)、[0008](0008-single-schema-source-of-truth.md)、[0009](0009-agent-orchestration.md)、[0010](0010-migration-phases.md)
- [`../CONCEPT_DIMENSIONAL_LADDER.md`](../CONCEPT_DIMENSIONAL_LADDER.md)
- [`../CAD_DSL.md`](../CAD_DSL.md)、[`../ARCHITECTURE.md`](../ARCHITECTURE.md)
- 現行實作：`app/models/cad.py`、`app/services/validator.py`

## 回顧紀錄

### 2026-11-26：第 2 階段 2a — Rust core crate 實作

`crates/promptcad-core/` 已實作（`cargo test` 於本機通過 253 個測試）。逐項對照驗收條件：

- **Rust core crate 可在沒有 Tauri 與 Python 的環境下建置並通過測試**：**達成**。crate 不依賴 Tauri，自有 `Cargo.lock`；`cd crates/promptcad-core && cargo test` 通過。唯一與 Python 有關的是 `tests/fixtures/gen_v1_parity.py`，它只用於重新產生已提交的 `v1_parity.json`，執行 `cargo test` 不需要 Python。CI 的 `core` job 已加入但尚未在 GitHub 上執行。
- **`examples/generated/` 的所有 1.0 文件經 adapter 轉為 2.0 並通過驗證，輸出幾何與原流程一致**：**部分達成**。`examples/generated/` 只有 `plate-four-holes`；升級後通過驗證（`tests/migrate_upgrade.rs`），往返不變式有屬性測試（`tests/migrate_roundtrip.rs`）。幾何一致（CadQuery 上的體積、包圍盒比對）**尚未驗證**，因為 Python 編譯器尚不接受 2.0。
- **無效 patch 不會改變文件狀態，且回傳結構化診斷**：**達成**。patch 以原子方式套用，拒絕時回傳拒絕原因與 issue 清單，文件不變（`tests/patch_engine.rs`、`tests/patch_permissions.rs`，後者逐格檢查 §8.5 權限矩陣）。限制：`human` 來源不受權限矩陣與 `annotation_only_removal` 限制。
- **任一已套用的 patch 可復原，復原後文件與套用前逐位元組相同（正規化序列化後）**：**達成**。`tests/patch_engine.rs` 的 `undo_restores_the_pre_image_byte_for_byte` 與 `tests/oplog.rs`（含 proptest 與重播）驗證。復原是在 op log 追加一筆還原前一修訂號的新紀錄；沒有 redo。
- **`DesignValidator` 的既有測試案例在 Rust 驗證器上得到相同的 error／warning 判定**：**達成（限於 1.0 規則的 49 個案例）**。`tests/validate_parity_v1.rs` 對照由實際執行 Python 驗證器產生的 `tests/fixtures/v1_parity.json`。已知差異：升級後的文件若 1.0 螺紋無效，回報 `tapped_hole_without_thread` 而非 `thread_annotation_only`（thread 已被 §10.1 正規化設為 null）。

其餘對應 ADR 內容的狀態：

- 2.0 型別、運算式與綁定、port 表與求解器（輸出與 `examples/cad2/` 兩份 `expected_resolved` 一致，`tests/solver_golden.rs`）、分層驗證器、patch 引擎、op log 與 1.0 ↔ 2.0 轉接皆已實作。
- 解析檢查對 `extrude`、`revolve` 與沉頭孔錐體使用保守的外包圖元：可能漏報，不會憑空產生 issue。核心層（kernel）驗證代碼未實作，屬於 sidecar 的工作。
- Rust core 尚未接入 Tauri 命令或 FastAPI 伺服器；伺服器宿主形式仍待新 ADR（本 ADR 的回顧條件之一）。Python 驗證器尚未移除，兩套驗證邏輯目前並存。

### 2026-11-26：伺服器宿主形式的回顧條件已觸發

上一筆紀錄所述「伺服器宿主形式仍待新 ADR」的回顧條件已觸發，並記錄於 [0012](0012-server-host-for-rust-core.md)（待負責人簽核）：桌面端由 Tauri 命令行程內呼叫 core；伺服器端由 FastAPI 每次呼叫啟動一次 `promptcad-core rpc` 子行程，兩者共用 `promptcad_core::api` 的同一契約。上一筆紀錄中「Rust core 尚未接入 Tauri 命令或 FastAPI 伺服器」已不再成立。Core 仍不含幾何核心；Python 驗證器尚未移除，1.0 驗證路徑未改變。

