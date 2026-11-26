# Project Status — v0.1.0（含未發佈的第 1 階段切片）

版本號仍為 0.1.0。下方「v0.1.0 可用功能」是已發佈的 MVP；「第 1 階段切片（Unreleased）」是依 [ADR 0010](docs/adr/0010-migration-phases.md) 第 1 階段加入、尚未發佈的部分。

## v0.1.0 可用功能

- Prompt → 本地規則／OpenAI-compatible LLM → CadDocument 1.0。
- Web UI、REST API、CLI。
- 可編輯 JSON 後重新驗證與輸出。
- Plate、cylinder、ring、L bracket、open enclosure。
- Through、blind、clearance、tapped approximation、counterbore、countersink。
- X／Y／Z 軸孔、fillet、chamfer。
- CadQuery STEP／STL／DXF／SVG 與 OpenSCAD STL fallback。
- 幾何驗證閘門、Token、下載路徑保護與工作 ZIP。

## 第 1 階段切片（Unreleased / Phase 1a）

| 項目 | 位置 | 狀態 |
|---|---|---|
| React + TypeScript + Vite 前端（已取代 `app/static/`） | `web/` | 已實作；建置輸出到 `app/web_dist/` |
| FastAPI 提供新前端 | `app/main.py`、`app/core/config.py`（`frontend_dir`） | `app/web_dist/index.html` 存在時於 `/` 提供；否則 `/` 回傳 503 說明頁（`app/static/` 已移除） |
| Three.js STL 檢視器（最小版，不依賴 React） | `web/src/viewer/` | 已實作；GLB 與元件選取未實作 |
| Pydantic → JSON Schema → TypeScript 型別管線 | `scripts/export_schema.py`、`schemas/promptcad.schema.json`、`web/src/api/generated/` | 已實作；CI 檢查漂移 |
| Tauri v2 桌面外殼 + FastAPI sidecar | `src-tauri/`、`app/sidecar.py`、`scripts/build_sidecar.py` | 已實作（Windows／MSVC）；sidecar 以 PyInstaller 打包 |
| CadDocument 2.0 規格凍結 | `docs/CAD_DSL_2.md`、`examples/cad2/`、`tests/test_cad2_examples.py` | 已凍結；程式依 ADR 0010 在第 2 階段以 Rust 實作 |
| 桌面版內含 CadQuery（第 1 階段 1b） | `app/render_worker.py`、`app/services/renderer.py`、`scripts/{desktop_env,build_sidecar,frozen_smoke}.py`、`src-tauri/` | 已實作：PyInstaller onedir sidecar 經 Tauri `resources` 安裝；凍結後以 `--render-worker` 執行 `model.py`，桌面版可輸出 STEP／STL／DXF／SVG |
| 移除 `app/static/`（負責人核准） | `app/main.py`、`Dockerfile`、`.github/workflows/ci.yml`（`docker` job） | 已完成；Dockerfile 多階段建置前端，CI 實際建置映像並確認 `/` 回傳新前端 |
| 乾淨機器驗證（隔離環境 smoke、Job Object 終止檢查） | `scripts/frozen_smoke.py --isolated-env`、`scripts/frozen_kill_check.py`、`.github/workflows/desktop-bundle.yml` | 已實作；在 GitHub `windows-latest` 上執行 |
| 第三方授權聲明 | `scripts/gen_third_party_notices.py`、`THIRD_PARTY_NOTICES.md`、`src-tauri/tauri.conf.json` | 已完成；隨安裝包安裝到 `<安裝目錄>\THIRD_PARTY_NOTICES.md` |
| 2D 預覽可讀性 | `web/src/components/SvgPreview.tsx`、`web/src/styles.css` | 已完成；`model.svg` 以紙色底顯示，`preview.svg` 不變 |
| 三平台桌面版（ADR 0011） | `src-tauri/tauri.{macos,linux}.conf.json`、`src-tauri/src/sidecar.rs`、`app/parent_watch.py`、`.github/workflows/desktop-bundle.yml` | 已實作、待 CI 驗證：Windows NSIS、macOS `.app`／`.dmg`（ad-hoc 簽章）、Linux `.deb`／AppImage；unix 以 stdin EOF + 行程群組終止 sidecar |
| 建置環境鎖版 | `scripts/desktop_lock.py`、`scripts/desktop_env.py`、`requirements/desktop-*.txt`、`ci.yml`（`lock-check`） | 已完成：三平台雜湊鎖檔，`uv pip sync --require-hashes`；以鎖檔重建的環境與 `.venv-desktop` 的 `uv pip freeze` 完全相同 |

### 第 1 階段狀態（2026-11-26）

已結案：

- **conda-pack 與 PyInstaller**：選定 PyInstaller onedir，**負責人已於 2026-11-26 簽核**（見 ADR 0006、0010 回顧紀錄）。
- **`app/static/` 已移除**（負責人核准）：未建置前端時 `/` 回傳 503 說明頁；Docker 映像以多階段建置內含前端。
- **第三方授權聲明**：`THIRD_PARTY_NOTICES.md` 由 `scripts/gen_third_party_notices.py --strict` 產生並隨安裝包發佈；LGPL 元件（OCCT／OCP、casadi、NLopt）可在 `<安裝目錄>\sidecar\_internal\` 替換。提交的檔案為 Windows 權威版，可由鎖檔重現，Windows CI 以 `--check --strict` 把關；macOS／Linux 於 CI 重新產生並隨包發佈（ADR 0011）。`build_sidecar.py` 確認未凍結的套件不再列入（Python 套件 60 → 39 個）。
- **CadQuery 投影 SVG 在深色主題下不易閱讀**：`model.svg` 改以紙色底顯示。
- **關閉外殼時中斷進行中的渲染**：新增 `scripts/frozen_kill_check.py` 實測（Job Object 關閉後 sidecar 與 render worker 須在 5 秒內結束；`--shell-exe` 另以已安裝的外殼驗證，首次執行通過後已改為必過），在 `desktop-bundle.yml` 中執行；結果併入下方的乾淨機器驗證。

尚未達成：

- **三平台測試矩陣**：已實作（ADR 0011）：`desktop-bundle.yml` 為 `windows-latest`／`macos-latest`／`ubuntu-22.04` 矩陣，`ci.yml` 的 `desktop` job 為三平台 `cargo fmt`／`clippy`／`test`。macOS／Linux 的外殼模式終止檢查、安裝後 smoke、`.dmg`、AppImage 先列為參考（`continue-on-error`），連續兩次全綠後改為必過。
  - 跨平台 desktop-bundle 執行結果：（待整合者填入跨平台 desktop-bundle 執行結果）
- **乾淨機器驗證**：已改由 GitHub `windows-latest` 上的 `desktop-bundle.yml` 執行（隔離環境 smoke、Job Object 終止檢查、安裝後檢查授權聲明存在且大於 10 KB），尚待第一次完整執行的結果。
  - desktop-bundle 執行結果：2026-11-26 於 GitHub `windows-latest`（Windows Server 2025，4 CPU）首次執行 [run 45723089074](https://github.com/stevenke1981/sicad/actions/runs/45723089074) 全數通過：建置與安裝後的 sidecar 皆在 isolated env 下通過 smoke（各觀察到 7 個 render worker，`foreign_processes=[]`）；安裝 sidecar 的 Job Object 中斷檢查 `KILL_CHECK_OK`；強制結束已安裝的 `promptcad-studio.exe` 後 sidecar 於 0.007 秒內結束（`job.rs` 實證，此步驟已改為必過）；安裝目錄含 `THIRD_PARTY_NOTICES.md`。量測：安裝檔 107.5 MiB、安裝後 554.1 MiB、READY 0.64–0.66 s、warm render 2.38 s、端到端 generate 2.42 s。
- 3D 檢視器的實際繪製尚未以畫面確認（STL 已成功取得）。
- 檢視器只支援 STL；GLB、元件選取與 port 圖層延到第 2 階段。

### 桌面版量測（2026-11-26，Windows 11）

| 項目 | 數值 |
|---|---|
| sidecar onedir 大小／檔案數 | 547.5 MiB／1,168 個 |
| NSIS 安裝包（LZMA） | 107.0 MiB |
| 安裝後大小 | 550.9 MiB |
| sidecar 啟動到 READY（第一次／暖啟動中位數） | 0.85 s／0.55 s |
| render worker，`plate-four-holes`（建置後第一次／暖中位數） | 9.65 s／2.21 s（第一次含防毒掃描新檔） |
| `POST /generate` 端到端（rule planner，4 種格式） | 約 2.2 s |

最大的組成：`vtk.libs` 264 MiB、`OCP` 90 MiB、`cadquery_ocp.libs` 62 MiB、`vtkmodules` 42 MiB、`casadi` 24 MiB。VTK 約佔 56%，是之後縮小體積最大的槓桿。

## 已驗證

v0.1.0 發行包：

- 26 個 pytest 測試通過。
- Python `compileall` 通過。
- Web JavaScript `node --check` 通過（`app/static/` 已於第 1 階段移除，此項不再適用）。
- Editable package 安裝與 `promptcad --help` 通過。
- Source-only 端到端 smoke generation 通過。

第 1 階段切片另以下列檢查驗收（見 `.github/workflows/ci.yml`）：

- `pytest`：原有 26 個測試不修改，另加 schema 匯出、前端服務、sidecar 啟動與 2.0 範例的測試。
- `ruff check .`。
- `python scripts/export_schema.py --check` 與 `npm --prefix web run gen:types` 後 `git diff --exit-code`（schema 漂移檢查）。
- `npm --prefix web run typecheck`、`test`、`build`。
- `src-tauri` 的 `cargo fmt --check`、`cargo clippy -D warnings`、`cargo test`（CI 的 `desktop` job，`windows-latest`／`macos-latest`／`ubuntu-22.04` 三平台）。
- 桌面安裝包：`scripts/frozen_smoke.py` 以凍結後的 sidecar 對五種基體 × 六種孔型實際渲染 STEP／STL／DXF／SVG（手動或 `v*` tag 觸發 `.github/workflows/desktop-bundle.yml`）。
- 前端服務：`tests/test_frontend_serving.py` 檢查建置產物的提供、路徑穿越防護，以及未建置時的 503 說明頁；CI 的 `docker` job 建置映像並確認 `/` 回傳含 `id="root"` 的新前端。
- 授權聲明：`tests/test_third_party_notices.py`（純函式、假資料，不需 CadQuery；含 unix 原生函式庫路徑的辨識與正規化）；`desktop-bundle.yml` 在 Windows 以 `--check --strict` 比對提交的檔案，在 macOS／Linux 以 `--strict` 重新產生。
- 建置環境鎖檔：`tests/test_desktop_lock.py`（純函式）；`ci.yml` 的 `lock-check` job 執行 `scripts/desktop_lock.py --check`。
- 桌面驗證腳本：`tests/test_frozen_kill_check.py` 等純函式測試在 `ci.yml` 執行；實際的隔離環境 smoke 與 Job Object 終止檢查只在 `desktop-bundle.yml` 對已安裝的 sidecar 執行。

## 封裝環境限制

建立 v0.1.0 發行包的執行環境沒有 Docker、CadQuery、OpenSCAD 或 Conda，因此沒有在該環境中實際執行 OpenCascade 生成 STEP／STL／DXF。專案的 Dockerfile 與 Conda 環境已固定 CadQuery 2.8.0。桌面版自第 1 階段 1b 起以 pip 安裝的 `cadquery==2.8.0`（`.venv-desktop`）打包，已在 Windows 上實際產生 STEP／STL／DXF；`.venv-desktop` 現由 `requirements/desktop-*.txt` 的雜湊鎖檔安裝（cadquery-ocp 7.9.3.1.1、vtk 9.6.2、casadi 3.8.2、nlopt 2.11.0、numpy 2.5.5、PyInstaller 6.22.3，三平台相同）。

## 下一階段

依 [ADR 0010](docs/adr/0010-migration-phases.md)：

1. 完成第 1 階段：取得 `desktop-bundle.yml` 三平台矩陣的執行結果，依 CI 的 `--strict` 失敗補齊 unix 原生函式庫分類，macOS／Linux 的參考項目連續兩次全綠後改為必過（ADR 0011）。
2. 第 2 階段：Rust core 實作 CadDocument 2.0（依 [`docs/CAD_DSL_2.md`](docs/CAD_DSL_2.md)）、驗證器、patch 引擎與 1.0 → 2.0 adapter；前端加入元件樹與 port 顯示。
3. 第 3 階段：約束求解器評估（[ADR 0007](docs/adr/0007-constraint-solver.md)）與多 agent 編排。

仍保留的 1.0 功能構想（將併入 2.0 的對應階段）：

- 2D 草圖 DSL：線、圓弧、約束、拉伸、旋轉與陣列（併入 2.0 的 Line／Plane 升維鏈）。
- 多孔群與 pattern／mirror。
- 正式工程圖、尺寸、公差、BOM 與標題欄。
- BREP validity、最小壁厚、製程規則與干涉檢查（2.0 的核心層驗證）。
- 非同步 renderer worker、sandbox、配額與多租戶。
- 圖片／手繪草圖／DXF 輸入。
