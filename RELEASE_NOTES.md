# Release Notes

## Unreleased / Phase 1a

版本號仍為 0.1.0。這是 [ADR 0010](docs/adr/0010-migration-phases.md) 第 1 階段的第一個切片，尚未發佈，也尚未達成第 1 階段的全部完成條件（見 [`PROJECT_STATUS.md`](PROJECT_STATUS.md)）。

新增：

- **新前端 `web/`**：React + TypeScript + Vite，功能對齊 `app/static/`（提示詞與 planner 選擇、生成、JSON 編輯與重新輸出、驗證報告、產物與 ZIP 下載、工作歷史、Token、SVG 預覽），另加 Three.js STL 檢視器。`npm --prefix web run dev` 開發、`npm --prefix web run build` 輸出到 `app/web_dist/`。
- **FastAPI 提供新前端**：`app/web_dist/index.html` 存在時在 `/` 提供；不存在時沿用 `app/static/`，既有使用方式不變。
- **Schema 管線**：`scripts/export_schema.py` 輸出 `schemas/promptcad.schema.json`（`--check` 檢查漂移），`npm --prefix web run gen:types` 生成前端型別；CI 新增 schema 漂移檢查與前端 typecheck／test／build。
- **Tauri v2 桌面外殼 `src-tauri/`**：啟動時在 `127.0.0.1` 的動態埠啟動 FastAPI sidecar（`app/sidecar.py`，以 `scripts/build_sidecar.py` 經 PyInstaller 打包），以一次性 Token 保護 API。`npm run tauri dev`／`npm run tauri build`。
- **CadDocument 2.0 規格凍結**：[`docs/CAD_DSL_2.md`](docs/CAD_DSL_2.md)，以及 `examples/cad2/` 的兩份手寫範例與 `tests/test_cad2_examples.py` 紙上驗證。只是規格；沒有任何程式接受 2.0 文件，實作依 ADR 0010 在第 2 階段以 Rust 進行。
- `pyproject.toml` 新增 `desktop` extra（PyInstaller）。

已知限制：

- 桌面版 sidecar 沒有打包 CadQuery，以 `source_only` 執行，不產生 STEP／STL／DXF。
- 只在 Windows x86_64（MSVC 工具鏈、WebView2）建置；安裝包大小與啟動時間尚未量測。
- 檢視器只支援 STL；GLB、元件選取與 port 圖層尚未實作。
- `app/static/` 仍保留作為備援。

## v0.1.0 — 2026-07-30

首個可執行 Prompt-to-CAD MVP：

- 受控、可編輯 CadDocument 1.0 DSL。
- 中文／英文規則解析及 OpenAI-compatible LLM planner。
- CadQuery 與 OpenSCAD 確定性編譯器。
- STEP、STL、DXF、SVG、Python、SCAD、JSON 輸出管線。
- Plate、cylinder、ring、L bracket、enclosure。
- Through、blind、clearance、tapped approximation、counterbore、countersink。
- X/Y/Z 軸孔與常見 M 制孔徑輔助。
- Web JSON 編輯與 `/generate-from-spec`。
- 驗證失敗阻止 CAD kernel 執行。
- Token-aware 預覽／下載、路徑安全、Docker、Conda、CI 與 26 項測試。
