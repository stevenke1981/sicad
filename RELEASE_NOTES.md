# Release Notes

## Unreleased / Phase 1

版本號仍為 0.1.0。這是 [ADR 0010](docs/adr/0010-migration-phases.md) 第 1 階段的內容，尚未發佈；除三平台測試矩陣與乾淨機器驗證的 CI 結果外，第 1 階段的完成條件均已達成（見 [`PROJECT_STATUS.md`](PROJECT_STATUS.md)）。

新增：

- **新前端 `web/`**：React + TypeScript + Vite，功能對齊原本的 `app/static/`（提示詞與 planner 選擇、生成、JSON 編輯與重新輸出、驗證報告、產物與 ZIP 下載、工作歷史、Token、SVG 預覽），另加 Three.js STL 檢視器。`npm --prefix web run dev` 開發、`npm --prefix web run build` 輸出到 `app/web_dist/`。
- **FastAPI 提供新前端**：`app/web_dist/index.html` 存在時在 `/` 提供（第 1 階段 1a 時不存在則沿用 `app/static/`；收尾時已移除該備援，見下方）。
- **Schema 管線**：`scripts/export_schema.py` 輸出 `schemas/promptcad.schema.json`（`--check` 檢查漂移），`npm --prefix web run gen:types` 生成前端型別；CI 新增 schema 漂移檢查與前端 typecheck／test／build。
- **Tauri v2 桌面外殼 `src-tauri/`**：啟動時在 `127.0.0.1` 的動態埠啟動 FastAPI sidecar（`app/sidecar.py`，以 `scripts/build_sidecar.py` 經 PyInstaller 打包），以一次性 Token 保護 API。`npm run tauri dev`／`npm run tauri build`。
- **CadDocument 2.0 規格凍結**：[`docs/CAD_DSL_2.md`](docs/CAD_DSL_2.md)，以及 `examples/cad2/` 的兩份手寫範例與 `tests/test_cad2_examples.py` 紙上驗證。只是規格；沒有任何程式接受 2.0 文件，實作依 ADR 0010 在第 2 階段以 Rust 進行。
- `pyproject.toml` 新增 `desktop` extra（PyInstaller）。

第 1 階段 1b（CadQuery 打包）：

- 桌面版 sidecar 以 PyInstaller **onedir** 打包 CadQuery 2.8.0，經 Tauri `bundle.resources` 安裝到 `<安裝目錄>/sidecar/`；凍結後以 `promptcad-sidecar.exe --render-worker` 執行 `model.py`，桌面版可輸出 STEP／STL／DXF／SVG。量測見 [`PROJECT_STATUS.md`](PROJECT_STATUS.md)（安裝包 107.0 MiB、安裝後 550.9 MiB）。

第 1 階段收尾：

- **負責人簽核**：2026-11-26 核准以 PyInstaller 取代 conda-pack（ADR 0006／0010 回顧紀錄），並核准移除 `app/static/`（ADR 0003 回顧紀錄）。
- **移除 `app/static/`（破壞性變更）**：`app/web_dist/index.html` 不存在時，`/` 改為回傳 HTTP 503 的說明頁（提示 `npm --prefix web run build`，並連到 `/docs`），不再退回舊介面；`/assets` 不掛載，API 不受影響。伺服器模式請先建置前端，或使用 Docker。非 editable 的 wheel 只有在先建置前端時才含 Web UI。
- **Docker 多階段建置**：`node:24-slim` 階段建置 `web/`，再複製到映像的 `app/web_dist/`；CI 新增 `docker` job，實際建置並啟動映像，確認 `/` 回傳新前端。
- **乾淨機器驗證**：`scripts/frozen_smoke.py --isolated-env`（只有系統 `PATH`、無 `PYTHON*`／`VIRTUAL_ENV`／`CONDA*`，並監看行程樹不得出現 sidecar 目錄外的映像或 `python*.exe`）與新的 `scripts/frozen_kill_check.py`（關閉 Job Object 時 sidecar 與 render worker 須在 5 秒內結束），在 GitHub `windows-latest` 的 `desktop-bundle.yml` 中執行。
- **第三方授權聲明**：新增 `scripts/gen_third_party_notices.py` 與生成的 `THIRD_PARTY_NOTICES.md`（Python、npm、Rust 相依與 OCCT、VTK、CPython 等原生元件；LGPL 元件的替換說明與原始碼網址），隨安裝包安裝到 `<安裝目錄>\THIRD_PARTY_NOTICES.md`。
- **2D 預覽**：CadQuery 的 `model.svg` 以紙色底顯示，深色主題下黑色線條可讀；`preview.svg` 不變。

已知限制：

- 只在 Windows x86_64（MSVC 工具鏈、WebView2）建置；三平台測試矩陣尚未建立。
- 乾淨機器驗證以 `desktop-bundle.yml` 的實際執行結果為準：2026-11-26 於 GitHub `windows-latest`（Windows Server 2025，4 CPU）首次執行 [run 45723089074](https://github.com/stevenke1981/sicad/actions/runs/45723089074) 全數通過：建置與安裝後的 sidecar 皆在 isolated env 下通過 smoke（各觀察到 7 個 render worker，`foreign_processes=[]`）；安裝 sidecar 的 Job Object 中斷檢查 `KILL_CHECK_OK`；強制結束已安裝的 `promptcad-studio.exe` 後 sidecar 於 0.007 秒內結束（`job.rs` 實證，此步驟已改為必過）；安裝目錄含 `THIRD_PARTY_NOTICES.md`。量測：安裝檔 107.5 MiB、安裝後 554.1 MiB、READY 0.64–0.66 s、warm render 2.38 s、端到端 generate 2.42 s。
- 檢視器只支援 STL；GLB、元件選取與 port 圖層尚未實作。

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
