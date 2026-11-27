# Contributing

1. 先建立 issue，說明新幾何特徵、提示詞例子、期望 DSL 與可驗證輸出。
2. 幾何功能必須先加到 Pydantic DSL，再加編譯器；禁止直接拼接使用者提供的 Python。
3. 新功能至少包含一個規則解析測試、一個編譯器測試，以及必要的驗證規則。驗證規則寫在 Rust core（`crates/promptcad-core/src/validate/`），不在 Python 端。
4. 執行：

```bash
ruff check .
pytest
```

修改 `crates/promptcad-core/`（Rust core）時，另在該目錄執行 `cargo fmt --check`、`cargo clippy --all-targets -- -D warnings` 與 `cargo test`。

修改 `crates/promptcad-core/src/api.rs` 或 Rust 型別後，請重新生成 `schemas/cad2/`（`cargo run -- schema --out-dir ../../schemas/cad2`），並在 `web/` 執行 `npm run gen:types`，一併提交生成檔。伺服器的整合測試與 1.0 的驗證都需要 `promptcad-core` 執行檔，可先執行 `make core`。

`app/models/generated/` 由 Rust 型別生成（`make schema-py`，需要 `pyproject.toml` 的 `schema` extra），請勿手改；CI 以 `python scripts/gen_pydantic.py --check` 檢查漂移。

提交內容不要包含 API Key、私有模型、加工客戶資料或 `generated/` 產物。
