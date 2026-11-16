# 0002：桌面外殼採用 Tauri v2，並保留伺服器／Docker 模式

- **狀態**：Accepted
- **日期**：2026-11-17
- **決策者**：專案負責人
- **影響範圍**：新增 `src-tauri/`（桌面外殼）、封裝與發佈流程、`app/` 的啟動方式、[`../SECURITY.md`](../SECURITY.md)

## 背景

v0.1.0 只能以伺服器形式使用：`uvicorn app.main:app`，或透過 `Dockerfile`（micromamba 映像，內含 CadQuery 2.8.0）啟動後以瀏覽器存取。CAD 使用者普遍期待桌面應用程式：離線可用、直接讀寫本機檔案、不需要自行安裝 Conda 或 Docker。

同時，專案已有可用的 REST API、CLI 與 Docker 部署，這些使用情境（團隊共用伺服器、CI 批次輸出）不能因為加入桌面版而消失。

額外約束：

- 幾何核心在可預見的未來仍是 Python + OCCT（見 [0006](0006-geometry-kernel-occt-python-sidecar.md)），桌面外殼必須能管理外部程序。
- 核心文件模型將移到 Rust（見 [0005](0005-rust-core-document-model.md)），外殼若本身就是 Rust 可直接內嵌。
- 系統會接收 LLM 的輸出並操作本機檔案，需要最小權限的安全模型。

## 決策

我們採用 **Tauri v2**（撰寫時最新穩定版）作為桌面外殼，並**保留伺服器／Docker 模式**；兩種模式共用同一套核心與同一套前端。

具體做法：

1. **Sidecar**：Python 後端以 Tauri 的 sidecar 機制（`bundle.externalBin` 加上 shell plugin）隨應用程式發佈並由外殼負責啟動與結束。Sidecar 主程式需為依目標平台三元組命名的可執行檔；Python 執行環境與 OCCT 動態函式庫等其餘檔案以 `bundle.resources` 隨附。
2. **權限**：使用 Tauri v2 的 capability／permission 模型，逐視窗宣告可用的 command 與 plugin 權限。Shell 權限只允許啟動指定的 sidecar 與固定參數樣式，不開放任意命令；檔案系統權限限定在使用者明確選擇的專案目錄與應用程式資料目錄。
3. **Sidecar 通訊（第 1 階段）**：sidecar 只監聽 `127.0.0.1` 的動態埠，外殼於啟動時產生一次性隨機 token，透過環境變數交給 sidecar，沿用現有 `PROMPTCAD_API_TOKEN` 機制驗證每個請求。
4. **兩種模式共用核心**：
   - 桌面模式：WebView 載入打包後的前端 → Tauri command（Rust core）→ sidecar。
   - 伺服器模式：瀏覽器載入相同前端 → HTTP API → 相同的核心邏輯與相同的 Python 幾何服務。
   - 前端透過單一 API 用戶端抽象層切換傳輸方式（Tauri `invoke`／HTTP），UI 程式碼不得直接依賴 Tauri API。
5. Rust core 必須是不依賴 Tauri 的獨立 crate，Tauri 專案只是它的一個宿主（見 [0005](0005-rust-core-document-model.md)）。

## 考慮過的替代方案

| 方案 | 優點 | 缺點 | 結論 |
|---|---|---|---|
| **Tauri v2** | 使用系統 WebView，安裝包小；外殼即 Rust，可直接內嵌 Rust core；內建 sidecar 與 capability／permission 模型；可沿用 Web 前端 | 各平台 WebView 行為不一（WebView2／WKWebView／WebKitGTK）；需要 Rust 工具鏈；sidecar 封裝需自行處理 | 採用 |
| Electron | 生態最成熟；內建 Chromium，各平台渲染一致；Node 生態豐富 | 每個應用程式自帶 Chromium 與 Node，體積與記憶體佔用大；Rust core 需經 N-API／額外程序橋接；安全模型需自行收緊 | 不採用：體積與 Rust 整合成本 |
| egui／iced（Rust 原生 GUI） | 單一語言、無 WebView；啟動快 | 無法與 Web／伺服器模式共用 UI；複雜表單、JSON 編輯器、文字排版（含中文輸入法）與 3D 檢視器都要自建；生態仍在快速變動 | 不採用：等於維護兩套 UI |
| Qt（C++ 或 PySide） | 成熟的桌面框架；與 OCCT 有長期整合先例（如 FreeCAD） | 授權（LGPL／商業）需審慎處理；UI 無法與 Web 共用；C++ 或 Python 綁定增加建置複雜度 | 不採用 |
| Slint | 輕量、宣告式 UI、Rust 一等支援 | 元件生態與 3D 整合有限；授權條款需評估；同樣無法共用 Web UI | 不採用 |
| 只做 Web（PWA）+ 本機安裝服務 | 不需桌面外殼 | 使用者仍要自行安裝並啟動 Python／OCCT 環境；本機檔案存取受瀏覽器限制 | 不採用：無法解決安裝門檻 |

## 後果

### 正面

- 桌面外殼本身的體積小；主要體積來自 Python + OCCT sidecar，而該部分未來可被替換（見 [0010](0010-migration-phases.md) 第 4 階段）。
- Rust core 以函式呼叫內嵌於外殼，文件編輯、驗證、patch 套用不經網路層。
- capability／permission 模型讓「LLM 輸出 → 檔案／程序操作」的攻擊面可被明確列舉與審查。
- 伺服器／Docker 使用者不受影響；現有 REST API、CLI、CI 流程持續可用。

### 負面

- 需要維護兩種傳輸路徑（Tauri IPC 與 HTTP）並保證行為一致。
- 建置與 CI 需涵蓋 Rust、Node 與 Python 三套工具鏈，以及各作業系統的簽章與公證流程。
- 前端必須在三種 WebView 引擎上測試，不能只測 Chromium。

### 風險與緩解

| 風險 | 影響 | 緩解措施 |
|---|---|---|
| Sidecar 體積（Python + OCCT 約 300–500 MB） | 下載與安裝體驗差，抵銷 Tauri 的體積優勢 | 評估 conda-pack 與 PyInstaller 兩種封裝；移除未使用的套件與測試資料；考慮首次啟動時另行下載幾何執行環境；長期以第 4 階段移除 Python |
| Windows 上的 WebView2 | 舊系統可能未安裝；企業環境可能鎖版本 | Windows 11 已內建；安裝程式採用 WebView2 bootstrapper 或離線安裝器；WebView2 為 Chromium 核心，WebGL 可用，於啟動時偵測並提示 |
| Linux WebKitGTK 的 WebGL／效能差異 | 3D 檢視器效能或相容性問題 | 列入 [0004](0004-3d-viewer-threejs.md) 的驗收測試矩陣；必要時提供降級渲染設定 |
| 大型網格經 IPC 傳輸的負擔 | 以 JSON 序列化大型陣列造成延遲與記憶體倍增 | 網格不走 JSON `invoke`；由 sidecar 寫出 glTF（GLB）檔，前端透過資產協定或本機 HTTP 以二進位方式載入（見 [0004](0004-3d-viewer-threejs.md)） |
| Sidecar 殘留或啟動失敗 | 殭屍程序、埠被占用 | 外殼負責生命週期：健康檢查、逾時、應用程式結束時終止子程序；使用動態埠 |
| 本機 HTTP 被其他程序存取 | 未授權呼叫幾何服務 | 僅綁定 loopback、每次啟動更換 token；之後可改用 stdio 或具名管道 |

## 驗收或回顧條件

- **驗收**：
  - 在未安裝 Python／Conda／Docker 的乾淨 Windows 11 機器上，安裝後可完成「提示詞 → 驗證 → STEP／STL 輸出」。
  - 同一份前端建置產物可同時用於桌面版與 `docker compose up` 的伺服器模式。
  - capability 檔中不存在萬用的 shell 或檔案系統權限。
  - 關閉應用程式後無殘留 sidecar 程序。
- **回顧**：
  - 安裝包（含 sidecar）在採取緩解措施後仍明顯超出可接受範圍（目標值於第 1 階段實測後訂定）。
  - 任一目標平台的 WebView 出現無法繞過的 WebGL 或 IPC 問題。
  - 第 4 階段啟動時，重新檢視是否仍需要 sidecar 機制。

## 相關文件

- [0003](0003-frontend-react-typescript.md)、[0005](0005-rust-core-document-model.md)、[0006](0006-geometry-kernel-occt-python-sidecar.md)、[0010](0010-migration-phases.md)
- [`overview.md`](overview.md)
- 現行啟動方式：`Dockerfile`、`docker-compose.yml`、`app/main.py`
- 現行 renderer 的程序隔離：`app/services/renderer.py`
- Tauri v2 官方文件：sidecar（Embedding External Binaries）、Capabilities、Permissions

## 回顧紀錄

（尚無）
