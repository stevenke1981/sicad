# 0001：以 ADR 記錄架構決策

- **狀態**：Accepted
- **日期**：2026-11-17
- **決策者**：專案負責人與架構 agent 團隊
- **影響範圍**：`docs/adr/` 及所有後續架構變更

## 背景

PromptCAD Studio v0.1.0 是單一語言（Python 3.11）的 MVP：FastAPI、Pydantic `CadDocument 1.0`、規則／LLM 規劃器、`DesignValidator`、CadQuery／OpenSCAD 編譯器、受限 subprocess renderer 與靜態 HTML/JS UI。現行設計說明集中在 [`../ARCHITECTURE.md`](../ARCHITECTURE.md)，描述「現在是什麼」，但沒有記錄「為什麼不是別的」。

接下來專案要演進為桌面與 Web 並行、跨 Rust／Python／TypeScript 三種語言，並引入元件圖、約束圖與多 agent 編排（見 [`../CONCEPT_DIMENSIONAL_LADDER.md`](../CONCEPT_DIMENSIONAL_LADDER.md)）。這類決策彼此牽動、難以回復，而且部分工作由 AI agent 執行；若沒有穩定的決策紀錄，人與 agent 都容易重新爭論已定案的問題，或在不知情下違反既有約束。

## 決策

我們採用 ADR 記錄所有具長期影響的架構決策：

- 存放於 `docs/adr/`，一個決策一個 Markdown 檔，檔名 `NNNN-kebab-case-title.md`。
- 內文使用繁體中文（台灣用語），識別字與技術名稱保留英文。
- 使用 [`0000-template.md`](0000-template.md) 的固定章節。
- 狀態限定為 `Proposed`、`Accepted`、`Superseded`、`Deprecated`。
- 已接受的 ADR 不改寫決策本文；要變更就新增 ADR 取代舊的。
- [`README.md`](README.md) 維護索引與流程；[`overview.md`](overview.md) 維護目標架構圖。

`docs/ARCHITECTURE.md` 繼續描述「目前實際的架構」，ADR 描述「決策與理由」。兩者不一致時，以已落地的程式碼與 `ARCHITECTURE.md` 為現況，以 ADR 為方向。

## 考慮過的替代方案

| 方案 | 優點 | 缺點 | 結論 |
|---|---|---|---|
| ADR（Markdown，放在 repo） | 與程式碼同版控、可審查差異；格式輕量；agent 可直接讀取 | 需要紀律維護索引與狀態 | 採用 |
| 只持續改寫 `ARCHITECTURE.md` | 只有一份文件 | 歷史與被否決的方案會被覆蓋；無法看出決策時序 | 不採用 |
| 外部 wiki／文件平台 | 編輯體驗好、可留言 | 與程式碼版本脫鉤；離線與 agent 取用不便 | 不採用 |
| 只靠 commit／PR 說明 | 零額外成本 | 分散、難以檢索；無法表達「被取代」 | 不採用 |
| RFC 流程（較重的提案文件） | 適合大型、多方審查 | 以目前團隊規模而言負擔過重 | 不採用；規模成長後可再評估 |

## 後果

### 正面

- 新成員與 agent 能在一處理解跨語言架構的來龍去脈。
- 被否決的方案與理由被保留，避免重複評估。
- 決策可被明確取代，而非悄悄漂移。

### 負面

- 每個重大變更多一份文件的撰寫與審查成本。
- 索引表與狀態需要人工同步。

### 風險與緩解

| 風險 | 影響 | 緩解措施 |
|---|---|---|
| ADR 與實作脫節 | 文件失去可信度 | 每份 ADR 訂定可觀察的驗收與回顧條件；階段結束時逐份檢查（見 [0010](0010-migration-phases.md)） |
| 範圍過細、數量膨脹 | 重要決策被淹沒 | 依 `README.md` 的「何時需要寫 ADR」篩選 |

## 驗收或回顧條件

- **驗收**：`docs/adr/README.md` 索引與資料夾內檔案一一對應；0002–0010 皆依範本撰寫。
- **回顧**：當 ADR 數量超過約 30 份，或出現需要多方正式審查的提案時，重新評估是否需要分類目錄或 RFC 流程。

## 相關文件

- [`README.md`](README.md)、[`0000-template.md`](0000-template.md)、[`overview.md`](overview.md)
- [`../ARCHITECTURE.md`](../ARCHITECTURE.md)
- Michael Nygard，〈Documenting Architecture Decisions〉（ADR 格式的原始出處）

## 回顧紀錄

（尚無）
