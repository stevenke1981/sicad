# promptcad-core

PromptCAD 的 Rust 核心（[ADR 0005](../../docs/adr/0005-rust-core-document-model.md)）。負責
CadDocument 2.0 文件模型、驗證、solver 前處理、1.0 遷移，以及 patch 與 op log 引擎。規格以
[`docs/CAD_DSL_2.md`](../../docs/CAD_DSL_2.md) 為準（正規化規格）。

- library crate：`promptcad_core`，不做任何 I/O；讀寫檔案由宿主負責。
- binary：`promptcad-core`，只使用 std 的小型 CLI，提供 `schema`、`validate`、`solve`、`upgrade`、`downgrade`、`normalize`、`apply-patch` 子命令（見下方「CLI 用法」）。
- JSON Schema 以 Rust 型別為唯一事實來源（[ADR 0008](../../docs/adr/0008-single-schema-source-of-truth.md)），
  產物提交在 [`../../schemas/cad2/`](../../schemas/cad2/)，CI 會比對是否過期。

## 模組

標註「Phase 2a 填寫中」的模組，目前仍只有檔頭說明或部分實作，會在 Phase 2a 逐步補齊。

| 模組 | 用途 |
|---|---|
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

路徑為 `-` 時從 stdin 讀取。`--out <file>` 會把結果寫入檔案而非 stdout。所有 JSON 輸出皆為
確定性、2 格縮排、LF 換行並以換行結尾。未知子命令回傳 2。

## 平台注意事項

倉庫根目錄的 `rust-toolchain.toml` 釘選 Windows MSVC 工具鏈。非 Windows 環境請先設定
`RUSTUP_TOOLCHAIN=stable`，CI 的 `core` job 也是這樣做。
