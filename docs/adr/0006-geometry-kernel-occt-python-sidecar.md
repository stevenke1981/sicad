# 0006：幾何核心維持 OpenCascade（CadQuery），以 Python sidecar 執行

- **狀態**：Accepted
- **日期**：2026-11-17
- **決策者**：專案負責人
- **影響範圍**：`app/services/compiler.py`、`app/services/openscad.py`、`app/services/renderer.py`、sidecar 封裝、`Dockerfile`、`environment*.yml`

## 背景

現行幾何路徑是：`CadQueryCompiler` 將 `CadDocument` 編譯為固定樣式的 `model.py`，`Renderer` 以 `shell=False`、固定命令列、封閉工作目錄與 timeout 的 subprocess 執行它，由 CadQuery 2.8.0（底層為 OpenCascade Technology，OCCT）產生 STEP／STL／DXF／SVG。OpenSCAD 為備援，只能輸出 STL 且不套用 fillet／chamfer。

產品已依賴的 B-rep 能力包括：布林運算（孔、沉孔、沉頭孔皆以 cut 實作）、圓角與倒角、STEP 匯出、截面（DXF）。這些正是 B-rep 核心中最難做對的部分。

既然核心文件模型要移到 Rust（[0005](0005-rust-core-document-model.md)），自然會問：幾何核心是否也該換成 Rust 原生方案，以去除 Python 與 OCCT 約 300–500 MB 的發佈體積？

## 決策

**現階段維持 OpenCascade，透過 CadQuery 以 Python sidecar 執行；不採用 Rust 原生 B-rep 核心。**

- Python 的職責收斂為：**已驗證規格 → 幾何 → 匯出**（STEP、STL、glTF、DXF、SVG）。文件模型、驗證與 patch 的權威在 Rust core。
- Sidecar 沿用並強化現行隔離原則：只執行系統自身的確定性編譯結果，絕不執行 LLM 產生的程式碼；固定命令列、封閉工作目錄、timeout；無需對外網路。
- Sidecar 回傳的幾何診斷（例如現行 `render-warnings.json` 中的「fillet skipped」）以結構化 issue 回報給 Rust core，與驗證報告使用相同結構。
- Sidecar 介面以「規格輸入、產物與診斷輸出」定義，不暴露 CadQuery 物件或 Python 特有概念，使日後更換實作（第 4 階段）不影響呼叫端。
- CadQuery 版本維持固定（現為 `cadquery==2.8.0`），升級需經幾何回歸測試。
- OpenSCAD 備援在第 1 階段維持現狀；元件圖模型（2.0）是否繼續支援 OpenSCAD 後端，於第 2 階段另行決定。
- **日後重新評估**：以 `cxx` 等機制從 Rust 直接綁定 OCCT，藉此移除 Python（[0010](0010-migration-phases.md) 第 4 階段，屬選擇性）。

## 考慮過的替代方案

| 方案 | 優點 | 缺點 | 結論 |
|---|---|---|---|
| **OCCT via CadQuery（Python sidecar）** | 已在產品中驗證；布林、圓角／倒角、STEP 皆成熟；現有編譯器與測試可沿用；CadQuery API 精簡 | 發佈體積大；跨程序呼叫；需封裝 Python 執行環境；CadQuery 與 OCCT 版本需鎖定 | 採用 |
| truck（Rust 原生 B-rep） | 純 Rust、可編譯為 WASM、模組化設計 | 圓角／倒角與複雜布林運算的穩健性尚不足以取代 OCCT；STEP 支援不完整 | 不採用：成熟度不足 |
| Fornjot（Rust 原生 B-rep） | 純 Rust、以程式化 CAD 為目標 | 仍屬早期專案，功能範圍有限；缺少產品需要的圓角、通用布林與 STEP 能力 | 不採用：成熟度不足 |
| opencascade-rs（OCCT 的 Rust 綁定） | 保留 OCCT 的能力又免除 Python；可與 Rust core 同程序 | 綁定只涵蓋部分 OCCT API；需自行建置並連結 OCCT；高階 API 尚在變動 | 現在不採用；為第 4 階段的候選路徑之一 |
| 自行以 `cxx` 綁定所需的 OCCT 子集 | 只綁定用得到的 API，可控 | 需要 C++／OCCT 專業與跨平台建置投資；等於重寫 CadQuery 提供的高階操作 | 現在不採用；第 4 階段評估 |
| build123d（同為 OCCT 的 Python 介面） | API 較新、型別較明確 | 同樣是 Python + OCCT，無法解決體積；需重寫編譯器，收益有限 | 不採用 |
| Manifold 等網格布林核心 | 快速、穩健、體積小、有 WASM 版本 | 是網格而非 B-rep：沒有精確曲面、無法輸出真正的 STEP、圓角需近似 | 不採用作為主要核心；可作為快速預覽的日後選項 |
| OpenSCAD 為主要核心 | 體積小、語言簡單 | CSG／網格導向，無 STEP、無原生圓角；現行已因此僅作為備援 | 不採用 |
| 商用核心（Parasolid、ACIS 等） | 業界最穩健 | 授權費用與散佈條款與本專案的開源（MIT）定位不合 | 不採用 |
| 瀏覽器端 OCCT（WASM） | 桌面與 Web 皆免 sidecar | WASM 套件龐大、效能與記憶體受限；伺服器／CLI 模式仍需另一條路徑 | 不採用 |

## 後果

### 正面

- 幾何結果與現行版本一致，遷移風險集中在架構而非幾何正確性。
- 既有的 `CadQueryCompiler`、範例與幾何相關測試可持續作為回歸基準。
- 幾何運算在獨立程序中執行：OCCT 崩潰或逾時不會拖垮桌面應用程式，並保留現行的沙箱邊界。
- Sidecar 介面與實作解耦，為第 4 階段保留替換空間。

### 負面

- 桌面版安裝包的主要體積來自 Python + OCCT。
- 每次幾何更新都有程序間往返與（冷啟動時的）直譯器及 OCCT 載入成本。
- 需要為每個目標作業系統與 CPU 架構建置並測試 sidecar。
- Python 仍是必要相依，三語言工具鏈至少維持到第 4 階段。

### 風險與緩解

| 風險 | 影響 | 緩解措施 |
|---|---|---|
| Sidecar 體積（約 300–500 MB） | 下載、安裝與更新成本高 | 比較 conda-pack（保留 conda 環境，相容性高）與 PyInstaller（可裁剪，但 OCCT 動態函式庫的收集需調校）；移除未使用套件；sidecar 與外殼分開版本，使 UI 更新不必重新下載幾何執行環境 |
| 冷啟動延遲 | 首次輸出等待時間長 | 應用程式啟動時於背景預熱 sidecar；sidecar 為常駐程序而非每次工作重新啟動直譯器 |
| 大型網格的 IPC 負擔 | 傳輸延遲與記憶體倍增 | 產物以檔案交付（GLB／STL），訊息中只傳路徑與中繼資料（見 [0004](0004-3d-viewer-threejs.md)） |
| 常駐程序削弱現行「一工作一程序」的隔離 | 單一工作的崩潰或記憶體洩漏影響後續工作 | 常駐服務內仍以子程序或可回收的工作者執行實際幾何運算，保留 timeout 與資源上限 |
| 跨語言 schema 漂移 | Sidecar 對規格的解讀與 Rust core 不同 | Pydantic 模型由 JSON Schema 生成，不手寫（見 [0008](0008-single-schema-source-of-truth.md)） |
| CadQuery／OCCT 升級造成幾何差異 | 相同規格產生不同輸出 | 版本鎖定；以範例集的體積、包圍盒與拓樸計數作回歸比對；manifest 記錄核心版本 |
| Rust 原生核心成熟後錯失時機 | 長期背負不必要的體積 | 明訂回顧條件（見下） |

## 驗收或回顧條件

- **驗收**：
  - Sidecar 可在未安裝 Python 的機器上隨桌面版啟動並輸出 STEP／STL／GLB。
  - 相同規格在桌面 sidecar 與 Docker 伺服器模式下產生等價幾何。
  - Sidecar 介面文件中不出現 CadQuery 專屬型別。
  - 幾何核心逾時或崩潰時，桌面應用程式維持可用並顯示結構化錯誤。
- **回顧**（任一成立即重新評估）：
  - 某個 Rust 原生核心或 OCCT 綁定能通過本專案的幾何回歸測試集（布林、fillet／chamfer、STEP 往返）。
  - 封裝最佳化後 sidecar 體積或冷啟動時間仍為使用者回饋中的主要問題。
  - 第 3 階段完成，開始評估第 4 階段時。
  - CadQuery 上游停止維護或與所需的 Python／OCCT 版本不相容。

## 相關文件

- [0002](0002-desktop-shell-tauri-v2.md)、[0004](0004-3d-viewer-threejs.md)、[0005](0005-rust-core-document-model.md)、[0008](0008-single-schema-source-of-truth.md)、[0010](0010-migration-phases.md)
- 現行實作：`app/services/compiler.py`、`app/services/renderer.py`、`app/services/openscad.py`
- 封裝：`Dockerfile`、`environment.yml`、`environment.runtime.yml`、`pyproject.toml`（`cad` extra）
- [`../SECURITY.md`](../SECURITY.md)
- CadQuery、OpenCascade Technology、truck、Fornjot、opencascade-rs、cxx

## 回顧紀錄

### 2026-11-26：CadQuery 打包進桌面 sidecar（第 1 階段 1b）

- **封裝方式：PyInstaller onedir**，由 Tauri 以 `bundle.resources` 帶到 `<安裝目錄>/sidecar/`。不採用 onefile：onefile 每次啟動都要把數百 MB 解壓到 `%TEMP%\_MEI*`，而外殼以 Job Object 終止 sidecar 時 bootloader 來不及清理，實測舊版 onefile 確實遺留了 `_MEI*` 目錄；onedir 版本不會產生。
- **render worker**：凍結後沒有獨立的 Python 直譯器，renderer 改以 `promptcad-sidecar.exe --render-worker <model.py> --output-dir … --formats …` 在獨立子行程執行 `model.py`（`app/render_worker.py`）。worker 只接受資料目錄（`PROMPTCAD_RENDER_ROOT`）之內、檔名為 `model.py` 的工作；子行程環境移除 `PROMPTCAD_API_TOKEN` 與 `PROMPTCAD_LLM_API_KEY`。原始碼／Docker 模式的命令不變。
- **修剪**：排除 `scipy`、`llvmlite`、`numba`、`matplotlib` 等（實測 `model.py` 不會匯入）；`vtk` 與 `casadi` 因 `cadquery` 在頂層匯入而保留。
- **驗收**：`scripts/frozen_smoke.py` 以凍結後的 exe 對五種基體 × 六種孔型輸出 STEP／STL／DXF／SVG 全數通過；安裝到含空白的路徑後，桌面版產生範例固定板得到 `completed／cadquery`，`model.step` 以 `ISO-10303-21` 開頭。

量測（Windows 11、24 CPU、cadquery 2.8.0、PyInstaller 6.22.3、Python 3.12.10）：

| 項目 | 數值 |
|---|---|
| sidecar onedir 大小／檔案數 | 547.5 MiB／1,168 個 |
| NSIS 安裝包（LZMA） | 107.0 MiB |
| 安裝後大小 | 550.9 MiB |
| sidecar 啟動到 READY（第一次／暖啟動中位數） | 0.85 s／0.55 s |
| render worker，`plate-four-holes`（建置後第一次／暖中位數） | 9.65 s／2.21 s（第一次含防毒掃描新檔） |
| `POST /generate` 端到端（rule planner，4 種格式） | 約 2.2 s |

最大的組成：`vtk.libs` 264 MiB、`OCP` 90 MiB、`cadquery_ocp.libs` 62 MiB、`vtkmodules` 42 MiB、`casadi` 24 MiB。VTK 約佔 56%，是之後縮小體積最大的槓桿。

**conda-pack 與 PyInstaller：選定 PyInstaller，未實作 conda-pack 原型。** 理由：本機沒有 conda；conda 原本的優勢（OCP 二進位相容）已由 pip wheel 解決（`cadquery==2.8.0` 以 pip 安裝 39 秒、渲染正確）；conda-pack 打包整個環境、無法依匯入圖修剪，**估計**會比未修剪的 1.1 GB venv 更大（此為推估，未實測）；且需要安裝時執行 `conda-unpack` 並引入第二套工具鏈。重新評估條件：凍結 smoke 無法在約一天的 hook 調整內通過，或安裝包超過 500 MB 且 vtk／casadi 的替身方案失敗。此結論是否滿足 ADR 0010「比較已完成」的完成條件，原列為待負責人簽核；**負責人已於 2026-11-26 簽核**（見下方紀錄）。

後續：VTK／casadi 替身以縮小體積、第三方授權聲明（OCCT 與 casadi 為 LGPL、VTK 為 BSD）在公開發佈前補齊、更新時整包重新下載的問題尚未處理。

### 2026-11-26：負責人簽核 PyInstaller，第三方授權聲明補齊

- **簽核**：負責人於 2026-11-26 核准以 PyInstaller onedir 取代 conda-pack 作為 sidecar 封裝方式，並同意上一筆紀錄的比較（conda-pack 以推估理由排除、未建原型）視為滿足 ADR 0010「封裝比較已完成並選定其一」的完成條件。上述重新評估條件維持有效。本紀錄不改動上方已接受的決策內容。
- **第三方授權聲明**：新增 `scripts/gen_third_party_notices.py`（僅用標準函式庫），產生並納入版本控制的根目錄 `THIRD_PARTY_NOTICES.md`。來源為實際安裝／鎖定的相依套件：以 `--python` 指定的直譯器（`.venv-desktop`）透過 `importlib.metadata` 列出凍結進 sidecar 的 Python 套件（排除 PyInstaller 等建置工具與 `build_sidecar.py` 修剪的模組）、`web/package-lock.json` 的正式相依、`src-tauri` 的 `cargo metadata`（`x86_64-pc-windows-msvc`、僅 normal 相依），另加手動維護的 OCCT、VTK、CPython、PyInstaller bootloader、NLopt、MSVC runtime、WebView2 bootstrapper 與 NSIS。授權全文以 SHA-256 去重，輸出排序且不含時間戳，`--check` 可比對漂移，`--strict` 在授權不明或出現未知的 wheel 隨附 DLL 時失敗。
- **LGPL 處理**：聲明檔的「LGPL 元件與替換權利」段落列出 OCCT（經 OCP 載入，LGPL-2.1 WITH OCCT-exception-1.0）、casadi（LGPL-3.0-or-later）、NLopt（wheel metadata 標示 MIT，但編譯後函式庫受 LGPL-2.1+ 規範）、LibRaw 與 VTK 內建的 gl2ps，說明 onedir 版面下這些 `.dll`／`.pyd` 以獨立檔案存放在 `<安裝目錄>\sidecar\_internal\`、可由使用者替換，並附原始碼網址與版本。OCCT 的 LGPL-2.1 全文與例外條款逐字收錄於產生器中（來源：OCCT `V7_9_3` 標籤）。
- **隨安裝包發佈**：`tauri.conf.json` 的 `bundle.resources` 新增 `"../THIRD_PARTY_NOTICES.md": "THIRD_PARTY_NOTICES.md"`，安裝後位於 `<安裝目錄>\THIRD_PARTY_NOTICES.md`；`desktop-bundle.yml` 在 `tauri build` 前以 `--strict` 自 `.venv-desktop` 重新產生並上傳，安裝後檢查該檔存在且大於 10 KB。
- **交叉核對**：以 PyInstaller 的分析結果（`build/sidecar/work/*/PYZ-00.toc`、`COLLECT-00.toc`）核對，凍結內容中的每個套件都列在聲明檔中；`setuptools`（含 `_vendor`）實際被凍結，因此列入而不視為建置工具。
- 本聲明檔是授權資訊彙整，不是法律意見；公開發佈前仍建議由負責人複核「需特別留意的授權」表（GPL／LGPL／MPL／EPL／CDDL）。

### 2026-11-27：2.0 幾何編譯器與輸出契約（第 2 階段 2d）

- **編譯器只讀 `resolved`**：2.0 幾何由 `Cad2Compiler`（`app/services/compiler2.py`）產生。它讀取 Rust core 驗證結果中的 `document` 與 `resolved`：零件依 `resolved.components` 剛體放置，主孔切刀依 `resolved.holes`，沉頭與埋頭的入口依 `resolved.ports["<id>.entry"]` 以 §7.2 算術計算。Python 不做任何求解。產生的程式碼不含文件文字（以索引命名、數值經 `float` 輸出、軸向以固定表格對應），輸出確定性。
- **未選「降級後交給 1.0 編譯器」**：降級會讓等價測試變成同義反覆；降級不保留幾何（沿軸向的入口被投影到 0）；且無法表達多零件文件（enclosure-with-lid 為 `expressible: false`）。降級保留為測試參照（oracle）。
- **輸出契約不變**：`model.py` 維持 1.0 的契約（`build() -> (result, warnings)`、`export_all`、`--output-dir`／`--formats`、`render-warnings.json`）。renderer 與 render worker 未改動。
- **2.0 不使用 OpenSCAD**：2.0 呼叫 `Renderer.render(allow_openscad=False)`。無 CadQuery 時回 `source_only` 並附固定警告，不產生 `model.scad` 與 `preview.svg`。
- **無法編譯的構造明確拒絕**：不支援的構造拋出 `CompileUnsupported`，列出所有 `(id, 原因)`，工作狀態為 `failed`／`compile-unsupported`，不會靜默略過。
- **已知偏差**：2.0 幾何在世界座標建立，不採用 [CAD_DSL_2](CAD_DSL_2.md) §6.2 的零件座標系與 `cq.Location` 組裝（延後）。
- **通孔切刀方向**：非盲孔的主切刀由負端沿正向軸建立（1.0 的形式）。改用另一種參數化時，`cylinder_y_axis_outside` 的 OCC 體積積分相對差異為 3.3e-6。
- **本機證據**：`scripts/geometry_equivalence.py` 的 54 個唯一案例全數與 1.0 等價（見 [0010](0010-migration-phases.md) 的 2d 回顧）；CI 結果待確認。

### 2026-11-27：1.0 幾何改由 `Cad2Compiler` 產生（第 2 階段 2e）

- **單一 CadQuery 編譯器**：`app/services/compiler2.py`（`Cad2Compiler`）是專案唯一的 CadQuery 編譯器，1.0 與 2.0 共用；`app/services/compiler.py` 已刪除。1.0 流程經 core 的單一 `validate` 呼叫取得報告、`resolved` 與正規化文件後交給它。
- **保留 `model.scad` 與 `preview.svg`**：兩者仍由 1.0 規格經 `openscad.py` 與 `preview.py` 產生，未改動。理由：`preview.svg` 是 source-only 使用者唯一的視覺（前端會退回顯示它）；`model.scad` 是沒有 CadQuery 時唯一的 STL 路徑。代價：`model.scad` 使用獨立的 1.0 算術，不受幾何 oracle 涵蓋。這個缺口先前即存在，本切片未擴大。
- **回歸基準改為凍結的 golden**：舊編譯器移除前，`tests/fixtures/v1_geometry_golden.json` 凍結其輸出（A/B 為 54／54 相等）。metrics 層需要 CadQuery；source 層以 `Cad2Compiler` 原始碼的 sha256 檢查，不需要 CadQuery。舊編譯器的對稱差檢查已不再進行，表面積指標僅部分補償。
- **跨平台證據**：移除前的 golden 於 CI 的 Linux conda CadQuery 2.8.0 通過 54／54，未需要任何核心敏感的例外。
- **本機證據**：移除後的 golden 模式 54 個 equal、3 個 native equal；實機輸出 `examples/generated/plate-four-holes` 約 5.4 秒，其 `model.py` 與 golden 的 `model_sha256` 相同。
