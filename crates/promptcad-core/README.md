# promptcad-core

PromptCAD 的 Rust 核心（[ADR 0005](../../docs/adr/0005-rust-core-document-model.md)）。負責
CadDocument 2.0 文件模型、驗證、solver 前處理、1.0 遷移，以及 patch 與 op log 引擎。規格以
[`docs/CAD_DSL_2.md`](../../docs/CAD_DSL_2.md) 為準（正規化規格）。

- library crate：`promptcad_core`，不做任何 I/O；讀寫檔案由宿主負責。
- binary：`promptcad-core`，只使用 std 的小型 CLI，提供 `schema`、`validate`、`solve`、`upgrade`、`downgrade`、`normalize`、`apply-patch`、`rpc` 子命令（見下方「CLI 用法」）。
- 與傳輸無關的請求／回應契約：`promptcad_core::api`（見下方「`api` 模組與 `rpc`」）。Tauri 桌面端在行程內呼叫
  `api::handle`，伺服器模式由 FastAPI 每次呼叫啟動一次 `promptcad-core rpc`，CLI 子命令也走同一個 dispatcher。
- JSON Schema 以 Rust 型別為唯一事實來源（[ADR 0008](../../docs/adr/0008-single-schema-source-of-truth.md)），
  產物提交在 [`../../schemas/cad2/`](../../schemas/cad2/)，CI 會比對是否過期。

## 模組

標註「Phase 2a 填寫中」的模組，目前仍只有檔頭說明或部分實作，會在 Phase 2a 逐步補齊。

| 模組 | 用途 |
|---|---|
| `api` | 與傳輸無關的請求／回應契約：`CoreRequest`、`CoreResponse`、`handle`、`handle_json` |
| `model` | 文件模型：CadDocument 2.0 / 1.0、patch、驗證 issue、solver 輸出（已完成） |
| `fields` | 以 `id:field` 讀寫數值欄位（spec §1.3、§3.3、§4） |
| `canonical` | 正規化 JSON 序列化與範例檔處理（spec §1.4、§11） |
| `expr` | 封閉的運算式文法：遞迴下降解析與求值（Phase 2a 填寫中） |
| `bindings` | 依相依順序求值 binding（spec §4.2、§4.4）（Phase 2a 填寫中） |
| `ports` | 固定的 port 表與 port 幾何（spec §3.4）（Phase 2a 填寫中） |
| `solver` | 方向與固定點平移 solver、hole 方向、`resolved.json`（spec §5.4、§5.5、§7）（Phase 2a 填寫中） |
| `migrate` | 1.0 ↔ 2.0 遷移（spec §10）（Phase 2a 填寫中） |
| `validate` | 解析式驗證器，輸出 spec §9.2 的 issue 代碼（Phase 2a 填寫中） |
| `patch_engine` | 原子 patch 套用、權限與拒絕原因（spec §8.4–§8.7）（Phase 2a 填寫中） |
| `oplog` | 只能附加的 `oplog.jsonl` 紀錄（ADR 0005）（Phase 2a 填寫中） |

## 建置、測試與重新產生 schema

在 `crates/promptcad-core` 目錄下執行。

```text
cargo build                 # 建置 library 與 promptcad-core 執行檔
cargo test                  # 單元測試、屬性測試與 CLI 整合測試
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

修改 `model` 下的型別後，重新產生 schema 並一併提交：

```text
cargo run -- schema --out-dir ../../schemas/cad2
```

只檢查、不寫檔（與 CI 相同）：

```text
cargo run --quiet -- schema --out-dir ../../schemas/cad2 --check
```

`--check` 有任何檔案不同或缺少時以結束碼 1 退出並列出檔名，全部相符時為 0。輸出為確定性的
JSON（排序鍵、2 格縮排、LF 換行、結尾換行）。

CLI 用法：

```text
usage: promptcad-core <command> [options]

commands:
  schema --out-dir <dir> [--check]
                    write the JSON Schemas (ADR 0008) into <dir>; with --check,
                    write nothing and exit 1 if any file there differs or is missing
  validate <spec.json> [--draft] [--strip-example-keys] [--out <file>]
                    print the ValidationReport of a 1.0 or 2.0 document (exit 1 if invalid)
  solve <spec.json> [--strip-example-keys] [--out <file>]
                    print resolved.json of a 1.0 or 2.0 document (exit 1 on solver errors)
  upgrade <spec-1.0.json> [--out <file>]
                    print the canonical CadDocument 2.0 JSON of a 1.0 document
  downgrade <spec-2.0.json> [--strip-example-keys] [--out <file>]
                    print the CadDocument 1.0 JSON (exit 1 if not expressible in 1.0)
  normalize <spec-2.0.json> [--strip-example-keys] [--out <file>]
                    recompute bindings and print the canonical 2.0 JSON (exit 1 on binding errors)
  apply-patch <spec-2.0.json> <patch.json> [--human] [--out <file>]
                    apply a patch and print the new canonical document (exit 1 and the reject
                    reason as JSON if the patch is rejected)
  rpc               read ONE JSON request (promptcad_core::api) from stdin and write ONE
                    single-line JSON response to stdout; exit 0 whenever a response was
                    written (also for "status": "error"), 1 if stdout cannot be
                    written, 2 on usage errors or unreadable stdin
  help              print this message

options:
  -h, --help        print this message
  -V, --version     print the version

A path of `-` reads from stdin. Exit codes: 0 success, 1 a domain failure (invalid,
unsolvable, rejected or not expressible), 2 usage or unreadable / unparsable input.
```

各子命令的結束碼：

- `schema`：0 成功；1 `--check` 發現過期或缺少的檔案，或寫檔失敗；2 用法錯誤。
- `validate`：0 報告有效；1 報告含 error；2 用法錯誤、讀檔失敗或 JSON 無法解析。`--draft` 將
  `completed` 設為 false；`--strip-example-keys` 先移除範例專用的 `expected_resolved`。
- `solve`：0 成功；1 solver 結果含 error 等級的 issue（結果仍會輸出）；2 輸入錯誤。
- `upgrade`：0 成功；2 輸入不是可解析的 1.0 文件。
- `downgrade`：0 成功；1 文件無法以 1.0 表示（單行說明寫到 stderr）；2 輸入錯誤。
- `normalize`：0 成功；1 重算 binding 時回報錯誤（issue 以 JSON 寫到 stderr）；2 輸入錯誤。
- `apply-patch`：0 patch 被接受並輸出新文件；1 patch 被拒絕（拒絕原因與 issue 以 JSON 輸出）；
  2 用法錯誤或輸入錯誤。`--human` 以人類身分套用，預設以 patch 的 `agent` 身分套用。
- `rpc`：只要寫出了回應就是 0（包含 `"status": "error"` 的回應）；1 無法寫入 stdout；2 用法錯誤
  （多餘的參數）或無法讀取 stdin。

`validate`、`solve`、`upgrade`、`downgrade`、`apply-patch` 內部都改為呼叫 `api::handle`，輸出與結束碼
不變。`normalize` 子命令只接受 2.0、只重算 binding（不重算 `root`），與 API 的 `normalize` op 不同；
`apply-patch` 仍先檢查 patch 的 schema（違反時結束碼 2），API 則把它當成 `schema_invalid` 拒絕。

路徑為 `-` 時從 stdin 讀取。`--out <file>` 會把結果寫入檔案而非 stdout。所有 JSON 輸出皆為
確定性、2 格縮排、LF 換行並以換行結尾。未知子命令回傳 2。

## `api` 模組與 `rpc`

`promptcad_core::api` 是三種宿主共用的唯一契約（ADR 0008 規則 1：請求／回應訊息也由 Rust 型別產生）。

- `pub fn handle(request: CoreRequest) -> CoreResponse`：純函式、確定性；內部以 `catch_unwind` 兜底，
  萬一 panic 會回傳 `internal` 錯誤而不是中止。不檢查請求大小。**注意**：`catch_unwind` 只在
  `panic = "unwind"` 的建置有效（`rpc`、CLI、測試都是）；Tauri 桌面版的 release profile 設了
  `panic = "abort"`，在那裡 core 的 panic 會直接結束整個桌面程式，不會變成 `internal` 回應。
- `pub fn handle_json(input: &str) -> String`：解析請求並回傳**單行、緊湊**的 JSON（無結尾換行），
  鍵依 struct 順序、map 依鍵排序，同樣的輸入永遠得到同樣的位元組。超過 `MAX_REQUEST_BYTES`
  （8 MiB）回傳 `too_large`；不是合法的 `CoreRequest` 回傳 `bad_request`。
- 回應上限 `MAX_RESPONSE_BYTES`（24 MiB）：所有宿主都經由 `CoreResponse::to_json` 輸出回應，序列化時
  寫入有上限的緩衝區，超過就整個丟棄並改回 `response_too_large` 錯誤（`path` 為 `null`）。在 schema 上限內
  的文件不會碰到這個上限：最大的合法回應是 255 個互相重疊的 `reserve` 空間（每對一個 `reserve_overlap`）
  的 `validate`（約 11 MB）與 `apply_patch`（約 15 MB），其餘 op 都與文件大小成線性（見
  `tests/api_regressions.rs` 的逐 op 最壞情況表）。伺服器的 stdout 上限（32 MiB）高於此上限加換行。
- `promptcad-core rpc`：從 stdin 讀**一個**請求，寫**一行**回應到 stdout（`handle_json` 的輸出加換行）。
  超過 8 MiB 的輸入會讀完丟棄後回傳 `too_large`；非 UTF-8 回傳 `bad_request`。

請求以 `op` 標記：`validate`（`document`、`completed` 預設 true）、`solve`、`normalize`、`upgrade`、
`downgrade`、`describe`（皆只有 `document`）、`apply_patch`（`document`、`patch`、`origin`，選填
`timestamp`、`log_seq`）、`version`。`document`／`patch` 是原始 JSON：由 core 依 `schema_version`
解析（1.0 會先升級），解析失敗會得到結構化的 `error` 回應（含指向請求內位置的 JSON pointer），
而不是整個請求失敗。文件不會自動移除範例專用的 `expected_resolved`，呼叫端需先移除。

回應以 `status` 標記：`ok` 帶 `result`（以同名 `op` 標記）；`error` 帶 `code`、`message`、`path`、
`violations`。`code` 是封閉列舉：`bad_request`、`too_large`、`unsupported_schema_version`、
`wrong_schema_version`、`invalid_document`、`schema_violation`、`invalid_patch`、`response_too_large`、
`internal`。
驗證報告無效、patch 被拒絕、無法降級為 1.0 都是領域結果，回應為 `ok`。

`describe` 的 `tree` 與文件大小成線性：每個零件／組裝只在第一次出現（前序、文件順序）時展開；之後再出現
（同一組裝重複列出的子項、多個父組裝的子項 `multi_owner`、循環 `reference_cycle`）都是
`repeated: true`、沒有 `children` 的回指節點。節點數最多為 `assemblies + parts + Σ children`（≤ 560），
另以 `MAX_TREE_NODES`（4096）作第二道防線（超過即 `internal`）。`describe` 只做 schema 檢查，不跑驗證器，
所以深度 ≤ 4 等圖形規則請用 `validate` 檢查。

範例（`rpc`）：

```text
$ echo '{"op":"version"}' | promptcad-core rpc
{"status":"ok","result":{"op":"version","crate_version":"0.1.0","api_version":1,"schema_versions":["1.0","2.0"]}}

$ echo '{"op":"validate","document":{"schema_version":"3.0"}}' | promptcad-core rpc
{"status":"error","code":"unsupported_schema_version","message":"unsupported schema_version (expected \"1.0\" or \"2.0\")","path":"/document/schema_version","violations":[]}
```

`schema` 子命令另外輸出 `core-request.schema.json`（以反序列化角度產生：有預設值的欄位為選填）與
`core-response.schema.json`（以序列化角度產生：core 一定會寫出的欄位，包括 `null` 與空陣列，都是
`required`）。兩者的 tagged union 都是帶 `const` 判別欄位的 `oneOf`；除了請求中的 `document`／`patch`、
op log 的 `revert.sections` 與 `log_entry.patch` 之外，沒有任意 JSON 的定義。

## 平台注意事項

倉庫根目錄的 `rust-toolchain.toml` 釘選 Windows MSVC 工具鏈。非 Windows 環境請先設定
`RUSTUP_TOOLCHAIN=stable`，CI 的 `core` job 也是這樣做。
