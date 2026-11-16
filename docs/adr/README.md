# 架構決策紀錄（ADR）

本資料夾保存 PromptCAD Studio 的架構決策紀錄（Architecture Decision Record）。每一份 ADR 記錄一個具有長期影響的技術決策：當時的背景、選了什麼、放棄了什麼，以及因此承擔的後果。

- 目標架構總覽與圖：[`overview.md`](overview.md)
- 現行（v0.1.0）架構：[`../ARCHITECTURE.md`](../ARCHITECTURE.md)
- 維度階梯概念與 CadDocument 2.0：[`../CONCEPT_DIMENSIONAL_LADDER.md`](../CONCEPT_DIMENSIONAL_LADDER.md)

## 索引

| 編號 | 標題 | 狀態 | 日期 |
|---|---|---|---|
| [0001](0001-record-architecture-decisions.md) | 以 ADR 記錄架構決策 | Accepted | 2026-11-17 |
| [0002](0002-desktop-shell-tauri-v2.md) | 桌面外殼採用 Tauri v2，並保留伺服器／Docker 模式 | Accepted | 2026-11-17 |
| [0003](0003-frontend-react-typescript.md) | 前端採用 React + TypeScript + Vite | Accepted | 2026-11-17 |
| [0004](0004-3d-viewer-threejs.md) | 3D 檢視器採用 Three.js | Accepted | 2026-11-17 |
| [0005](0005-rust-core-document-model.md) | 文件模型、驗證器與 patch／操作紀錄引擎以 Rust 實作 | Accepted | 2026-11-17 |
| [0006](0006-geometry-kernel-occt-python-sidecar.md) | 幾何核心維持 OpenCascade（CadQuery），以 Python sidecar 執行 | Accepted | 2026-11-17 |
| [0007](0007-constraint-solver.md) | 約束求解採用既有求解器（planegcs 或 libslvs），不自行開發 | Accepted | 2026-11-17 |
| [0008](0008-single-schema-source-of-truth.md) | 以 Rust serde 型別作為 schema 單一事實來源 | Accepted | 2026-11-17 |
| [0009](0009-agent-orchestration.md) | Agent 編排：Claude API tool use + structured outputs，僅輸出 patch JSON | Accepted | 2026-11-17 |
| [0010](0010-migration-phases.md) | 四階段遷移路線 | Accepted | 2026-11-17 |

## 決策之間的關係

```text
0002 Tauri v2 ──┬─→ 0003 React/TS/Vite ──→ 0004 Three.js
                └─→ 0005 Rust core ──┬─→ 0008 Schema 單一來源
                                     ├─→ 0007 約束求解器
                                     └─→ 0009 Agent 編排
0006 OCCT／Python sidecar ←─ 0002（sidecar 機制）、0008（生成 Pydantic）
0010 遷移階段：排定以上全部的落地順序
```

## 流程

### 何時需要寫 ADR

符合下列任一條件即應撰寫：

- 引入、替換或移除語言、框架、執行環境、幾何核心或對外協定。
- 改變模組邊界、程序邊界（例如 sidecar）、資料模型或 schema 版本策略。
- 改變安全模型（沙箱、權限、金鑰存放、LLM 可輸出的內容範圍）。
- 決策難以回復，或會約束後續多個決策。

單純的實作細節、可輕易回復的函式庫小版本升級，不需要 ADR。

### 步驟

1. 複製 [`0000-template.md`](0000-template.md)，以下一個可用編號命名。
2. 以 `Proposed` 狀態送審；討論內容應反映在「考慮過的替代方案」與「後果」中。
3. 取得同意後改為 `Accepted` 並填入日期，同時更新本檔索引表。
4. 已 `Accepted` 的 ADR **不改寫決策內容**。只允許：修正錯字、補充連結、更新狀態欄與「回顧紀錄」。
5. 要推翻或大幅修改既有決策時，新增一份 ADR，並把舊 ADR 的狀態改為 `Superseded by NNNN`。

### 編號與檔名

- 四位數、零填補、依序遞增，不重複使用、不跳號回填：`NNNN-kebab-case-title.md`。
- `0000` 保留給範本。
- 檔名使用英文 kebab-case；內文使用繁體中文，識別字與技術名稱保留英文。

### 狀態

| 狀態 | 意義 |
|---|---|
| `Proposed` | 已提出，尚在討論；不得作為實作依據。 |
| `Accepted` | 已同意，為現行有效決策。 |
| `Superseded` | 已被較新的 ADR 取代；須註明 `Superseded by NNNN`。 |
| `Deprecated` | 不再適用，但沒有直接的取代決策（例如相關功能已移除）。 |

### 內容要求

每份 ADR 至少包含：狀態、日期、背景、決策、考慮過的替代方案（含優缺點表）、後果（正面／負面／風險）、驗收或回顧條件、相關文件。

撰寫時請遵守：

- 一份 ADR 只記錄一個決策。
- 不確定的版本號不要寫死，改寫「撰寫時最新穩定版」，實際版本以 lockfile 為準。
- 「回顧條件」必須是可觀察的事件或量測值，而不是「之後再看」。
