# 0008：以 Rust serde 型別作為 schema 單一事實來源

- **狀態**：Accepted
- **日期**：2026-11-17
- **決策者**：專案負責人
- **影響範圍**：Rust core 型別、schema 產物、Python sidecar 的 Pydantic 模型、前端 TypeScript 型別、LLM structured output、CI

## 背景

目前 schema 的唯一定義是 `app/models/cad.py` 的 Pydantic 模型，同一份定義同時用於 API 驗證、`spec.json` 讀寫，以及 OpenAI-compatible planner 的 `json_schema` 結構化輸出模式。單一語言時，這自然是單一來源。

目標架構中，同一份文件會被三種語言處理：

- **Rust**：文件模型、驗證、patch 引擎（[0005](0005-rust-core-document-model.md)）。
- **Python**：sidecar 讀取已驗證規格以建立幾何（[0006](0006-geometry-kernel-occt-python-sidecar.md)）。
- **TypeScript**：前端顯示與編輯（[0003](0003-frontend-react-typescript.md)）。

此外還有第四個消費者：**LLM**。Agent 的輸出必須受 schema 約束（[0009](0009-agent-orchestration.md)）。

若三種語言各自手寫型別，漂移是必然的：欄位改名漏改一處、列舉值不一致、可選性不同。這類錯誤在執行期才以「未知欄位」或靜默丟失資料的形式出現。

## 決策

**以 Rust 的 serde 型別為唯一事實來源**，其餘皆為生成產物：

```text
Rust 型別（serde + schemars）
        │  schemars
        ▼
   JSON Schema  ──────────────→ LLM tool 定義／structured output
        │
        ├─ datamodel-code-generator ──→ Pydantic 模型（Python sidecar）
        └─ json-schema-to-typescript ─→ TypeScript 型別（前端）
```

規則：

1. **涵蓋範圍**：CadDocument 2.0、patch／操作型別、驗證 issue、sidecar 的請求／回應訊息，全部由此管線產生。
2. **生成物進版控**：JSON Schema、生成的 Pydantic 模型與 TypeScript 型別都提交到 repo，檔頭標示「自動生成，請勿手動編輯」。CI 重新執行生成並比對，有差異即失敗。
3. **嚴格性延續**：Rust 型別使用 `deny_unknown_fields`，使 JSON Schema 帶有 `additionalProperties: false`，對應現行 Pydantic 的 `extra="forbid"`。數值範圍與集合長度上限以 schemars 屬性表達並輸出到 schema。
4. **Schema 不是完整的驗證**：JSON Schema 無法表達的跨欄位規則（例如現行 `RingBase` 的 `inner_diameter < outer_diameter`、盲孔必須有 `depth`）只存在於 Rust 驗證器。Python 與 TypeScript 端不得各自重新實作這些規則；它們只處理已由 Rust core 驗證過的資料，或把資料送回 Rust core 驗證。
5. **LLM 用的 schema 是衍生視圖**：各家供應商的 structured output 只支援 JSON Schema 的子集（確切限制以供應商文件為準）。因此由正式 schema 經確定性的轉換步驟產生「LLM 相容」版本；被轉換移除的約束仍由 Rust 驗證器把關。
6. **版本化**：文件保留 `schema_version` 欄位。破壞相容的變更需提升版本並提供 adapter（如 1.0 → 2.0）；op log 記錄寫入時的 schema 版本。
7. **過渡期**：第 1 階段尚無 Rust core，`app/models/cad.py` 仍是 1.0 的事實來源；此時前端的 TypeScript 型別由 Pydantic 匯出的 JSON Schema 生成，以便先建立下游管線。第 2 階段起來源切換為 Rust，Pydantic 模型改為生成物。

## 考慮過的替代方案

| 方案 | 優點 | 缺點 | 結論 |
|---|---|---|---|
| **Rust serde → JSON Schema → Pydantic／TypeScript** | 來源與執行驗證的程式碼是同一份型別；JSON Schema 是三個下游（Python、TS、LLM）共同的中介格式；工具成熟 | 生成管線多一層；JSON Schema 表達力有限；生成的程式碼可讀性普通 | 採用 |
| Pydantic 維持為來源，生成 Rust 與 TypeScript | 不需改動現有模型；Pydantic 匯出 JSON Schema 很成熟 | 權威邏輯在 Rust 而來源在 Python，方向相反；由 JSON Schema 生成慣用的 Rust 型別（tagged enum 等）品質不佳；與第 4 階段移除 Python 衝突 | 不採用（僅作第 1 階段過渡） |
| 手寫 JSON Schema 為來源，三端皆生成 | 語言中立 | 手寫大型 schema 易錯且冗長；Rust 端的生成品質問題同上 | 不採用 |
| Protocol Buffers | 跨語言生成成熟、二進位格式精簡、有相容性規則 | 文件需為人類可讀、可差異比對的 JSON；無法直接作為 LLM structured output 的 schema；範圍約束需另行擴充 | 不採用 |
| TypeSpec 等介面定義語言 | 專為 API／schema 設計，可輸出多種格式 | 新增一種語言與工具鏈；Rust 型別仍需生成或手寫，來源與驗證程式碼分離 | 不採用 |
| ts-rs／specta／typeshare（Rust 直接生成 TypeScript） | 生成的 TS 型別較貼近 Rust 原意；與 Tauri command 整合佳 | 只解決 Rust → TS，不涵蓋 Python 與 LLM；會形成兩條平行管線 | 不作為文件模型的管線；可用於 Tauri command 簽章等非文件型別 |
| 各語言手寫，以契約測試把關 | 每種語言都能寫出最慣用的型別 | 三倍的維護成本；漂移只能在測試涵蓋到的地方被發現 | 不採用 |

## 後果

### 正面

- Schema 變更是單點修改；下游不相容會在編譯或 CI 階段被發現，而不是在執行期。
- 餵給 LLM 的 schema 與系統實際驗證的 schema 同源，降低「模型輸出合乎提示但被系統拒絕」的落差。
- 生成物進版控後，schema 變更在 code review 中以明確的差異呈現。

### 負面

- 建置流程需要 Rust、Python 與 Node 三套工具才能完整重新生成。
- 生成的 Pydantic／TypeScript 型別未必是最慣用的寫法；需要時應在生成型別之外加上薄包裝，而不是修改生成檔。
- Rust 的型別設計需顧及「可被 JSON Schema 與下游生成器良好表達」，某些 serde 表示法（如未加標籤的 enum、`flatten`）應避免。

### 風險與緩解

| 風險 | 影響 | 緩解措施 |
|---|---|---|
| 跨語言 schema 漂移（生成物過期或被手動修改） | 三端解讀不一致 | CI 重新生成並比對；生成檔標頭警示；禁止手寫重複型別 |
| 生成器對特定 schema 結構的支援差異 | 下游型別錯誤或過於寬鬆（例如退化為 `Any`／`unknown`） | 限定 serde 表示法：tagged union 一律使用內部標籤（延續現行 `kind` 判別欄位）；以黃金範例做三端往返測試（Rust 序列化 → Python／TS 解析 → 再序列化後相等） |
| 數值序列化差異 | 浮點數、大整數在三種語言間不一致 | 文件中的數值限定為 JSON 可精確表示的範圍；往返測試涵蓋邊界值 |
| LLM 相容 schema 與正式 schema 落差過大 | 模型產生通過 structured output 但被驗證器拒絕的結果 | 轉換步驟自動化並測試；驗證失敗的診斷回饋給 agent 重試（見 [0009](0009-agent-orchestration.md)） |
| 跨欄位規則被下游重複實作 | 規則不一致 | 明訂只有 Rust 驗證器擁有語意規則；code review 把關 |
| 生成工具版本變動改變輸出 | 無關變更造成大量差異 | 鎖定生成工具版本；升級為獨立變更 |

## 驗收或回顧條件

- **驗收**：
  - 單一指令可從 Rust 型別重新生成 JSON Schema、Pydantic 模型與 TypeScript 型別。
  - CI 在生成物與來源不一致時失敗。
  - Repo 中除 Rust core 外，不存在手寫的文件模型型別（第 2 階段完成後，`app/models/cad.py` 僅保留 1.0 相容用途或被生成檔取代）。
  - 三端往返測試通過所有範例文件。
  - LLM planner 使用的 schema 由管線產生，而非另行維護。
- **回顧**：
  - 生成器的限制迫使 Rust 型別設計出現明顯不自然的妥協時。
  - 第 4 階段移除 Python 後，簡化管線（移除 Pydantic 生成）。
  - 需要二進位或串流的文件格式時。

## 相關文件

- [0003](0003-frontend-react-typescript.md)、[0005](0005-rust-core-document-model.md)、[0006](0006-geometry-kernel-occt-python-sidecar.md)、[0009](0009-agent-orchestration.md)
- 現行來源：`app/models/cad.py`、[`../CAD_DSL.md`](../CAD_DSL.md)
- serde、schemars、datamodel-code-generator、json-schema-to-typescript

## 回顧紀錄

### 2026-11-26：第 2 階段 2a — Rust → JSON Schema 與 CI 漂移檢查

部分實作，單一事實來源尚未切換：

- **已完成**：CadDocument 2.0 的 Rust 型別（serde + schemars，`deny_unknown_fields`）可匯出 JSON Schema。`promptcad-core schema --out-dir <dir> [--check]` 輸出 `schemas/cad2/` 的五份檔案（`cad-document-1`、`cad-document-2`、`patch`、`resolved`、`validation-report` 的 `*.schema.json`），已提交進版控；`--check` 在漂移時失敗，`ci.yml` 的 `core` job 於 `cargo test` 之後執行（測試見 `crates/promptcad-core/tests/schema_export.rs`、`cli_schema.rs`）。`core` job 尚未在 GitHub 上執行過。
- **尚未完成**：由 `schemas/cad2/` 生成 Pydantic 與 TypeScript 型別、單一指令重新生成三者、三端往返測試，以及 LLM planner 改用管線產生的 schema，均未實作。
- **執行中的 1.0 應用程式不變**：來源仍是 `app/models/cad.py`，其 JSON Schema 由 `scripts/export_schema.py` 匯出為 `schemas/promptcad.schema.json`，前端型別由它生成（第 1 階段的過渡安排）。`app/models/cad.py` 尚未被取代，repo 中仍有手寫的 1.0 文件模型型別。
- 本 ADR 的驗收項目因此皆尚未全部達成；2.0 的 Rust → JSON Schema 與漂移檢查是其中第一步。

### 2026-11-26：第 2 階段 2b — core 請求／回應的 TypeScript 型別生成

- **已完成**：`promptcad_core::api` 的請求與回應 JSON Schema（`schemas/cad2/core-request.schema.json`、`schemas/cad2/core-response.schema.json`，由 Rust 型別生成）已生成前端 TypeScript 型別（`web/scripts/gen-types.mjs` 輸出 `web/src/api/generated/core-request.d.ts`、`core-response.d.ts`），CI 的 `web` job 檢查漂移。
- **尚未完成**：2.0 的 Pydantic 生成模型尚不存在。伺服器的 `POST /api/v1/core` 只轉交位元組（規則 4 與 [0012](0012-server-host-for-rust-core.md)），本端點目前不需要 Pydantic 模型。
- **1.0 應用程式的 schema 來源不變**：仍為 `app/models/cad.py`；`schemas/promptcad.schema.json` 與其 TypeScript 型別仍由 Pydantic 匯出生成。
- **規則 4 的遵守**：`app/services/core_bridge.py` 不解讀、不驗證、不重新序列化 2.0 內容。

