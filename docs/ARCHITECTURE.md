# Architecture

```text
Prompt
  ├─ RuleBasedPlanner
  └─ OpenAICompatiblePlanner
          ↓
LLMIntent / normalized intent
          ↓
CadDocument 1.0
(Pydantic, extra=forbid, units=mm, bounded lists/numbers)
          ↓
DesignValidator
  ├─ valid ───→ CadQueryCompiler + OpenScadCompiler → Renderer
  └─ invalid ─→ 保存 DSL／原始碼／預覽，但 validation-blocked
          ↓
Job manifest + downloadable artifacts
```

本文描述目前可執行的 v0.1.0 架構，以及 [ADR 0010](adr/0010-migration-phases.md) 第 1 階段加入、尚未發佈的前端與桌面外殼（見「前端、桌面外殼與 schema 管線」一節）。目標架構見 [`adr/overview.md`](adr/overview.md)；CadDocument 2.0 的凍結規格見 [`CAD_DSL_2.md`](CAD_DSL_2.md)，目前沒有任何程式接受 2.0 文件。

## 為何加入 DSL

直接讓模型輸出並執行 CAD Python 會帶來任意程式碼執行、不可重現、套件版本漂移與幾何失敗風險。DSL 將能力限制在明確的幾何白名單；模型只負責理解需求，Pydantic 負責型別與範圍，編譯器負責確定性實作。

同一份 `spec.json` 可以：

- 經 Web UI 修改後重新生成。
- 透過 CLI `promptcad render spec.json` 重現。
- 放入 Git 進行設計審查與差異比較。
- 由不同 renderer 產生可比較的輸出。

## 坐標系

- 單位固定為毫米。
- 原點位於零件 XY 中心、底面 Z=0。
- 板件、圓柱、圓環與盒體沿 +Z 建立。
- 孔位置使用全域 `(x, y, z)`。
- `axis=z`：孔在 XY 平面定位；正面入口位於 +Z 面。
- `axis=x`：孔在 YZ 平面定位；正面入口位於 +X 面。
- `axis=y`：孔在 XZ 平面定位；正面入口位於 +Y 面。
- 盲孔、沉孔與沉頭孔由正向外表面向零件內部切除。

## Renderer 選擇

`PROMPTCAD_RENDER_BACKEND`：

- `auto`：CadQuery → OpenSCAD → source-only。
- `cadquery`：優先 CadQuery；失敗時依 `ALLOW_SOURCE_FALLBACK` 決定是否降級。
- `openscad`：可輸出 STL，但不支援 STEP／DXF，且 fallback 不套用 fillet/chamfer。
- `source_only`：只產生可重現來源與預覽。

Renderer 只執行系統編譯器生成的固定程式，使用 `shell=false`、固定命令列、封閉工作目錄及 timeout。

## 驗證閘門

驗證分為 schema 與設計兩層：

1. Pydantic schema：型別、必填欄位、未知欄位、正數、最大尺寸、最大特徵數。
2. DesignValidator：邊界、有效孔徑、孔重疊、材料深度、圓環內孔、薄壁、邊距、圓角／倒角。

只要出現 `error`，renderer 就不會啟動。`warning` 不阻止輸出，但 `review_required=true`。

## 儲存模型

每個工作使用不可預測 UUID hex 目錄，包含：

```text
spec.json
validation.json
model.py
model.scad
preview.svg
model.step / model.stl / model.dxf / model.svg（依環境與請求）
render-warnings.json（CadQuery 執行時）
manifest.json
```

manifest 記錄 planner、renderer、請求格式、狀態、警告及產物，不把 API Key 寫入檔案。

## 前端、桌面外殼與 schema 管線（第 1 階段，未發佈）

```text
Pydantic models (app/models) ──export_schema.py──→ schemas/promptcad.schema.json
                                                        │ json-schema-to-typescript
                                                        ↓
web/ (React + TS + Vite) ──import──→ web/src/api/generated/promptcad.d.ts
  │  npm --prefix web run build
  ↓
app/web_dist/ ──served at "/" by FastAPI when index.html exists (else app/static/)
  │
  ├─ 瀏覽器／Docker 模式：同源呼叫 /api/v1/*，Token 由 UI 欄位提供
  └─ 桌面模式：src-tauri/（Tauri v2）載入 app/web_dist/
         │ 啟動 sidecar：promptcad-sidecar --host 127.0.0.1 --port 0
         │   env: PROMPTCAD_API_TOKEN（一次性）、PROMPTCAD_DATA_DIR、PROMPTCAD_CORS_ORIGINS
         │ 讀取 stdout：PROMPTCAD_SIDECAR_READY {"port": N}
         └─ command backend_info() → {base_url, token} → 前端 bootstrap.ts
```

- **前端**：`web/` 是 React + TypeScript（strict）+ Vite 應用程式，功能對齊 `app/static/`。API 呼叫集中在 `web/src/api/`：`client.ts` 定義介面、`httpClient.ts` 實作 HTTP 與 Token 標頭、`bootstrap.ts` 是唯一接觸 `window.__TAURI__` 的檔案（有 Tauri 時以 `backend_info` 取得位址與 Token，否則使用同源位址）。模型型別全部由 schema 生成，不手寫。
- **檢視器**：`web/src/viewer/` 以 Three.js 載入 `model.stl`（`STLLoader`、`OrbitControls`、WebGL 能力偵測），核心不依賴 React（[ADR 0004](adr/0004-3d-viewer-threejs.md)）。
- **前端服務**：`Settings.frontend_dir`（預設 `app/web_dist`）含 `index.html` 時，FastAPI 在 `/` 提供它，並在 `/assets` 掛載建置資產；否則維持原本的 `app/static/` 行為。`app/web_dist/` 不納入版本控制，但會隨套件資料安裝。
- **桌面外殼**：`src-tauri/` 是 Tauri v2 應用程式（[ADR 0002](adr/0002-desktop-shell-tauri-v2.md)），以 `stable-x86_64-pc-windows-msvc` 工具鏈建置。sidecar 由 Rust 端以固定參數啟動，WebView 沒有任何 shell 權限（capability 只開放 `core:default` 與 `backend_info`）；sidecar 只綁定 `127.0.0.1`，所有 `/api/v1/*` 請求都要帶一次性 Token；程式結束時終止 sidecar（Windows 以 Job Object 一併終止子行程）。開發時（僅 debug 建置）可用 `PROMPTCAD_SIDECAR_CMD` 以本機 Python 取代打包的 sidecar；release 建置忽略此變數。
- **Sidecar**：`app/sidecar.py` 是同一個 FastAPI 應用程式的進入點；`scripts/build_sidecar.py` 以 PyInstaller 打包成 `src-tauri/binaries/promptcad-sidecar-<target-triple>.exe`。**CadQuery 尚未打包**，因此桌面版的 renderer 為 `source_only`。
- **Schema 管線**：`scripts/export_schema.py` 以確定性方式（排序鍵、LF、結尾換行）輸出 `schemas/promptcad.schema.json`；`--check` 在漂移時以非零狀態結束。CI 另檢查 `npm run gen:types` 之後生成的型別沒有差異（[ADR 0008](adr/0008-single-schema-source-of-truth.md) 的過渡安排；第 2 階段 schema 來源改為 Rust）。

## 擴充新特徵

1. 在 `app/models/cad.py` 加入新的 base 或 feature 型別與 schema 限制。
2. 在 `compiler.py` 與 `openscad.py` 實作確定性編譯。
3. 在 `validator.py` 加入邊界、材料深度與特徵互動檢查。
4. 更新 `LLMIntent`、system prompt 與 rule parser。
5. 新增語意、原始碼與 API 回歸測試。
6. 更新 DSL 文件與範例。
7. 執行 `python scripts/export_schema.py` 與 `npm --prefix web run gen:types`，提交更新後的 schema 與 TypeScript 型別。

## 生產化建議

- 將 renderer 拆成無對外網路的獨立 worker 與工作佇列。
- 每個工作使用容器、非 root 使用者、唯讀根檔案系統、seccomp、CPU／記憶體／檔案大小／時間限制。
- 使用 PostgreSQL 與物件儲存取代單機目錄，並加入租戶與配額。
- 對 prompt、schema、compiler、kernel 與輸出保存版本 provenance。
- 增加 BREP 有效性、最小壁厚、干涉、可加工性、公差堆疊與材料規則。
- 對複雜／退化幾何進行 fuzzing 與 renderer 崩潰隔離。
