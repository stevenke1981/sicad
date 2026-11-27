# REST API

基底路徑為 `/api/v1`。設定 `PROMPTCAD_API_TOKEN` 後，所有本章端點都需要 Bearer Token 或 `X-API-Key`。

## 狀態語意

- `completed`：請求的 CAD 核心輸出已執行完成。
- `source_only`：沒有可用 CAD 核心、指定 `render=false`，或設定為 source-only；DSL、原始碼與預覽仍可用。
- `failed`：設計驗證失敗或 renderer 發生不可恢復錯誤。
- `renderer_used=validation-blocked`：DSL 可解析，但存在幾何 error，因此沒有執行 CadQuery／OpenSCAD。

## `GET /health`

```json
{"status": "ok", "version": "0.1.0"}
```

## `GET /capabilities`

回傳規劃器、特徵、格式、CadQuery／OpenSCAD 可用狀態及目前設定。

另含 Rust core 的狀態（皆為選填欄位，舊用戶端可忽略）：`core_available`（布林，預設 `false`）、
`core_crate_version`、`core_api_version`、`core_schema_versions`（例如 `["1.0", "2.0"]`）。找不到 core
執行檔時 `core_available` 為 `false`，其餘為 `null`／空陣列。

## `POST /plan`

只產生與驗證 CAD DSL，不建立工作目錄，也不啟動幾何核心。

```json
{
  "prompt": "做一個外徑30、內徑15、厚5的墊圈",
  "planner": "rule"
}
```

`planner` 可為 `auto`、`rule`、`llm`。

## `POST /generate`

由提示詞建立 DSL、驗證、原始碼、預覽及可用的 CAD 輸出。

```json
{
  "prompt": "長120寬60厚10的固定板，四角M6孔，R5",
  "planner": "auto",
  "formats": ["step", "stl", "dxf", "svg", "py", "scad", "json"],
  "render": true
}
```

## `POST /generate-from-spec`

將 Web 或版本控制中修改過的 `CadDocument` 重新驗證及輸出。`spec` 必須是完整 DSL；未知欄位會被拒絕。

```json
{
  "spec": {
    "schema_version": "1.0",
    "name": "edited-plate",
    "source_prompt": "manual edit",
    "unit": "mm",
    "material": "aluminum",
    "base": {"kind": "plate", "length": 140, "width": 60, "thickness": 10},
    "holes": [],
    "fillets": [],
    "chamfers": [],
    "assumptions": [],
    "notes": [],
    "planner": {"planner": "manual", "confidence": 1, "review_required": false}
  },
  "formats": ["step", "stl", "dxf", "svg", "py", "scad", "json"],
  "render": true
}
```

## `POST /generate-from-document`（第 2 階段 2d）

將 CadDocument 2.0 經 Rust core 的 `validate` 驗證後輸出幾何。請求本文為完整的 2.0 文件（`document`）、`formats` 與 `render`。

```json
{
  "document": {"schema_version": "2.0", "...": "..."},
  "formats": ["step", "stl", "dxf", "svg"],
  "render": true
}
```

| HTTP | 情況 |
|---|---|
| `200`，`completed`／`source_only` | 工作成功。`render=true` 且有 CadQuery 時為 `completed`，否則 `source_only`。產物：`spec.json`（core 正規化的文件）、`validation.json`、`resolved.json`、`model.py`；有 CadQuery 時另有 STEP、STL、DXF、SVG。不產生 `model.scad` 與 `preview.svg` |
| `200`，`failed`，`renderer_used="validation-blocked"` | core 驗證報告不合法。不產生 `model.py`，但仍寫入 `resolved.json` |
| `200`，`failed`，`renderer_used="compile-unsupported"` | 文件含不支援的構造。`error` 列出每個 id 與原因（例如 `extrude`、`revolve`、Empty 的 `of`、旋轉零件上的圓角） |
| `422` | core 拒絕文件（`detail` 含 core 的代碼，例如 `invalid_document`，與路徑）；或 `schema_version` 不是 `"2.0"`（此時不呼叫 core） |
| `503` | core 不可用，或 core 忙碌（附 `Retry-After`） |
| `504` | core 逾時 |
| `502` | core 行程失敗 |

其他行為：

- `planner_used` 為 `manual-dsl`，`prompt` 為文件的 `source_prompt`。
- 2.0 不回退到 OpenSCAD。無 CadQuery 時為 `source_only`，並附固定警告。
- 本端點不移除 `expected_resolved`（僅範例使用的欄位）；用戶端必須自行移除。CLI 會移除。
- 工作 manifest 新增 `spec_version`（`"1.0"`／`"2.0"`，預設 `"1.0"`）與 `document`（2.0 時為 core 正規化的文件）；`spec` 改為可為 null。舊的 manifest 仍可讀取。
- 支援與不支援的構造見 `PROJECT_STATUS.md` 的「第 2 階段 2d 切片」。

## `POST /validate`

Body 是完整 `CadDocument`。回傳：

```json
{
  "valid": true,
  "review_required": false,
  "issues": []
}
```

`error` 會阻止 renderer；`warning` 要求人工覆核；`info` 用於假設與螺紋近似資訊。

## 1.0 驗證的執行位置（第 2 階段 2c）

`/plan`、`/generate`、`/generate-from-spec`、`/validate`，以及 CLI 的 `promptcad validate|render|generate` 與桌面 sidecar，所有 1.0 驗證都經 Rust core 的 `validate` 操作執行（`app/services/core_validator.py`）。Python 端不再含任何設計規則；請求本文仍先經 1.0 Pydantic 模型解析（`422` 的情況不變）。

這些端點需要 core 執行檔（`make core` 或 `PROMPTCAD_CORE_BIN`，見下方「`POST /core`」的設定）。傳輸層錯誤回應如下，與 `POST /core` 相同的代碼定義：

| HTTP | 情況 |
|---|---|
| `503` | 找不到 core 執行檔（`detail` 提示執行 `make core` 或設定 `PROMPTCAD_CORE_BIN`），或 core 忙碌（附 `Retry-After`） |
| `504` | core 逾時 |
| `502` | core 行程失敗；或 Pydantic 合法的 1.0 文件經 core 回 `status: error`（`detail` 含 core 的代碼與路徑） |

CLI 遇到 core 傳輸錯誤時以結束碼 `3` 結束。

### 2c 的 1.0 回應變更（已接受）

- issue 的 `message` 改為 core 的英文 id 模板（例如 `hole e_h1 …`）；`assumption` 的訊息不再回顯假設文字。
- issue 新增 `target`、`related`、`owner`、`layer` 欄位。舊的 manifest 仍可讀取。
- issue 的順序改變；不應依賴順序。
- 2.0 專屬代碼可能出現在 1.0 流程（例如 `empty_cut_no_effect`）。
- 無效的螺紋字串回報 `tapped_hole_without_thread`，取代 `thread_annotation_only`。
- `feature_index` 對 1.0 文件有值：孔相關 issue 為 0-based 孔索引，`fillet_too_large`、`chamfer_too_large` 為對應清單的索引；其餘為 `null`。

## `POST /core`

CadDocument 2.0 的唯一入口：把請求原封不動轉交給 Rust core（`promptcad-core rpc`，每次呼叫啟動一次子行程），
再把 core 的回應原封不動回傳。伺服器**不**解讀、驗證或重新序列化 2.0 內容（ADR 0008 規則 4）；
`validate`、`solve`、`normalize`、`upgrade`、`downgrade`、`apply_patch`、`describe`、`version` 等 `op`
的欄位與回應內容請見 [`crates/promptcad-core/README.md`](../crates/promptcad-core/README.md)。
本端點與 `/validate`、`/generate-from-spec`、`/generate-from-document` 無關。`/validate` 與 `/generate-from-spec` 只接受 1.0 文件；2.0 文件的生成改用 `/generate-from-document`（見下節）。

請求：JSON 物件（以 `op` 標記），例如 `{"op": "version"}`。文件中的範例專用鍵 `expected_resolved` 需由呼叫端先移除。

```bash
curl -X POST http://localhost:8000/api/v1/core -H "Content-Type: application/json" -H "Authorization: Bearer YOUR_TOKEN" -d '{"op": "version"}'
```

```json
{"status":"ok","result":{"op":"version","crate_version":"0.1.0","api_version":1,"schema_versions":["1.0","2.0"]}}
```

回應是 core 的信封：`status: "ok"` 帶 `result`；`status: "error"` 帶 `code`、`message`、`path`、`violations`。
**領域錯誤（`status: "error"`）與成功一樣回 HTTP `200`**，讓前端對 Tauri 與 HTTP 兩種傳輸使用同一個解碼器。

| HTTP | 意義 |
|---|---|
| `200` | core 已回應（`status` 為 `ok` 或 `error`） |
| `401` | Token 缺少或錯誤 |
| `413` | 請求本文超過 `PROMPTCAD_CORE_MAX_REQUEST_BYTES`（上限 8 MiB，與 core 相同），不會啟動 core |
| `422` | 本文第一個非空白字元不是 `{`（不會啟動 core）。注意：以 `{` 開頭但格式錯誤的本文**不再**回 422，伺服器不解析本文，由 core 回 HTTP `200` 與 `bad_request` 錯誤信封 |
| `502` | core 異常結束、輸出超過上限或無法解析（`detail` 只含截斷後的 stderr 尾端） |
| `503` | 找不到或無法啟動 core 執行檔；或 core 忙碌（`PROMPTCAD_CORE_QUEUE_WAIT_SECONDS` 內拿不到執行名額，或等待佇列已滿），此時帶 `Retry-After` 標頭且不會啟動 core |
| `504` | core 超過 `PROMPTCAD_CORE_TIMEOUT_SECONDS`（從行程啟動起算），行程已被終止 |

設定：

- `PROMPTCAD_CORE_BIN`：`promptcad-core` 執行檔路徑。未設定時依序尋找：目前執行檔旁（或 PyInstaller bundle 內）、
  `PATH`（只搜尋絕對路徑目錄，不含目前工作目錄；Windows 只接受 `.exe`，不接受 `.cmd`／`.bat`）、
  `crates/promptcad-core/target/{release,debug}`（僅限原始碼 checkout，`PROMPTCAD_ENV=production` 或凍結的 bundle 不使用）。
  設定後必須指向既有檔案，否則視為 core 不可用（`/capabilities` 的 `core_available` 為 `false`），不會退回其他位置。Docker 映像已內建並設定為 `/usr/local/bin/promptcad-core`；
  本機以 `make core` 建置。桌面版的 sidecar 內含 core（`<onedir>/_internal/`，由 `scripts/build_sidecar.py` 複製），桌面前端另可經 Tauri 命令呼叫 core。
- `PROMPTCAD_CORE_TIMEOUT_SECONDS`（預設 `15`）、`PROMPTCAD_CORE_MAX_REQUEST_BYTES`（預設 `8388608`）。
- `PROMPTCAD_CORE_MAX_CONCURRENCY`（預設 `4`，1–64）：同時執行的 core 子行程上限，使用專用執行緒池，不佔用 render 工作的預設池。
  最多再有 4 倍此數量的請求可等待；等待超過 `PROMPTCAD_CORE_QUEUE_WAIT_SECONDS`（預設 `3`，秒）即回 `503` + `Retry-After`。
  客戶端中斷連線時，對應的 core 子行程會被終止並釋放名額；伺服器關閉時執行中的子行程直接被終止。
- `/capabilities` 的 core 探測有獨立的短逾時（5 秒），失敗結果快取 30 秒，不會因 core 而阻塞或失敗。
- 單一回應上限：core 自身 24 MiB（超過時回 `error` 信封，代碼 `response_too_large`）；橋接層另有略高的第二道上限（28 MiB，超過回 `502`）。

## 工作與下載

- `GET /jobs`：列出最近工作。
- `GET /jobs/{job_id}`：取得 manifest。
- `GET /jobs/{job_id}/files/{filename}`：下載單一輸出。
- `GET /jobs/{job_id}/bundle.zip`：下載工作目錄中的所有輸出。

`job_id` 必須是系統產生的 32 位十六進位 ID；檔名只接受安全白名單字元，且解析後必須直接位於該工作目錄。

## 認證

```http
Authorization: Bearer YOUR_TOKEN
```

或：

```http
X-API-Key: YOUR_TOKEN
```

範例：

```bash
curl http://localhost:8000/api/v1/capabilities \
  -H "Authorization: Bearer YOUR_TOKEN"
```

## 常見錯誤

- `401`：Token 缺少或錯誤。
- `404`：工作或檔案不存在，或識別碼／檔名不符合安全格式。
- `413`：僅 `POST /core`，見該節。
- `502`、`503`、`504`：`POST /core`、`POST /generate-from-document`，以及所有 1.0 驗證端點（見「1.0 驗證的執行位置」）。
- `422`：Prompt、DSL、格式或 LLM 規劃結果不符合 schema。
- HTTP `200` 且 manifest `status=failed`：工作已被記錄，但驗證閘門阻止渲染；請查看 `validation.issues`。
