# 0004：3D 檢視器採用 Three.js

- **狀態**：Accepted
- **日期**：2026-11-17
- **決策者**：專案負責人
- **影響範圍**：前端檢視器模組、Python sidecar 的網格輸出、job 產物清單

## 背景

v0.1.0 沒有互動式 3D 檢視：`preview.svg` 是不依賴 CAD 核心的快速示意，`model.svg` 是 CadQuery 的固定視角投影。使用者無法旋轉模型，也無法在畫面上指出「這個孔」或「這個面」。

引入元件圖與 port 之後（見 [`../CONCEPT_DIMENSIONAL_LADDER.md`](../CONCEPT_DIMENSIONAL_LADDER.md)），檢視器成為編輯流程的一部分：

- 顯示幾何核心輸出的網格（glTF／STL）。
- 以元件為單位選取，並與元件樹、屬性面板、agent 對話同步。
- 標示 port（位置、方向、類型）與已建立／未滿足的連接。
- 在 Tauri 的三種系統 WebView 與一般瀏覽器上運作（見 [0002](0002-desktop-shell-tauri-v2.md)）。

## 決策

我們採用 **Three.js**（撰寫時最新穩定版）實作 3D 檢視器。

- **主要顯示格式為 glTF 2.0 二進位（GLB）**，STL 作為相容與匯出格式。glTF 能保留節點階層、名稱與 `extras` 中繼資料，STL 只有無結構的三角形。
- **選取對應**：sidecar 輸出 glTF 時，每個元件對應一個具名節點，節點帶有元件 ID；檢視器以 raycasting 命中 mesh 後回溯到元件 ID。選取狀態屬於 UI 狀態，不寫入文件。
- **Port 顯示**：port 的幾何資訊來自文件模型（Rust core），而非從網格反推。檢視器將 port 以獨立的輔助圖層（標記、座標軸、連線）疊加繪製，可依類型與連接狀態上色。
- **網格傳輸**：GLB 以二進位方式載入（Tauri 資產協定或本機 HTTP），不經 JSON 序列化的 `invoke`。
- **評估 three-cad-viewer**：它是以 Three.js 為基礎、針對 CadQuery／OCP 輸出設計的檢視器，提供 CAD 常用的導覽、剖切、邊線顯示與物件樹。它使用自有的細分資料格式而非 glTF，因此需評估其資料路徑、選取事件 API 與授權是否符合上述需求。第 1 階段先以 Three.js 直接實作 GLB 載入與元件選取的最小檢視器，同時進行此評估；結論以本 ADR 的回顧紀錄或新 ADR 記載。
- 是否使用 React 的宣告式包裝（如 react-three-fiber）屬實作層級選擇，不在本 ADR 範圍；條件是檢視器核心必須能脫離 React 獨立測試。

## 考慮過的替代方案

| 方案 | 優點 | 缺點 | 結論 |
|---|---|---|---|
| **Three.js** | 生態最大、文件與範例最多；glTF／STL 載入器與 raycasting 齊備；細粒度控制材質、邊線與輔助圖層；套件可按需引入 | 屬低階函式庫，CAD 慣用功能（剖切、量測、視圖方塊）需自建或另找套件 | 採用 |
| Three.js + three-cad-viewer | 直接獲得 CAD 風格導覽、剖切、邊線與樹狀檢視；與 CadQuery／OCP 生態相容 | 使用自有細分格式；客製 port 圖層與選取行為的彈性待確認；單一維護者風險 | 評估中，見決策 |
| Babylon.js | 功能完整的引擎，內建檢視工具與 inspector；TypeScript 原生 | 套件較大；CAD 檢視相關的社群資源少於 Three.js；無對應 three-cad-viewer 的現成 CAD 檢視器 | 不採用 |
| `<model-viewer>` | 幾行標記即可顯示 glTF | 以展示為目的，難以實作元件選取、port 圖層與自訂互動 | 不採用 |
| xeokit／That Open Engine 等 BIM 檢視器 | 大型模型效能與剖切、量測功能成熟 | 以 BIM／IFC 為主要對象；授權（部分為 AGPL 或商業）與資料模型不符 | 不採用 |
| 原生渲染（wgpu／Bevy，嵌入或另開視窗） | 效能上限高；與 Rust core 同語言 | 無法在 Web 模式共用；與 WebView UI 的整合複雜 | 不採用 |
| 直接在瀏覽器執行 OCCT（WASM，如 opencascade.js） | 不需傳輸網格；可取得精確拓樸 | WASM 套件龐大；形成第二套幾何路徑，與 [0006](0006-geometry-kernel-occt-python-sidecar.md) 衝突 | 不採用 |

## 後果

### 正面

- 使用者可以互動檢視並直接在 3D 中選取元件，選取結果可作為 agent 指令的明確上下文（元件 ID），而不是模糊的文字描述。
- glTF 節點階層與元件圖一一對應，為日後的組件、爆炸圖與局部更新保留空間。
- 檢視器只消費網格與文件資料，與幾何核心的實作解耦；核心更換（第 4 階段）不影響前端。

### 負面

- 顯示的是細分後的網格，不是精確的 B-rep；面／邊層級的精確選取與量測需要 sidecar 另外提供拓樸對應資料。
- 每次幾何變更都要重新細分並傳輸；在取得增量更新機制之前，大型模型的回饋延遲會隨尺寸成長。
- CAD 慣用的檢視功能需要投入開發或導入第三方套件。

### 風險與緩解

| 風險 | 影響 | 緩解措施 |
|---|---|---|
| 大型網格的傳輸與記憶體 | 介面卡頓、WebView 記憶體壓力 | GLB 二進位傳輸；細分公差可調（現行 STL 輸出使用 `tolerance=0.05`、`angularTolerance=0.1`，預覽可採較粗設定）；以元件為單位快取未變更的網格 |
| WebView 的 WebGL 支援差異 | 特定平台無法顯示或效能不足 | WebView2 為 Chromium 核心，WebGL 可用；WKWebView 與 WebKitGTK 列入測試矩陣；啟動時偵測 WebGL 並顯示明確錯誤；暫不依賴 WebGPU |
| CadQuery 的 glTF 輸出路徑與節點命名不符需求 | 無法以元件為單位選取 | 第 1 階段先以實測確認 `cq.Assembly` 的 glTF 輸出可攜帶元件名稱；不足時於 sidecar 自行組裝節點或採用 three-cad-viewer 的資料路徑 |
| 網格與文件版本不同步 | 畫面顯示的不是目前文件 | 每份網格標記其來源文件的修訂識別碼；檢視器在不一致時顯示「過期」狀態 |

## 驗收或回顧條件

- **驗收**：
  - 可載入 sidecar 輸出的 GLB 與 STL 並進行旋轉、平移、縮放。
  - 點選網格可得到正確的元件 ID，並與元件樹雙向同步。
  - 可開關 port 圖層，未連接與已連接的 port 以不同樣式顯示。
  - 在 Windows（WebView2）、macOS（WKWebView）、Linux（WebKitGTK）與一款主流桌面瀏覽器上通過相同的檢視器測試。
- **回顧**：
  - three-cad-viewer 評估完成時。
  - 需要面／邊層級的精確選取、量測或尺寸標註時。
  - 典型模型的重新細分與載入延遲超出互動可接受範圍（門檻於第 1 階段實測後訂定）。

## 相關文件

- [0002](0002-desktop-shell-tauri-v2.md)、[0003](0003-frontend-react-typescript.md)、[0006](0006-geometry-kernel-occt-python-sidecar.md)
- [`../CONCEPT_DIMENSIONAL_LADDER.md`](../CONCEPT_DIMENSIONAL_LADDER.md)（元件與 port 的定義）
- 現行輸出：`app/services/compiler.py` 的 `export_all`
- Three.js、three-cad-viewer、glTF 2.0 規格

## 回顧紀錄

（尚無）
