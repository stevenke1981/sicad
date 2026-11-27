# Contributing

1. 先建立 issue，說明新幾何特徵、提示詞例子、期望 DSL 與可驗證輸出。
2. 幾何功能必須先加到 Pydantic DSL，再加編譯器；禁止直接拼接使用者提供的 Python。
3. 新功能至少包含一個規則解析測試、一個編譯器測試，以及必要的驗證規則。驗證規則寫在 Rust core（`crates/promptcad-core/src/validate/`），不在 Python 端。
4. 執行：

```bash
ruff check .
pytest
```

修改 2.0 的幾何輸出（`app/services/compiler2.py`）或 `scripts/geometry_equivalence.py` 時，另執行幾何等價 harness：

```bash
make geometry-check
```

harness 比較 1.0 `CadQueryCompiler` 與 2.0 `Cad2Compiler` 的輸出，需要含 CadQuery 的直譯器（`.venv-desktop`，見 `README.md` 的「桌面版」）。pytest 的包裝測試會優先使用 `PROMPTCAD_CADQUERY_PYTHON` 指定的直譯器，未指定時尋找 `.venv-desktop`；找不到時略過。設定 `PROMPTCAD_REQUIRE_GEOMETRY=1` 則改為失敗。

2.0 編譯器的規則：只讀取 core 的 `resolved`，產生的程式碼不得含文件文字（以索引命名、數值經 `float` 輸出）；不支援的構造必須拋出 `CompileUnsupported` 並列出原因，不可靜默略過。

修改 `crates/promptcad-core/`（Rust core）時，另在該目錄執行 `cargo fmt --check`、`cargo clippy --all-targets -- -D warnings` 與 `cargo test`。

修改 `crates/promptcad-core/src/api.rs` 或 Rust 型別後，請重新生成 `schemas/cad2/`（`cargo run -- schema --out-dir ../../schemas/cad2`），並在 `web/` 執行 `npm run gen:types`，一併提交生成檔。伺服器的整合測試與 1.0 的驗證都需要 `promptcad-core` 執行檔，可先執行 `make core`。

`app/models/generated/` 由 Rust 型別生成（`make schema-py`，需要 `pyproject.toml` 的 `schema` extra），請勿手改；CI 以 `python scripts/gen_pydantic.py --check` 檢查漂移。

提交內容不要包含 API Key、私有模型、加工客戶資料或 `generated/` 產物。
