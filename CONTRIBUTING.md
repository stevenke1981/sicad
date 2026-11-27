# Contributing

1. 先建立 issue，說明新幾何特徵、提示詞例子、期望 DSL 與可驗證輸出。
2. 幾何功能必須先加到 Pydantic DSL，再加編譯器；禁止直接拼接使用者提供的 Python。
3. 新功能至少包含一個規則解析測試、一個編譯器測試，以及必要的驗證規則。驗證規則寫在 Rust core（`crates/promptcad-core/src/validate/`），不在 Python 端。
4. 執行：

```bash
ruff check .
pytest
```

修改幾何輸出（`app/services/compiler2.py`，1.0 與 2.0 共用）、`scripts/geometry_equivalence.py` 或 `tests/fixtures/v1_geometry_golden.json` 時，另執行幾何等價 harness：

```bash
make geometry-check
```

harness 預設為 golden 模式：以 `Cad2Compiler` 產生每個案例，比較凍結的 `tests/fixtures/v1_geometry_golden.json`，包括體積、包圍盒、質心、表面積、實體數、警告數與 `shape_valid`，並比對 `spec_sha256` 與 `model_sha256`。舊的 `CadQueryCompiler` 已移除，不再做 A/B 比較；基準來自它移除前的輸出。metrics 層需要含 CadQuery 的直譯器（`.venv-desktop`，見 `README.md` 的「桌面版」）。pytest 的包裝測試會優先使用 `PROMPTCAD_CADQUERY_PYTHON` 指定的直譯器，未指定時尋找 `.venv-desktop`；找不到時略過。設定 `PROMPTCAD_REQUIRE_GEOMETRY=1` 則改為失敗。`tests/test_v1_compile_golden.py` 不需要 CadQuery，會比對 `model_sha256`。

golden 的重新基準化規則：`--write-golden PATH` 以目前輸出重新建立 golden；`PROMPTCAD_UPDATE_GOLDEN=1` 只改寫 `model_sha256` 欄位。任何重新基準化都必須在 PR 中說明理由（哪些案例改變、為什麼改變），不以放寬容差取代說明。

專案只有一個 CadQuery 編譯器（`app/services/compiler2.py`），1.0 與 2.0 共用。`Cad2Compiler` 的規則：只讀取 core 的 `resolved`，產生的程式碼不得含文件文字（以索引命名、數值經 `float` 輸出）；不支援的構造必須拋出 `CompileUnsupported` 並列出原因，不可靜默略過。

修改 `crates/promptcad-core/`（Rust core）時，另在該目錄執行 `cargo fmt --check`、`cargo clippy --all-targets -- -D warnings` 與 `cargo test`。

修改 `crates/promptcad-core/src/api.rs` 或 Rust 型別後，請重新生成 `schemas/cad2/`（`cargo run -- schema --out-dir ../../schemas/cad2`），並在 `web/` 執行 `npm run gen:types`，一併提交生成檔。伺服器的整合測試與 1.0 的驗證都需要 `promptcad-core` 執行檔，可先執行 `make core`。

`app/models/generated/` 由 Rust 型別生成（`make schema-py`，需要 `pyproject.toml` 的 `schema` extra），請勿手改；CI 以 `python scripts/gen_pydantic.py --check` 檢查漂移。

提交內容不要包含 API Key、私有模型、加工客戶資料或 `generated/` 產物。
