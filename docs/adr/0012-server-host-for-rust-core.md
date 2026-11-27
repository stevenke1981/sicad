# 0012：伺服器模式下 Rust core 的宿主形式

- **狀態**：Proposed（已依此實作，待負責人簽核）
- **日期**：2026-11-26
- **決策者**：專案負責人
- **影響範圍**：`crates/promptcad-core/src/api.rs`、`crates/promptcad-core/src/bin/promptcad-core.rs`（`rpc` 子命令）、`src-tauri/src/core.rs`、`src-tauri/capabilities/default.json`、`app/api/routes.py`（`POST /api/v1/core`）、`app/services/core_bridge.py`、`app/core/config.py`、`web/src/api/`、`Dockerfile`、`.github/workflows/ci.yml`、[`../API.md`](../API.md)

## 背景

[0005](0005-rust-core-document-model.md) 把 Rust core 定義為「不依賴 Tauri、不做 I/O 以外副作用的函式庫 crate」，並規定桌面模式由 Tauri command 呼叫、伺服器模式由一個薄宿主暴露相同操作；伺服器宿主的具體形式（獨立 HTTP 服務，或以 PyO3 供 FastAPI 載入）留待第 2 階段開始時決定，並以新 ADR 記錄。[0010](0010-migration-phases.md) 第 2 階段的完成條件之一是「決定伺服器模式下 Rust core 的宿主形式（新增 ADR）」，且 REST API 與 CLI 在伺服器模式下仍須可用。

到第 2 階段 2a 為止，Rust core 只能經 `cargo test` 與 `promptcad-core` 命令列工具使用，Tauri 與 FastAPI 都沒有呼叫它。本 ADR 處理的約束如下：

- **桌面版**：sidecar 是 PyInstaller onedir 凍結的 Python 程式（[0006](0006-geometry-kernel-occt-python-sidecar.md)）；Tauri 外殼是 Rust 程式，與 core 同一程序內即可直接呼叫。
- **伺服器與 Docker 模式**：FastAPI 是 Python 程序，需要透過某種邊界取得 core 的結果。Docker 映像的執行階段基底為 `mambaorg/micromamba:2.10.0`（含 CadQuery 的 Conda 環境，見 `Dockerfile`），伺服器模式的部署形式見 [0002](0002-desktop-shell-tauri-v2.md)。
- **相依管理**：Python 依賴以 hash 鎖檔安裝（`requirements/`，見 [0011](0011-cross-platform-desktop.md)）；桌面版另有 PyInstaller 打包與三平台鎖檔。任何需要在 Python 端編譯的原生擴充都必須進入這些流程。
- **既有模式**：渲染已經採用「子行程 + 結構化輸出」的隔離方式（桌面版以 `promptcad-sidecar --render-worker` 執行 `model.py`，見 [0006](0006-geometry-kernel-occt-python-sidecar.md)）。
- **錯誤隔離**：2.0 的驗證與求解是新的 Rust 程式碼，尚無長時間的使用經驗。核心若 panic、abort 或陷入無限迴圈，不應拖垮 API 伺服器。
- **未來需求**：第 3 階段的多 agent 編排與 op log 可能需要伺服器端的 session 狀態（[0009](0009-agent-orchestration.md)），這會影響宿主是否需要常駐。
- **契約一致性**：[0008](0008-single-schema-source-of-truth.md) 規定 Python 與 TypeScript 端不得各自重新實作跨欄位規則；伺服器只能轉交，不能解讀。

## 決策

我們採用 **單一請求／回應契約、三個宿主** 的形式：

1. **契約**：`promptcad_core::api` 定義 `CoreRequest`（以 `op` 區分：`validate`、`solve`、`normalize`、`upgrade`、`downgrade`、`apply_patch`、`describe`、`version`）與 `CoreResponse`（以 `status` 區分：`ok`、`error`），並提供 `handle` 與 `handle_json`（請求上限 8 MiB）。請求與回應的 JSON Schema 由 Rust 型別生成，位於 `schemas/cad2/core-request.schema.json` 與 `core-response.schema.json`。
2. **桌面宿主（Tauri，行程內）**：Tauri 命令 `core_request`（`src-tauri/src/core.rs`）在阻塞執行緒中直接呼叫 core；權限為 `allow-core-request`。
3. **伺服器宿主（FastAPI，每次呼叫一個子行程）**：`POST /api/v1/core` 以 `app/services/core_bridge.py` 啟動 `promptcad-core rpc`。`rpc` 子命令從 stdin 讀入一個 JSON 請求，向 stdout 寫出一行 JSON 回應，成功寫出回應時結束碼為 0。伺服器**原封不動**轉交位元組，不解讀、不驗證、不重新序列化。HTTP 端點本身只是經過驗證的透明傳輸層。
4. **命令列宿主**：`promptcad-core` 既有的各子命令，與 `rpc` 共用同一個 crate。

同一個請求在 Tauri 與 `rpc` 兩條路徑得到逐位元組相同的回應（已以測試驗證）。

本決策的範圍與限制：

- 不採用常駐的 core 服務；每次 HTTP 呼叫都啟動新行程。是否需要常駐，留待 [回顧條件](#驗收或回顧條件) 所列的事件或量測值出現後再決定。
- 子行程的環境只保留 `PATH`、`SYSTEMROOT`、`WINDIR`、`TEMP`、`TMP`、`TMPDIR`、`HOME`、`LANG`、`LC_ALL`，因此不繼承 API Token 或 LLM 金鑰。
- 逾時（預設 15 秒，`PROMPTCAD_CORE_TIMEOUT_SECONDS`）後強制結束子行程；core 自身拒絕輸出超過 24 MiB 的回應（改回 `response_too_large`），橋接層另以 28 MiB 為第二道上限。
- 找不到執行檔時回報 `503`，且 `/api/v1/capabilities` 的 `core_available` 為 `false`。

## 考慮過的替代方案

| 方案 | 優點 | 缺點 | 結論 |
|---|---|---|---|
| **(a) 每次呼叫啟動一次 `promptcad-core rpc` 子行程（stdin／stdout JSON）** | 不需在 Python 端編譯或安裝原生擴充，三平台與 PyInstaller 打包流程不變；行程隔離：core 崩潰或卡住不會拖垮伺服器，逾時後可直接結束行程；與既有的渲染子行程模式一致；桌面與伺服器回應逐位元組相同 | 每次呼叫需付出一次行程啟動的成本（**尚未量測**）；Docker 映像需額外的 Rust 建置階段，並把執行檔放入映像；請求上限 8 MiB；不支援串流 | **採用** |
| (b) PyO3 擴充模組，由 FastAPI 直接載入 | 呼叫最快，沒有行程啟動成本；介面可以是 Python 物件 | 每個平台、每個 Python 版本都要建置 maturin wheel，並加入 hash 鎖檔與 PyInstaller 打包；core 的 panic 或 abort 會終止伺服器程序；Python 與 Rust 的發佈節奏被綁在一起；Python 必須在邊界上重新解讀資料，容易違反 ADR 0008 規則 4 | 不採用：增加三平台原生建置與隔離風險，且與第 4 階段「移除 Python」方向相反 |
| (c) 獨立的長駐 Rust HTTP 服務 | 沒有每次呼叫的行程啟動成本；未來放置 session 狀態、op log 與快取的自然位置 | 多一個要部署、認證、健康檢查、管理埠號的服務；目前沒有需要伺服器端狀態的功能，屬於過早的複雜度；需要處理服務生命週期與重啟 | 不採用（現階段）：待第 3 階段出現伺服器端 session 需求後重新評估 |
| (d) 把 core 編譯為 WASM，在瀏覽器執行 | 不需要伺服器往返，編輯回饋最快 | 不服務 REST 與 CLI 用戶端；伺服器仍必須自行驗證，等於多維護一條路徑；WASM 的建置與除錯成本 | 不採用：可作為日後的額外選項，但不取代伺服器宿主 |

## 後果

### 正面

- 三個宿主共用同一份契約與同一份實作，回應逐位元組一致，減少跨宿主的行為漂移。
- Python 端沒有新的原生相依，`requirements/`、PyInstaller 打包與三平台鎖檔的流程都不需要改變。
- core 的錯誤、崩潰與逾時被限制在子行程內；API 伺服器可以回傳 `502`／`504` 並繼續服務其他請求。
- 伺服器完全不解讀 2.0 內容，符合 0008 規則 4，Python 不會出現第二份驗證邏輯。

### 負面

- 每次呼叫都有行程啟動成本。目前**尚未量測**，因此不知道它在實際負載下是否可接受。
- Docker 映像需要新增 Rust 建置階段（`rust:1.90.0-slim-trixie`），建置時間與映像大小都會增加。
- 單次請求上限 8 MiB，沒有串流；大型文件必須一次送出。
- 伺服器模式下 `POST /api/v1/core` 只是轉交，不提供 2.0 的文件儲存、patch 歷史或 op log 持久化。

### 風險與緩解

| 風險 | 影響 | 緩解措施 |
|---|---|---|
| 部署時缺少 `promptcad-core` 執行檔 | 端點無法使用 | 回報 `503`；`/api/v1/capabilities` 的 `core_available` 為 `false`；Docker 映像內建執行檔，CI 的 `docker` job 呼叫端點確認 |
| Python 伺服器與 core 執行檔版本不一致 | 契約欄位對不上 | `version` 操作回傳 crate 版本與 `api_version`；capabilities 同時揭露 `core_crate_version`、`core_api_version` 與 `core_schema_versions` |
| 高併發下行程啟動成本造成延遲 | 端點回應變慢 | 以量測結果為準；若超過可接受範圍，依 [回顧條件](#驗收或回顧條件) 重新評估方案 (c) |
| Docker 建置階段未在本機驗證 | 映像建置可能失敗或執行檔路徑錯誤 | 開發機沒有 Docker；以 CI 的 `docker` job 在 PR 執行時驗證，結果出來前視為未驗證 |
| 子行程殘留或卡住 | 資源耗盡 | 逾時後 `kill` 子行程並等待；stdout、stderr 與 stdin 皆在 `finally` 中關閉 |
| 子行程繼承過多環境資訊 | 憑證外洩 | 只保留白名單內的環境變數（見上方「決策」）；Token 與 LLM 金鑰不會傳入 |

## 驗收或回顧條件

- **驗收**（以下皆已於本機或 CI 驗證，除非另註）：
  - `cargo test` 通過（`crates/promptcad-core`，317 個 Rust 測試）；`src-tauri` 的 `cargo test`（26 個）通過。
  - 同一請求經 Tauri 命令與 `promptcad-core rpc` 得到逐位元組相同的回應（測試涵蓋）。
  - `pytest` 381 項通過、3 項略過；伺服器整合測試以實際的 `promptcad-core` 執行，並以 49 個 parity 案例重播 Python `DesignValidator` 的結果。
  - 前端 vitest 68 項通過。
  - 瀏覽器中對內建的 2.0 範例執行「驗證 JSON」，元件樹正確顯示（本機伺服器的端到端檢查）。
  - **未驗證**：Tauri `invoke` 在真實 WebView 內的往返；完整的 `tauri build`；Docker 映像建置（待 CI `docker` job）。
- **回顧**：
  - 第 3 階段出現需要伺服器端 session 狀態的功能（例如多 agent 的 op log 持久化或 patch 佇列）時，重新評估方案 (c)。
  - 量測到的每次呼叫行程啟動時間，在典型請求下達到不可接受的水準（門檻由負責人於量測後訂定）時，重新評估方案 (b) 與 (c)。
  - 第 4 階段移除 Python 時，伺服器宿主是否仍需要以子行程形式存在。

## 相關文件

- [0002](0002-desktop-shell-tauri-v2.md)、[0005](0005-rust-core-document-model.md)、[0006](0006-geometry-kernel-occt-python-sidecar.md)、[0008](0008-single-schema-source-of-truth.md)、[0010](0010-migration-phases.md)
- [`../API.md`](../API.md)（`POST /api/v1/core` 的端點說明與狀態碼）
- [`../../crates/promptcad-core/README.md`](../../crates/promptcad-core/README.md)（契約與命令列用法）
- `app/services/core_bridge.py`、`app/api/routes.py`、`src-tauri/src/core.rs`

## 回顧紀錄

（尚無）
