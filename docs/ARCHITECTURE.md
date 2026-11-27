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
Rust core（validate，經 app/services/core_validator.py）
  ├─ valid ───→ Cad2Compiler（model.py）+ OpenScadCompiler → Renderer
  └─ invalid ─→ 保存 DSL／原始碼／預覽，但 validation-blocked
          ↓
Job manifest + downloadable artifacts
```

本文描述目前可執行的 v0.1.0 架構，以及 [ADR 0010](adr/0010-migration-phases.md) 第 1 階段加入、尚未發佈的前端與桌面外殼（見「前端、桌面外殼與 schema 管線」一節）。目標架構見 [`adr/overview.md`](adr/overview.md)；CadDocument 2.0 的凍結規格見 [`CAD_DSL_2.md`](CAD_DSL_2.md)，2.0 的 Rust core 已實作於 `crates/promptcad-core/`（見「Rust core crate」一節），並已由桌面版（Tauri 命令）與伺服器（`POST /api/v1/core`）兩種宿主呼叫（見「Rust core 的宿主與資料流」）。自第 2 階段 2c 起，1.0 的驗證也由 core 執行，Python 端不再含任何設計規則；生成流程的其餘部分不變。2.0 文件可經 core 驗證並輸出幾何（第 2 階段 2d，未發佈）：`Cad2Compiler` 只讀取 core 的 `resolved`，不使用 OpenSCAD，支援的構造見下節與 `PROJECT_STATUS.md`。自第 2 階段 2e 起，1.0 的 `model.py` 也由 `Cad2Compiler` 產生（見「1.0 生成路徑」一節），`CadQueryCompiler` 已移除。

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
2. 設計規則（Rust core 的 `validate` 操作，經 `CoreValidator` 呼叫）：邊界、有效孔徑、孔重疊、材料深度、圓環內孔、薄壁、邊距、圓角／倒角。

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
app/web_dist/ ──served at "/" by FastAPI when index.html exists (else 503 "build the frontend" page)
  │
  ├─ 瀏覽器／Docker 模式：同源呼叫 /api/v1/*，Token 由 UI 欄位提供
  └─ 桌面模式：src-tauri/（Tauri v2）載入 app/web_dist/
         │ 啟動 sidecar：promptcad-sidecar --host 127.0.0.1 --port 0
         │   env: PROMPTCAD_API_TOKEN（一次性）、PROMPTCAD_DATA_DIR、PROMPTCAD_CORS_ORIGINS
         │ 讀取 stdout：PROMPTCAD_SIDECAR_READY {"port": N}
         └─ command backend_info() → {base_url, token} → 前端 bootstrap.ts
```

- **前端**：`web/` 是 React + TypeScript（strict）+ Vite 應用程式，是唯一的 Web UI（原本無建置步驟的 `app/static/` 已於 2026-11-26 移除，見 [ADR 0003](adr/0003-frontend-react-typescript.md) 回顧紀錄）。CadQuery 投影的 `model.svg` 以米白紙色底（`#f3f1ea`）顯示以利閱讀黑色線條，`preview.svg` 維持自身的深色底。API 呼叫集中在 `web/src/api/`：`client.ts` 定義介面、`httpClient.ts` 實作 HTTP 與 Token 標頭、`bootstrap.ts` 是唯一接觸 `window.__TAURI__` 的檔案（有 Tauri 時以 `backend_info` 取得位址與 Token，否則使用同源位址）。模型型別全部由 schema 生成，不手寫。
- **檢視器**：`web/src/viewer/` 以 Three.js 載入 `model.stl`（`STLLoader`、`OrbitControls`、WebGL 能力偵測），核心不依賴 React（[ADR 0004](adr/0004-3d-viewer-threejs.md)）。
- **前端服務**：`Settings.frontend_dir`（預設 `app/web_dist`）含 `index.html` 時，FastAPI 在 `/` 提供它，並在 `/assets` 掛載建置資產；否則 `/` 回傳 HTTP 503 的內嵌說明頁（`app/main.py` 的字串常數，提示 `npm --prefix web run build` 並連到 `/docs`），`/assets` 不掛載，API 不受影響。`app/web_dist/` 不納入版本控制，但會隨套件資料安裝（非 editable 的 wheel 只有在先建置前端時才含 UI）。Docker 映像以多階段建置：`node:24-slim` 階段建置 `web/`，再複製到 `/app/app/web_dist`；CI 的 `docker` job 建置並啟動映像，確認 `/` 提供新前端。凍結的桌面 sidecar 不內含任何 Web UI，桌面 UI 由 Tauri 自身的資產提供。
- **桌面外殼**：`src-tauri/` 是 Tauri v2 應用程式（[ADR 0002](adr/0002-desktop-shell-tauri-v2.md)），以 `stable-x86_64-pc-windows-msvc` 工具鏈建置。sidecar 由 Rust 端以固定參數啟動，WebView 沒有任何 shell 權限（capability 只開放 `core:default`、`backend_info` 與 `allow-core-request`）；sidecar 只綁定 `127.0.0.1`，所有 `/api/v1/*` 請求都要帶一次性 Token；程式結束時終止 sidecar（Windows 以 Job Object 一併終止子行程）。開發時（僅 debug 建置）可用 `PROMPTCAD_SIDECAR_CMD` 以本機 Python 取代打包的 sidecar；release 建置忽略此變數。
- **Sidecar**：`app/sidecar.py` 是同一個 FastAPI 應用程式的進入點；`scripts/build_sidecar.py`（在含 `cadquery==2.8.0` 的 `.venv-desktop` 中執行）以 PyInstaller **onedir** 打包到 `src-tauri/binaries/promptcad-sidecar/`，Tauri 以 `bundle.resources` 安裝到 `<安裝目錄>/sidecar/`（[ADR 0006](adr/0006-geometry-kernel-occt-python-sidecar.md)）。凍結後沒有獨立直譯器，renderer 以 `promptcad-sidecar.exe --render-worker <model.py> …` 在子行程執行 `model.py`（`app/render_worker.py` 只接受資料目錄內的 `model.py`；子行程環境不含 API Token 與 LLM 金鑰），桌面版因此可輸出 STEP／STL／DXF／SVG。原始碼與 Docker 模式仍以 `python model.py` 執行。
- **桌面版驗證**：`.github/workflows/desktop-bundle.yml`（`windows-latest`，手動或 `v*` tag 觸發）代替乾淨機器：`scripts/frozen_smoke.py --isolated-env` 讓 sidecar 只看到系統 `PATH` 且移除 `PYTHON*`／`VIRTUAL_ENV`／`CONDA*`，並監看行程樹，出現 sidecar 目錄外的映像或 `python*.exe`／`py.exe` 即失敗；`scripts/frozen_kill_check.py`（標準函式庫 + ctypes）以 `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` 的 Job Object 啟動 sidecar，在 render worker 執行中關閉 Job，確認追蹤的 PID 於 5 秒內全部結束（`--shell-exe` 另以已安裝的外殼驗證 `job.rs`）。`ci.yml` 只執行這些腳本的純函式測試。
- **第三方授權聲明**：`scripts/gen_third_party_notices.py` 由 `.venv-desktop` 的 `importlib.metadata`、`web/package-lock.json` 的正式相依與 `cargo metadata`（`x86_64-pc-windows-msvc`）加上手動維護的原生元件表，產生確定性的 `THIRD_PARTY_NOTICES.md`（授權全文以 SHA-256 去重）；`tauri.conf.json` 以 `bundle.resources` 將它安裝到 `<安裝目錄>\THIRD_PARTY_NOTICES.md`。LGPL 元件（OCCT／OCP、casadi、NLopt）在 onedir 版面下是 `sidecar\_internal\` 內的獨立檔案，可由使用者替換。
- **Schema 管線**：`scripts/export_schema.py` 以確定性方式（排序鍵、LF、結尾換行）輸出 `schemas/promptcad.schema.json`；`--check` 在漂移時以非零狀態結束。CI 另檢查 `npm run gen:types` 之後生成的型別沒有差異（[ADR 0008](adr/0008-single-schema-source-of-truth.md) 的過渡安排；第 2 階段 schema 來源改為 Rust）。

## Rust core crate（第 2 階段 2a、2b、2c，未發佈）

`crates/promptcad-core/` 是依 [`CAD_DSL_2.md`](CAD_DSL_2.md) 與 [ADR 0005](adr/0005-rust-core-document-model.md) 實作的獨立 library crate，不依賴 Tauri，有自己的 `Cargo.lock`。**2b 起，兩個應用程式宿主呼叫它**：桌面版的 Tauri 命令 `core_request`（`src-tauri/src/core.rs`，行程內呼叫）與伺服器的 `POST /api/v1/core`（每次呼叫啟動一次 `promptcad-core rpc` 子行程）。兩者共用 `promptcad_core::api` 的同一契約。Python 編譯器自第 2 階段 2d 起接受 core 驗證後的 2.0 文件，見下節「2.0 生成路徑」。命令列用法見 `crates/promptcad-core/README.md`。

模組分層（由下而上，上層只依賴下層）：

```text
model        CadDocument 2.0／1.0 型別、id 與參照、24 個軸對齊朝向、issue 與 §9.2 代碼表、
             patch 型別、resolved.json 型別（serde + schemars，deny_unknown_fields）
fields, canonical     欄位處理；正規化序列化與 SHA-256
expr, bindings        封閉文法的運算式與綁定求值（§4）
ports, solver         port 表（§3.4）；朝向與位移的不動點求解、孔方向與切削體（§5、§7）
validate              schema → 結構 → 綁定 → 求解 → 設計規則 的分層驗證
patch_engine, oplog   原子套用 patch（§8.5 權限矩陣）、復原；append-only JSONL op log 與重播
migrate               upgrade_1_to_2（全函式）、downgrade_2_to_1（部分函式）
bin/promptcad-core    命令列工具（schema 匯出與其他子命令）
```

- **Schema**：`promptcad-core schema --out-dir ../../schemas/cad2` 由 Rust 型別生成 `schemas/cad2/` 的五份 JSON Schema，`ci.yml` 的 `core` job 以 `--check` 比對漂移（該 job 已於 PR #5 在 GitHub 上執行並通過）。2.0 的 core 請求與回應 TypeScript 型別已由這兩份 schema 生成（`web/src/api/generated/core-request.d.ts`、`core-response.d.ts`，由 `web/scripts/gen-types.mjs` 產生）；2.0 文件本身的 Pydantic 生成型別尚不存在；1.0 應用程式仍以 `app/models/cad.py` 為 schema 來源。
- **驗證範圍**：解析檢查對 `extrude`、`revolve` 與沉頭孔錐體使用保守的外包圖元（可能漏報，不會憑空產生 issue）；核心層（kernel）驗證代碼不在此 crate，屬於 sidecar。
- **與 1.0 的關係**：2c 起，1.0 的驗證也經 core 的 `validate` 執行。移除 Python 驗證器前，以 83 個輸入做差異閘門（77 個判定一致，6 個為記錄的允許差異）；移除後以黃金基準 `tests/fixtures/v1_validation_golden.json` 做回歸。細節見 `PROJECT_STATUS.md` 的「第 2 階段 2c 切片」。

## Rust core 的宿主與資料流（第 2 階段 2b，未發佈）

Rust core 有三個宿主，共用 `promptcad_core::api` 的 `CoreRequest`（以 `op` 區分）與 `CoreResponse`（以 `status` 區分 `ok`／`error`）。宿主形式的決策與替代方案見 [ADR 0012](adr/0012-server-host-for-rust-core.md)（待負責人簽核）。

| 宿主 | 呼叫方式 | 程序 |
|---|---|---|
| 桌面版 | Tauri 命令 `core_request`（權限 `allow-core-request`） | 與 Tauri 外殼同一程序，在阻塞執行緒中執行 |
| 伺服器與 Docker | `POST /api/v1/core`（需 Token）→ 子行程 `promptcad-core rpc` | 每次呼叫一個子行程；stdin 讀一個請求，stdout 寫一行回應 |
| 命令列 | `promptcad-core <子命令>` 與 `rpc` | 獨立程序 |

**2.0 驗證在桌面版的資料流**：

```text
編輯器（2.0 文件）
  └─ web/src/api/coreClient.ts（Tauri transport，經 bootstrap.ts 取得 invoke）
       └─ invoke("core_request", { request: { op: "validate", document } })
            └─ src-tauri/src/core.rs：respond()，阻塞執行緒
                 └─ promptcad_core::api::handle → CoreResponse
                      └─ 回應 JSON 原樣回傳前端（與 rpc 逐位元組一致）
```

**2.0 驗證在伺服器模式的資料流**：

```text
編輯器（2.0 文件）
  └─ web/src/api/coreClient.ts（HTTP transport，Bearer Token）
       └─ POST /api/v1/core（app/api/routes.py）
            ├─ 讀取本文，超過 PROMPTCAD_CORE_MAX_REQUEST_BYTES → 413；非 JSON 物件 → 422
            └─ app/services/core_bridge.py：Popen([promptcad-core, "rpc"])，shell=false
                 ├─ stdin：請求位元組；stdout：一行回應（超過 32 MiB → 502）
                 ├─ 逾時（PROMPTCAD_CORE_TIMEOUT_SECONDS）→ 子行程被結束 → 504
                 ├─ 執行檔不存在或無法啟動 → 503；非零結束或無法解析 → 502
                 └─ 子行程環境只保留白名單變數（不含 Token 與 LLM 金鑰）
            └─ 回應位元組原樣回傳，ok 與 error 皆為 HTTP 200
```

- 伺服器不解讀 2.0 內容（[ADR 0008](adr/0008-single-schema-source-of-truth.md) 規則 4）；驗證規則只存在於 Rust core。
- 1.0 驗證（2c）：`/plan`、`/generate`、`/generate-from-spec`、`/validate` 與 CLI 經 `app/services/core_validator.py`（`CoreValidator`，透過 `CoreBridge`）呼叫 `validate`。core 不可用時這些端點回 `503`，逾時 `504`，core 失敗 `502`；錯誤對應見 `docs/API.md`。
- 找不到 core 執行檔時，`/api/v1/capabilities` 的 `core_available` 為 `false`，1.0 驗證端點回 `503`。桌面版 sidecar 內含 core（`<onedir>/_internal/`），由 `scripts/build_sidecar.py` 複製。
- 執行檔的查找順序與設定見 `docs/API.md` 的 `POST /core` 一節。

## 2.0 生成路徑（第 2 階段 2d，未發佈）

`POST /api/v1/generate-from-document`、CLI 的 `render`（2.0 檔案）與前端「套用 JSON 並重新輸出」共用同一條路徑：

1. `CoreValidator.validate_document` 呼叫 core 的 `validate`，取得正規化的 `document`、`resolved` 與報告。
2. 報告不合法：工作為 `failed`／`validation-blocked`，不產生 `model.py`，但寫入 `resolved.json`。
3. `Cad2Compiler`（`app/services/compiler2.py`）只讀取 `document` 與 `resolved` 產生 `model.py`。零件依 `resolved.components` 剛體放置，主孔切刀依 `resolved.holes`，沉頭與埋頭的入口依 `resolved.ports["<id>.entry"]` 以 §7.2 算術計算。Python 不做求解。不支援的構造拋出 `CompileUnsupported`，工作為 `failed`／`compile-unsupported`。
4. `model.py` 沿用 1.0 的契約，由 renderer 與 render worker 執行（兩者未改動）。2.0 不回退到 OpenSCAD。

等價性由 `scripts/geometry_equivalence.py`（`make geometry-check`）檢查，見「1.0 生成路徑」一節。

## 1.0 生成路徑（第 2 階段 2e，未發佈）

`POST /api/v1/generate`、`POST /api/v1/generate-from-spec` 與 CLI 的 `generate`、`render` 共用同一條路徑，`model.py` 由與 2.0 相同的 `Cad2Compiler` 產生：

1. `CoreValidator.validate_spec` 呼叫 core 的 `validate` 一次，取得報告、`resolved` 與正規化的 2.0 文件（每個工作只呼叫 core 一次）。
2. `Cad2Compiler` 產生 `model.py`。不論報告是否合法都會嘗試產生；報告不合法時工作仍為 `failed`／`validation-blocked`，且 `model.py` 仍會寫入（與 2e 之前相同）。
3. `CompileUnsupported` 時工作為 `failed`／`compile-unsupported`，不產生 `model.py`。
4. `model.scad` 與 `preview.svg` 仍由 1.0 規格產生（`openscad.py`、`preview.py`），不經 core。這兩個檔案在 `compile-unsupported` 時仍寫入。1.0 工作不產生 `resolved.json`。

由於 `model.py` 改由 `Cad2Compiler` 產生，1.0 的 `model.py` 有以下變更：不再內嵌 `CAD_SPEC`；文件字串說明它由 2.0 文件產生；含 `place()` 輔助函式。完整清單見 `PROJECT_STATUS.md` 的「第 2 階段 2e 的 1.0 行為變更」。

幾何回歸由 `scripts/geometry_equivalence.py`（`make geometry-check`）檢查。預設的 golden 模式比對凍結的 `tests/fixtures/v1_geometry_golden.json`，而不再比對舊編譯器；該檔的數值取自舊編譯器移除前的輸出。metrics 層需要 CadQuery，source 層（`tests/test_v1_compile_golden.py`）不需要。

## 擴充新特徵

1. 在 `app/models/cad.py` 加入新的 base 或 feature 型別與 schema 限制。
2. 在 `app/services/compiler2.py`（`Cad2Compiler`，1.0 與 2.0 共用）與 `openscad.py` 實作確定性編譯。
3. 在 `crates/promptcad-core/src/validate/` 加入邊界、材料深度與特徵互動檢查（設計規則只存在於 Rust core；Python 端不再實作）。
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
