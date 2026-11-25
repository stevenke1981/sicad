# 0003：前端採用 React + TypeScript + Vite

- **狀態**：Accepted
- **日期**：2026-11-17
- **決策者**：專案負責人
- **影響範圍**：新增前端專案（暫定 `ui/`）、`app/static/`（逐步淘汰）、CI

## 背景

現行 UI 位於 `app/static/`，是沒有建置步驟的靜態 HTML/JS：一個提示詞輸入框、可編輯的 `spec.json` 文字區、驗證報告與 SVG 預覽。README 中的品質檢查只有 `node --check app/static/app.js`，沒有型別檢查與元件測試。

目標架構需要的 UI 能力已超出此做法能負擔的範圍：

- 元件樹、port 與約束的結構化編輯（見 [`../CONCEPT_DIMENSIONAL_LADDER.md`](../CONCEPT_DIMENSIONAL_LADDER.md)）。
- 3D 檢視器及其與文件狀態的雙向選取（見 [0004](0004-3d-viewer-threejs.md)）。
- Agent 操作紀錄的檢視、接受與復原（見 [0005](0005-rust-core-document-model.md)、[0009](0009-agent-orchestration.md)）。
- 同一套 UI 同時運行於 Tauri WebView 與一般瀏覽器（見 [0002](0002-desktop-shell-tauri-v2.md)）。
- 型別需由 schema 自動生成（見 [0008](0008-single-schema-source-of-truth.md)）。

## 決策

我們採用 **React + TypeScript + Vite**（皆為撰寫時最新穩定版）建立新前端，並**漸進取代** `app/static/`。

- TypeScript 開啟 `strict`。文件模型型別一律使用由 JSON Schema 生成的檔案，不手寫重複定義。
- 所有後端互動經由單一 API 用戶端介面，提供 Tauri `invoke` 與 HTTP `fetch` 兩種實作；元件不得直接匯入 `@tauri-apps/api`。
- 文件狀態的唯一權威在 Rust core（桌面）或伺服器端；前端保存的是投影與暫時性 UI 狀態，修改一律以 patch 送出，不在前端自行變更文件後再同步。
- 漸進遷移：
  1. 新前端先達到與 `app/static/` 功能等價（提示詞、JSON 編輯、驗證報告、下載）。
  2. FastAPI 改為提供 Vite 建置產物；`app/static/` 保留為備援直到等價驗收通過。
  3. 驗收後移除 `app/static/` 與 `pyproject.toml` 中對應的 `package-data`。
- 狀態管理、UI 元件庫、路由等次層級選擇不在本 ADR 範圍，原則是先用 React 內建機制，有具體需求再引入。

## 考慮過的替代方案

| 方案 | 優點 | 缺點 | 結論 |
|---|---|---|---|
| **React + TypeScript + Vite** | 生態最大、人才與範例最多；Three.js 整合方案成熟；LLM／agent 產出程式碼的品質穩定；Vite 為 Tauri 官方範本支援的建置工具 | 需自行約束狀態管理與專案結構；執行期體積不是最小 | 採用 |
| 維持無建置步驟的原生 HTML/JS | 零工具鏈、部署最簡單 | 無型別、無元件化；無法承載元件圖編輯與 3D 互動的複雜度；無法使用生成的型別 | 不採用 |
| Svelte／SvelteKit | 產出小、語法精簡 | CAD／3D 相關元件與範例較少；團隊與 agent 的熟悉度較低 | 不採用 |
| Vue | 學習曲線平緩、官方工具完整 | 與 React 相比無決定性優勢；3D 檢視器相關生態較小 | 不採用 |
| SolidJS | 細粒度反應式、效能佳 | 生態小，第三方元件選擇有限 | 不採用 |
| Rust 前端（Leptos／Yew／Dioxus，編譯為 WASM） | 與 Rust core 共用型別，免除 TS 生成 | 生態與除錯工具不成熟；Three.js 需透過 JS 互通；開發迭代較慢 | 不採用 |
| Next.js 等全端框架 | 內建路由與 SSR | 桌面應用不需要 SSR；伺服器端執行模型與 Tauri 靜態載入不合 | 不採用 |

## 後果

### 正面

- 型別檢查涵蓋文件模型，schema 變更會在編譯期暴露前端的不相容。
- 同一份建置產物供桌面與 Web 使用，避免兩套 UI。
- 元件化後可為編輯器、驗證報告、操作紀錄建立獨立測試。

### 負面

- 專案新增 Node 工具鏈與建置步驟；README 中「無前端建置步驟」的特性不再成立，Docker 映像需要多階段建置。
- 遷移期間新舊 UI 並存，需避免功能只加在其中一邊。

### 風險與緩解

| 風險 | 影響 | 緩解措施 |
|---|---|---|
| 前端依賴樹膨脹 | 供應鏈風險、建置變慢 | 新增相依套件需說明理由；lockfile 進版控；CI 執行相依套件稽核 |
| 元件直接呼叫 Tauri API | Web 模式失效 | 以 lint 規則禁止在 API 用戶端以外的位置匯入 Tauri 套件；CI 同時建置並測試 Web 模式 |
| WebView 引擎差異 | 桌面版出現瀏覽器中看不到的錯誤 | 端對端測試涵蓋 WebView2、WKWebView、WebKitGTK |
| 新舊 UI 長期並存 | 維護成本倍增 | 在 [0010](0010-migration-phases.md) 第 1 階段的完成條件中明訂移除 `app/static/` |

## 驗收或回顧條件

- **驗收**：
  - 新前端通過 `tsc --noEmit` 且無手寫的文件模型型別。
  - README「Web 編輯流程」的五個步驟可在新前端完整執行，桌面與 Web 模式皆然。
  - `app/static/` 已移除。
- **回顧**：
  - 3D 檢視器或大型元件樹出現 React 渲染模型造成、且無法以常規最佳化解決的效能問題。
  - Tauri 或 Vite 的官方支援方向發生重大改變。

## 相關文件

- [0002](0002-desktop-shell-tauri-v2.md)、[0004](0004-3d-viewer-threejs.md)、[0008](0008-single-schema-source-of-truth.md)、[0010](0010-migration-phases.md)
- 現行 UI：`web/`（原 `app/static/` 已於 2026-11-26 移除，見回顧紀錄）
- 現行 API：[`../API.md`](../API.md)

## 回顧紀錄

### 2026-11-26：移除 `app/static/`（遷移步驟 3）

- **等價驗收**：`web/` 的新前端已達到與 `app/static/` 功能等價（提示詞與 planner 選擇、生成、JSON 編輯與驗證後重新輸出、驗證報告、產物與工作 ZIP 下載、工作歷史、Token 欄位、SVG 預覽），另加 Three.js STL 檢視器，桌面與 Web 模式皆可完成 README「Web 編輯流程」。
- **移除**：負責人於 2026-11-26 核准移除 `app/static/`。目錄與 `pyproject.toml` 中對應的 `package-data` 已刪除，驗收條件「`app/static/` 已移除」達成。
- **未建置前端時的行為**：`app/web_dist/index.html` 不存在時，FastAPI 的 `/` 回傳 HTTP 503 的內嵌說明頁（`app/main.py` 的字串常數），指示執行 `npm --prefix web ci` 與 `npm --prefix web run build`，並連到 `/docs`；`/assets` 不掛載，`/api/v1/*` 與 `/api/v1/health` 不受影響。伺服器模式因此必須先建置前端，或使用多階段 Dockerfile（在映像內建置 `web/`）。
- **預覽**：CadQuery 的 `model.svg` 改以紙色底顯示，`preview.svg` 不變（只改前端）。
