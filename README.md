# PromptCAD Studio

PromptCAD Studio 將中文或英文提示詞轉成**可驗證、可重現、可編輯的參數化 CAD**：

```text
提示詞
  → 本地規則／OpenAI-compatible LLM 規劃器
  → 受控 CadDocument 1.0 DSL
  → 幾何與製造前驗證
  → CadQuery／OpenSCAD 編譯器
  → STEP、STL、DXF、SVG、Python、SCAD、JSON
```

目前版本是可直接執行的 MVP，適合固定板、法蘭／墊圈、圓柱、L 型支架與開口外殼。系統不會直接執行模型產生的 Python；LLM 只能填寫有型別、尺寸上限與特徵白名單的 CAD DSL，再由確定性編譯器建立幾何。

## 下一代方向：維度階梯（Dimensional Ladder）

本專案正在從「單一模板零件」演進為「以維度原語組裝成品」的 CAD 系統：

```text
Empty（空）→ Dot（點）→ Line（線）→ Plane（面）→ Volume（體）
```

這五種原語都是元件，由 AI agent 團隊負責調整參數、接合、組裝成品；所有幾何仍由確定性編譯器產生，LLM 只輸出受 schema 限制的 patch JSON。

- 概念與技術設計：[`docs/CONCEPT_DIMENSIONAL_LADDER.md`](docs/CONCEPT_DIMENSIONAL_LADDER.md)
- 技術選型架構決策紀錄（Tauri v2、React、Three.js、Rust 核心、OpenCascade sidecar）：[`docs/adr/`](docs/adr/README.md)
- CadDocument 2.0 凍結規格與手寫範例：[`docs/CAD_DSL_2.md`](docs/CAD_DSL_2.md)、[`examples/cad2/`](examples/cad2/)

> 狀態：2.0 規格已凍結（階段 0），程式依 ADR 0010 在第 2 階段以 Rust 實作，第 2 階段 2a 的 Rust core 已實作於 `crates/promptcad-core/`（獨立 crate，`cargo test` 驗證），2b 起已由桌面版（Tauri 命令 `core_request`）與伺服器（`POST /api/v1/core`）呼叫，用於 2.0 文件的驗證與唯讀元件樹顯示；Python 編譯器仍不接受 2.0，2.0 文件目前不能輸出幾何，1.0 的生成流程與 REST API 不變。ADR 0010 第 1 階段的桌面／Web 前端切片已加入但尚未發佈（見下方「桌面版與新前端」與 [`PROJECT_STATUS.md`](PROJECT_STATUS.md)）。其餘段落描述的是目前可執行的 v0.1.0 MVP。

## 已包含

- 中文／英文提示詞本地解析，沒有 API Key 也能使用。
- OpenAI-compatible LLM 規劃器，支援 `json_schema`、`json_object` 與純提示 JSON 模式。
- 可編輯 `spec.json`：在 Web UI 修改尺寸後重新驗證與輸出。
- 基礎幾何：板件、圓柱、圓環、L 型支架、開口外殼。
- 孔特徵：通孔、盲孔、間隙孔、攻牙底孔近似、沉孔、沉頭孔，以及 X／Y／Z 軸方向。
- 邊特徵：圓角與倒角。
- 螺紋尺寸輔助：常見 M2～M12 一般間隙孔與攻牙底孔表；其他尺寸使用明確標示的比例近似。
- 驗證閘門：孔超出輪廓、孔重疊、盲孔／沉孔深度、沉頭深度、圓環內孔、邊距、壁厚、圓角與倒角。
- 無效設計會保存 DSL 與原始碼供修正，但**不會送入 CAD 核心渲染**。
- Web UI、REST API、CLI、Docker Compose、Conda 環境、測試、CI 與完整範例。
- 每次工作保存 manifest、DSL、驗證報告、CadQuery、OpenSCAD、預覽及實際輸出。

## 最快啟動：Docker

Docker 環境內含 CadQuery 2.8.0，可實際產生 STEP、STL、DXF 與 SVG。Dockerfile 為多階段建置：第一階段以 `node:24-slim` 建置 `web/` 前端，再複製到映像的 `app/web_dist/`，因此不需在主機上安裝 Node.js。另一個 `rust:1.90.0-slim-trixie` 階段建置 `promptcad-core`，複製到映像的 `/usr/local/bin/promptcad-core`；此階段尚未在本機以 Docker 驗證，待 CI 的 `docker` job。

```bash
docker compose up --build
```

瀏覽器開啟：

- Web：`http://localhost:8000`
- OpenAPI：`http://localhost:8000/docs`

不設定 `.env` 也能以本地規則解析器啟動。需要 LLM 或 API Token 時再建立設定檔：

```bash
cp .env.example .env
# Windows PowerShell: Copy-Item .env.example .env
docker compose up --build
```

停止服務：

```bash
docker compose down
```

> Linux 主機注意：容器以 `mambauser`（uid 57439）執行，綁定掛載的 `./generated` 若屬於主機使用者可能無法寫入；請先 `sudo chown 57439 generated`，或在 compose 設定 `user:`。

生成檔保存在專案的 `generated/`，重新建立容器不會消失。

## 不使用 Docker

### 完整 CAD 環境

CadQuery 的 OpenCascade 相依套件較重，建議使用 Miniforge／Mambaforge：

```bash
mamba env create -f environment.yml
conda activate promptcad
npm --prefix web ci && npm --prefix web run build   # Web UI；需要 Node.js
uvicorn app.main:app --reload --host 0.0.0.0 --port 8000
```

非 Docker 的伺服器模式必須先建置前端：`app/web_dist/index.html` 不存在時，`/` 回傳 HTTP 503 的說明頁（提示上述建置指令並連到 `/docs`），API 本身照常運作。

### 僅開發 DSL、API 與原始碼

```bash
python -m venv .venv
# Windows: .venv\Scripts\activate
# Linux/macOS: source .venv/bin/activate
python -m pip install -e ".[dev]"
pytest
uvicorn app.main:app --reload
```

未安裝 CadQuery／OpenSCAD 時，系統進入 `source_only`：仍會產生 `spec.json`、`validation.json`、`model.py`、`model.scad` 與 `preview.svg`，但不會偽造 STEP／STL／DXF。

## 桌面版與新前端（第 1 階段，未發佈）

依 [ADR 0010](docs/adr/0010-migration-phases.md) 第 1 階段，專案多了三個部分：

| 目錄 | 內容 |
|---|---|
| `web/` | React + TypeScript + Vite 前端（已取代並移除原本的 `app/static/`），另加 Three.js STL 檢視器；`npm --prefix web run build` 輸出到 `app/web_dist/` |
| `src-tauri/` | Tauri v2 桌面外殼；啟動時在 `127.0.0.1` 的動態埠啟動 FastAPI sidecar，並傳入一次性 Token |
| `schemas/` | 由 Pydantic 匯出的 JSON Schema（`scripts/export_schema.py`），前端型別由它生成，請勿手改 |

FastAPI 在 `/` 提供 `app/web_dist/` 的建置產物（Docker 與 `uvicorn` 模式皆同）。原本無建置步驟的 `app/static/` 介面已移除；`app/web_dist/index.html` 不存在時，`/` 回傳 HTTP 503 的說明頁（提示執行 `npm --prefix web run build`，並連到 `/docs`），`/api/v1/*` 不受影響。因此伺服器模式需先執行 `npm --prefix web run build`，或使用會在映像內建置前端的多階段 Dockerfile。

以 `pip install .`（非 editable）建置 wheel 時，`app/web_dist/` 以 package data 打包，**只有先建置前端，wheel 才會內含 Web UI**；否則安裝後的 `/` 也是 503 說明頁。

前端開發（需要 Node.js；開發伺服器把 `/api` 代理到 `127.0.0.1:8000`）：

```bash
uvicorn app.main:app --reload          # 另一個終端機
npm --prefix web ci
npm --prefix web run dev               # http://localhost:5173
npm --prefix web run build             # 輸出 app/web_dist/，之後由 FastAPI 提供
```

Schema 與型別（修改 `app/models/` 後執行；CI 會檢查兩者是否同步）：

```bash
python scripts/export_schema.py        # 寫入 schemas/promptcad.schema.json；--check 只檢查
npm --prefix web run gen:types         # 生成 web/src/api/generated/promptcad.d.ts
```

桌面版支援三個平台（[ADR 0011](docs/adr/0011-cross-platform-desktop.md)）：Windows x86_64（NSIS，需 Rust `stable-x86_64-pc-windows-msvc` 與 WebView2）、macOS arm64（`.app`／`.dmg`，macOS 11+）、Linux x86_64（`.deb`／AppImage，glibc 2.35+，即 Ubuntu 22.04／Debian 12 以上，需 WebKitGTK 4.1）。`rust-toolchain.toml` 固定 Windows 的 MSVC 工具鏈；在 macOS／Linux 建置時請先設定 `RUSTUP_TOOLCHAIN=stable`。Linux 建置另需 `libwebkit2gtk-4.1-dev libappindicator3-dev librsvg2-dev patchelf libxdo-dev libssl-dev`。

```bash
npm ci                                 # 安裝根目錄的 @tauri-apps/cli
npm --prefix web run build             # 只有 tauri build 需要；cargo check／test 不需要（tauri build 的 beforeBuildCommand 也會自動建置）
# 開發：以本機 Python 取代打包的 sidecar
#   （僅 debug 建置有效）PowerShell: $env:PROMPTCAD_SIDECAR_CMD = "$PWD\.venv\Scripts\python.exe -m app.sidecar"
npm run tauri dev
# 發佈建置：以鎖檔建立含 CadQuery 的建置環境，凍結 sidecar，驗證後建置安裝包
# （以下以 Windows 路徑示範；macOS／Linux 把 .venv-desktop/Scripts/python.exe 換成 .venv-desktop/bin/python）
python scripts/desktop_env.py                                   # 依本機平台的 requirements/desktop-*.txt 建立 .venv-desktop（--recreate 重建）
.venv-desktop/Scripts/python.exe scripts/build_sidecar.py       # 輸出 src-tauri/binaries/promptcad-sidecar/（onedir，含 VTK 精簡；--no-prune 關閉）
.venv-desktop/Scripts/python.exe scripts/frozen_smoke.py --measure  # 以凍結後的 exe 實際渲染並量測（--isolated-env 模擬乾淨機器）
.venv-desktop/Scripts/python.exe scripts/frozen_kill_check.py --exe src-tauri/binaries/promptcad-sidecar/promptcad-sidecar.exe  # 關閉外殼時 sidecar 與 render worker 一併結束（Windows：Job Object；unix：stdin EOF + 行程群組）
.venv-desktop/Scripts/python.exe scripts/gen_third_party_notices.py --python .venv-desktop/Scripts/python.exe --strict  # 更新 THIRD_PARTY_NOTICES.md（Windows 版才提交）
npm run tauri build                                             # Windows：NSIS；macOS：.app + .dmg；Linux：.deb + AppImage
```

#### 建置環境鎖檔

`.venv-desktop` 的每個套件（含雜湊）都固定在三份鎖檔中：`requirements/desktop-windows-x86_64.txt`、`desktop-macos-arm64.txt`、`desktop-linux-x86_64.txt`。三份都由 `scripts/desktop_lock.py` 在**同一台機器**上以 `uv pip compile --python-platform` 產生（需要 uv），`scripts/desktop_env.py` 則以 `uv pip sync --require-hashes` 讓環境與鎖檔完全一致。

```bash
python scripts/desktop_lock.py            # pyproject.toml 改了之後：重新編譯，沿用既有版本
python scripts/desktop_lock.py --check    # 只比對（CI 的 lock-check job）
python scripts/desktop_lock.py --upgrade  # 升級全部版本：三份一起更新
python scripts/desktop_env.py --recreate  # 依新鎖檔重建 .venv-desktop
# 之後重新產生 THIRD_PARTY_NOTICES.md，與三份鎖檔一起提交
```

Makefile 也提供 `make desktop-lock`、`desktop-lock-check`、`desktop-env`、`notices`、`notices-check`。

#### 安裝未簽章的測試版

macOS 與 Linux 的安裝包未經 Apple 公證或簽章（macOS 只有 ad-hoc 簽章），僅供測試：

- **macOS**：把 `PromptCAD Studio.app` 拖到「應用程式」後，Gatekeeper 會因隔離屬性拒絕開啟從網路下載的 App。確認來源可信後執行：

  ```bash
  xattr -dr com.apple.quarantine "/Applications/PromptCAD Studio.app"
  ```

- **Linux（.deb）**：以 apt 安裝本機檔案，系統相依（`deb.depends` 列出的 GL／X11 函式庫與 WebKitGTK）會一併安裝：

  ```bash
  sudo apt install ./PromptCAD*.deb       # 注意 ./ 前綴；之後以 promptcad-studio 或應用程式選單啟動
  sudo apt remove <套件名>                 # 套件名可由 dpkg-deb -f PromptCAD*.deb Package 查詢
  ```

  AppImage 目前為參考產物：`chmod +x` 後直接執行（需要 FUSE 2）。

桌面版內含 CadQuery 2.8.0，可直接輸出 STEP／STL／DXF／SVG（凍結後以 `promptcad-sidecar --render-worker` 執行 `model.py`）。Windows 安裝包約 107 MiB，精簡前安裝後約 551 MiB；其餘量測見 [`PROJECT_STATUS.md`](PROJECT_STATUS.md)。乾淨機器驗證在 GitHub hosted runner（`windows-latest`、`macos-latest`、`ubuntu-22.04`）上以手動或 `v*` tag 觸發的 `.github/workflows/desktop-bundle.yml` 執行（鎖檔安裝、onedir smoke、終止檢查、安裝後檢查；各項在各平台的必過／參考分級見 ADR 0011）。

### 第三方授權聲明

桌面安裝包隨附的第三方軟體（凍結進 sidecar 的 Python 套件與 wheel 隨附 DLL、前端 npm 正式相依、Tauri 外殼的 Rust crates，以及 OCCT、VTK、CPython、PyInstaller bootloader、MSVC runtime、WebView2 bootstrapper 等）列於根目錄的 [`THIRD_PARTY_NOTICES.md`](THIRD_PARTY_NOTICES.md)，安裝後位於 `<安裝目錄>\THIRD_PARTY_NOTICES.md`。其中 OCCT（經 OCP 載入）、casadi 與 NLopt 為 LGPL，在 onedir 版面下以獨立的 `.dll`／`.pyd` 存放於 `<安裝目錄>\sidecar\_internal\`，可由使用者替換；原始碼網址與版本見該檔的「LGPL 元件與替換權利」段落。

- 提交在版本庫中的 `THIRD_PARTY_NOTICES.md` 是 **Windows 版的權威檔**：`.venv-desktop` 由鎖檔建立，因此可重現，`desktop-bundle.yml` 在 Windows 上以 `--check --strict` 比對，不一致即失敗。macOS／Linux 的安裝包各自在 CI 以 `--strict` 重新產生一份（路徑與原生函式庫依平台而異，並移除只屬於 Windows 的元件），隨安裝包發佈並上傳為 artifact，但不提交。
- 若日後某個相依套件帶來未對應的原生函式庫（`.dll`／`.so`／`.dylib`）或授權，`--strict` 會讓打包失敗——這是刻意的警報，請補上 `LICENSE_OVERRIDES`／`VENDORED_DLLS` 後重跑。`scripts/build_sidecar.py` 的 `PRUNED_DISTS`（確認未凍結的套件）不列入聲明。
- 公開發布前，請在安裝檔旁附上（或封存連結）LGPL 元件對應版本的原始碼（OCP/OCCT、casadi、NLopt）。本文件不是法律意見。

這個檔案由 `scripts/gen_third_party_notices.py` 產生，請勿手改：相依套件變動後以 `.venv-desktop` 重新執行（見上方指令；`--check` 只比對、不寫入）。`--strict` 在授權不明、或 wheel 隨附了未登錄的 DLL 時失敗；metadata 缺漏或有誤的套件在腳本的 `LICENSE_OVERRIDES` 中逐項註明理由。此檔是授權資訊彙整，不是法律意見。

## 使用方式

### Web 編輯流程

1. 輸入「長 120、寬 60、厚 10，四角 M6 通孔，R5」。
2. 系統產生 CAD DSL、驗證報告與預覽。
3. 展開「可編輯 CAD DSL / JSON」，修改長寬、孔徑、孔位或特徵。
4. 按「套用 JSON 並重新輸出」。
5. 下載個別檔案或工作 ZIP。

### CLI

```bash
promptcad generate "畫一個長120、寬60、厚10的固定板，四角M6孔，R5" --planner rule
promptcad validate examples/generated/plate-four-holes/spec.json
promptcad render examples/generated/plate-four-holes/spec.json
promptcad doctor
```

只輸出 DSL 與原始碼：

```bash
promptcad generate "外徑30、內徑15、厚5的墊圈" --no-render
```

### REST API

提示詞產生：

```bash
curl -X POST http://localhost:8000/api/v1/generate \
  -H "Content-Type: application/json" \
  -d '{
    "prompt": "鋁合金固定座，長120mm、寬60mm、厚10mm，四角 M6 通孔，R5",
    "planner": "rule",
    "formats": ["step", "stl", "dxf", "svg", "py", "scad", "json"],
    "render": true
  }'
```

修改後的 DSL 重新輸出：

```bash
curl -X POST http://localhost:8000/api/v1/generate-from-spec \
  -H "Content-Type: application/json" \
  --data-binary @request-with-spec.json
```

完整端點與狀態語意見 [`docs/API.md`](docs/API.md)。

## LLM 設定

`.env` 範例：

```dotenv
PROMPTCAD_PLANNER_MODE=auto
PROMPTCAD_LLM_BASE_URL=https://api.openai.com/v1
PROMPTCAD_LLM_API_KEY=你的金鑰
PROMPTCAD_LLM_MODEL=gpt-5.6
PROMPTCAD_LLM_STRUCTURED_MODE=json_schema
```

OpenAI-compatible 本地端點，例如 llama.cpp server：

```dotenv
PROMPTCAD_LLM_BASE_URL=http://host.docker.internal:8080/v1
PROMPTCAD_LLM_API_KEY=local
PROMPTCAD_LLM_MODEL=qwen3-coder
PROMPTCAD_LLM_STRUCTURED_MODE=json_object
```

`auto` 會在 LLM 未設定時使用本地規則；LLM 呼叫失敗時，預設也會退回本地解析器。不同供應商對 Structured Outputs 的支援不一致，必要時將模式改成 `json_object` 或 `prompt_only`。

## API 保護

設定：

```dotenv
PROMPTCAD_API_TOKEN=請換成長且隨機的字串
```

之後所有 `/api/v1/*` 端點都要帶以下其中一種標頭：

```http
Authorization: Bearer YOUR_TOKEN
X-API-Key: YOUR_TOKEN
```

Web UI 的 Token 欄位會連同預覽、單檔下載及 ZIP 下載一起使用。

## 支援的提示詞

```text
畫一個鋁合金固定板，長120、寬60、厚10，四角 M6 通孔，孔中心離邊10，四角 R5。
做一個長80、寬40、厚10的板，中央一個直徑5、深6的盲孔。
做一個墊圈，外徑30、內徑15、厚5。
做一個 L 型支架，寬80、深50、高60、厚4，立板兩個 M5 孔。
Create a 100x60x10 mm plate with two 5 mm blind holes 6 mm deep.
做一個 100×70×30 mm 的開口盒，壁厚2mm，底部四角3mm通孔。
```

更穩定的寫法、軸向與孔位格式見 [`docs/PROMPT_GUIDE.md`](docs/PROMPT_GUIDE.md)。

## 輸出檔

| 檔案 | 用途 |
|---|---|
| `spec.json` | 可編輯、可版本控制的 CAD DSL |
| `validation.json` | errors、warnings、info 與 review 狀態 |
| `model.py` | 可獨立執行的 CadQuery 模型 |
| `model.scad` | OpenSCAD fallback 模型 |
| `preview.svg` | 不依賴 CAD 核心的快速工程預覽 |
| `model.step` | 實體 CAD 交換檔，需要 CadQuery |
| `model.stl` | 3D 列印網格，需要 CadQuery 或 OpenSCAD |
| `model.dxf` | 水平截面 2D DXF，需要 CadQuery |
| `model.svg` | CadQuery 投影 SVG |
| `manifest.json` | 工作狀態、產物與 renderer provenance |

## 重要限制

1. **M6、M8 等標示不等於完整實體螺紋。** 目前建立攻牙底孔／間隙孔的圓柱幾何，螺紋規格保留在 DSL 與驗證資訊中。
2. DXF 是零件水平截面，不是含尺寸、公差、基準、表面處理與圖框的正式工程圖。
3. 規則解析器偏向常見單一零件與單一孔群；複雜多段輪廓、多孔群、裝配與自由曲面應使用 LLM 規劃器或直接編輯 DSL。
4. `preview.svg` 是快速預覽，不是 CAD kernel 的 BREP 渲染結果。
5. AI 推論與預設值都會要求 review；投入 CNC、雷切、射出或承載用途前，必須由合格工程人員覆核材料、公差、強度與製程。

## 測試與品質檢查

```bash
pytest
ruff check .
python -m compileall -q app scripts tests
python scripts/smoke_test.py
python scripts/export_schema.py --check
npm --prefix web run typecheck
npm --prefix web test
```

目前測試涵蓋規則解析、M 制孔徑、盲孔、沉頭孔、X/Y/Z 軸幾何原始碼、驗證閘門、可編輯 DSL、API Token、檔案下載與 ZIP；第 1 階段另加 schema 匯出、新前端服務（含未建置時的 503 說明頁）、sidecar 啟動、凍結 smoke／終止檢查、鎖檔與授權聲明產生器的純函式測試、2.0 範例的紙上驗證，以及前端的 vitest 測試。CI 另有 `docker` job 實際建置映像並確認 `/` 提供新前端、`lock-check` job 確認三份桌面鎖檔與 `pyproject.toml` 一致，以及三平台的 `desktop` job（`cargo fmt`／`clippy`／`test`）。第 2 階段 2b 另有：Rust core 303 項測試、`src-tauri` 23 項測試、伺服器整合測試（對實際的 `promptcad-core` 執行，含 49 個 parity 案例）與前端 vitest 52 項；這些數字為撰寫時的本機結果。

## 專案結構

```text
app/
  api/                 REST 路由
  core/                設定、Token 與路徑安全
  models/              CadDocument DSL 與 API 模型
  services/
    planners/          本地規則與 OpenAI-compatible LLM
    compiler.py        CadQuery 確定性編譯器
    openscad.py        OpenSCAD 編譯器
    renderer.py        CAD 核心選擇與受限 subprocess
    validator.py       幾何／製造前驗證閘門
  web_dist/            web/ 的建置輸出（不納入版本控制；不存在時 "/" 回傳 503 說明頁）
  sidecar.py           桌面版 sidecar 進入點（印出 READY 行後提供 API）
web/                   React + TypeScript + Vite 前端與 Three.js 檢視器
src-tauri/             Tauri v2 桌面外殼（Rust）
crates/promptcad-core/ CadDocument 2.0 的 Rust core（獨立 crate；由 Tauri 命令與 `POST /api/v1/core` 呼叫；`cd crates/promptcad-core && cargo test`）
schemas/               JSON Schema（生成物）：根目錄的 promptcad.schema.json 由 Pydantic 匯出（1.0），cad2/ 由 Rust core 匯出（2.0）
docs/                  API、架構、DSL（1.0 與 2.0）、提示詞、安全說明與 ADR
examples/generated/    可直接修改及重新輸出的完整範例（1.0）
examples/cad2/         CadDocument 2.0 手寫範例（規格驗證用；`crates/promptcad-core/` 的 Rust core 與其宿主接受 2.0，Python 編譯器不接受）
scripts/               doctor、smoke test、export_schema.py、desktop_lock.py、desktop_env.py、
                       build_sidecar.py、frozen_smoke.py、frozen_kill_check.py、
                       gen_third_party_notices.py
requirements/          桌面建置環境的三平台雜湊鎖檔（desktop-*.txt，由 desktop_lock.py 產生）
tests/                 單元、API、安全回歸、schema、sidecar、授權聲明與 2.0 範例測試
generated/             執行時工作目錄
THIRD_PARTY_NOTICES.md 桌面安裝包的第三方授權聲明（生成物，隨安裝包發佈）
```

詳細資料：

- [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md)
- [`docs/CAD_DSL.md`](docs/CAD_DSL.md)、[`docs/CAD_DSL_2.md`](docs/CAD_DSL_2.md)
- [`docs/API.md`](docs/API.md)
- [`docs/SECURITY.md`](docs/SECURITY.md)
- [`PROJECT_STATUS.md`](PROJECT_STATUS.md)
