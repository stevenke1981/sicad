# 0011：桌面版擴展到 Windows、macOS 與 Linux 三平台

- **狀態**：Accepted
- **日期**：2026-11-26
- **決策者**：專案負責人
- **影響範圍**：`src-tauri/`（`sidecar.rs`、`job.rs`、`tauri.{macos,linux}.conf.json`）、`app/sidecar.py`、`app/parent_watch.py`、`scripts/{desktop_env,desktop_lock,build_sidecar,frozen_smoke,frozen_kill_check,gen_third_party_notices}.py`、`requirements/desktop-*.txt`、`THIRD_PARTY_NOTICES.md`、`.github/workflows/{ci,desktop-bundle}.yml`

## 背景

[0010](0010-migration-phases.md) 第 1 階段的完成條件之一是三平台測試矩陣；到 2026-11-26 為止，桌面版只在 Windows x86_64 建置與驗證。把同一套 Tauri 外殼 + PyInstaller onedir sidecar（[0002](0002-desktop-shell-tauri-v2.md)、[0006](0006-geometry-kernel-occt-python-sidecar.md)）帶到 macOS 與 Linux 時，有下列事實與限制：

- **關閉外殼時終止 sidecar**：Windows 以 `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` 的 Job Object 達成，並已在 GitHub runner 實證（run 45723089074）。macOS、Linux 沒有 Job Object；sidecar 每次渲染還會啟動一個 `--render-worker` 子行程，兩者都不能比外殼活得久。
- **Linux 的 `PR_SET_PDEATHSIG`** 綁定的是「建立子行程的執行緒」而非行程，Tauri 以非主執行緒啟動 sidecar 時會誤觸發；macOS 沒有對應機制。以 `getppid()` 輪詢則有延遲，且父行程可能被 subreaper 收養而判斷失準。
- **原生 wheel**：cadquery-ocp 在 Linux 只提供 `manylinux_2_31` wheel；VTK、OCP、casadi 在各平台以不同方式隨附原生函式庫（Windows 為 delvewheel 的 `<套件>.libs/*.dll`，Linux 為 auditwheel 的 `<套件>.libs/*.so*` 與套件內的 `lib*.so`，macOS 為 delocate 的 `.dylibs/*.dylib`）。
- **建置環境未鎖版**：`.venv-desktop` 只固定 `cadquery==2.8.0`，每次重建可能凍結不同版本；提交的 `THIRD_PARTY_NOTICES.md` 因此只是一份快照，CI 無法以 `--check` 驗證它。
- `uv pip compile --python-platform` 在一台機器上即可為三個平台解析出相同的主要版本（cadquery-ocp 7.9.3.1.1、vtk 9.6.2、casadi 3.8.2、nlopt 2.11.0、numpy 2.5.5、PyInstaller 6.22.3）。
- 專案沒有 Apple Developer ID、也沒有 Windows 程式碼簽章憑證；本機沒有 macOS 與 Linux 機器，只能以 GitHub hosted runner 驗證。
- `rust-toolchain.toml` 固定 `stable-x86_64-pc-windows-msvc`（本機預設 host 為 gnu），非 Windows 的 runner 必須另外指定工具鏈。

## 決策

### 1. 關閉外殼時終止 sidecar（各平台）

| 平台 | 機制 | 外殼（`src-tauri/src/sidecar.rs`） | sidecar |
|---|---|---|---|
| Windows | Job Object（不變） | 建立 kill-on-close Job，sidecar 啟動後立即加入；stdin 為 null，不傳旗標 | 不變；render worker 繼承 Job |
| macOS／Linux | **stdin EOF + 行程群組** | `Stdio::piped()` 的 stdin，`ChildStdin` 由 `Sidecar` 持有；`CommandExt::process_group(0)`；`shutdown()` 先關閉 stdin，以 `try_wait` 等待至多 3 秒，再 `kill(-pid, SIGKILL)` | 收到 `--exit-on-stdin-eof` 時 `os.setpgid(0, 0)`（失敗忽略），daemon 執行緒阻塞於 `sys.stdin.buffer.read()`；讀到 EOF 即對自己的行程群組送 `SIGTERM`，約 2 秒後 `SIGKILL` |

- 外殼當掉（含 `kill -9`）時，核心會關閉它持有的 pipe 寫入端，sidecar 讀到 EOF，效果與正常關閉相同；不需要外殼存活即可收拾。
- render worker 以 `stdin=DEVNULL` 啟動、不另開 session，因此留在 sidecar 的群組內，一次 `killpg` 即可帶走；`app/services` 不需修改。
- 開發用的 `PROMPTCAD_SIDECAR_CMD` 在 unix 上同樣附加 `--exit-on-stdin-eof`。
- PyInstaller onedir 在 unix 上應為單一行程（bootloader 不 fork）；`scripts/frozen_kill_check.py` 的 sidecar 模式檢查行程樹恰為 sidecar + render worker、且都在 sidecar 的群組內，再以兩種方式結束 pipe：`pipe`（本腳本關閉寫入端，等同正常關閉）與 `holder`（持有唯一寫入端的中介行程被 `SIGKILL`，等同外殼當掉），兩者都必須在 5 秒內不留任何行程。
- 不採用 `PR_SET_PDEATHSIG` 與 `getppid()` 輪詢（理由見背景）。

### 2. 安裝包格式

以 Tauri 的平台設定檔（與 `tauri.conf.json` 合併）指定：

| 平台 | 設定檔 | 格式 | CI 判定 |
|---|---|---|---|
| Windows x86_64 | `tauri.conf.json` | NSIS（`currentUser`） | 必過 |
| macOS arm64 | `tauri.macos.conf.json` | `.app`（必過）、`.dmg`（參考） | `.app` 必過 |
| Linux x86_64 | `tauri.linux.conf.json` | `.deb`（必過，`deb.depends` 列出 GL／X11 等系統函式庫）、AppImage（參考） | `.deb` 必過 |

sidecar 的版面契約不變：`binaries/promptcad-sidecar/{promptcad-sidecar[.exe], _internal/}` → `<resource_dir>/sidecar/`（macOS：`PromptCAD Studio.app/Contents/Resources/sidecar/`；Linux .deb：`/usr/lib/PromptCAD Studio/sidecar/`）。

### 3. 簽章現況

- **Windows**：NSIS 安裝檔未簽章（SmartScreen 會警告）。
- **macOS**：只做 ad-hoc 簽章（`signingIdentity: "-"`），未公證（notarization）；`minimumSystemVersion` 為 11.0。從網路下載的 `.app` 會被 Gatekeeper 標記隔離，使用者需自行執行 `xattr -dr com.apple.quarantine "/Applications/PromptCAD Studio.app"`（README 已註明）。
- **Linux**：`.deb` 與 AppImage 未簽章。
- 取得 Developer ID 或程式碼簽章憑證前，所有安裝包都只作測試用途，不公開發布。

### 4. glibc 下限

cadquery-ocp 的 `manylinux_2_31` 決定 Python 端的下限為 glibc 2.31；但外殼的 Rust 執行檔、PyInstaller bootloader 與 CPython 都在 `ubuntu-22.04` runner 上建置，會連結到該 runner 的 glibc 2.35，且外殼需要 `libwebkit2gtk-4.1`。因此 `.deb` 的實際下限為 **glibc 2.35／Ubuntu 22.04、Debian 12 或更新版**。CI 刻意使用 `ubuntu-22.04` 而非 `ubuntu-latest`，避免下限隨 runner 映像升級而悄悄提高。

### 5. 建置環境鎖版

- `scripts/desktop_lock.py` 以 `uv pip compile pyproject.toml --extra cad --extra desktop --python-version 3.12 --python-platform <triple> --generate-hashes` 在**同一台機器**為三個平台各產生一份含雜湊的鎖檔：`requirements/desktop-windows-x86_64.txt`（`x86_64-pc-windows-msvc`）、`desktop-macos-arm64.txt`（`aarch64-apple-darwin`）、`desktop-linux-x86_64.txt`（`x86_64-manylinux_2_31`）。
- 重新產生時沿用既有版本作為偏好；`--check` 在暫存目錄重新編譯並逐位元組比對（`ci.yml` 的 `lock-check` job），`--upgrade` 一次更新三份，不得只更新其中一份。
- `scripts/desktop_env.py` 依主機平台挑選鎖檔，以 `uv pip sync --require-hashes` 讓 `.venv-desktop` 與鎖檔完全一致，再 `uv pip install --no-deps -e .`；沒有 uv 時退回 `pip install --require-hashes -r`。沒有鎖檔的平台（Intel macOS、Linux arm64、Windows arm64）直接拒絕。`--venv`（或 `PROMPTCAD_DESKTOP_VENV`）可在其他位置建立相同環境以比對。
- 三份鎖檔除平台專屬套件外版本完全相同：Windows 多 `pefile`、`pywin32-ctypes`；macOS 多 `macholib`；macOS／Linux 多 `uvloop`。
- CI 固定 CPython 的修補版本（`3.12.10`），因為授權聲明記錄該版本與其 LICENSE。

### 6. 第三方授權聲明

- 提交在版本庫的 `THIRD_PARTY_NOTICES.md` 是 **Windows 版、且具權威性**：Windows 的 `desktop-bundle.yml` 以鎖檔建立環境後執行 `--check --strict`（必過），不一致即失敗。
- macOS／Linux 的 CI 以 `--strict` 就地重新產生（路徑、函式庫名稱、Rust target 依平台改寫，並移除只屬於 Windows 的 MSVC runtime、WebView2 bootstrapper、NSIS），隨該平台的安裝包發佈並上傳為 artifact，**不提交**。
- 產生器辨識三種 wheel 隨附函式庫的佈局（delvewheel、auditwheel、delocate）與套件內的 `lib*.so*`／`lib*.dylib`，正規化 `lib` 前綴、auditwheel 雜湊與 soname 版本後比對 `VENDORED_DLLS`；未登錄的函式庫在 `--strict` 下失敗，unix 專屬的分類（libgomp、libgfortran、libquadmath、libomp 等）依 CI 首次 `--strict` 失敗的結果補齊。
- `scripts/build_sidecar.py` 的 `PRUNED_DISTS`（每次建置都驗證確實不在 `_internal/` 的套件）不列入聲明；讀不到時 `--strict` 失敗，避免聲明內容默默改變。

### 7. CI 判定（首次執行）

- **三平台必過**：鎖檔安裝、凍結與 VTK 精簡、對建置出的 onedir 跑 frozen smoke、sidecar 模式的終止檢查、授權聲明 `--strict`、`tauri build`（`.exe`／`.app`／`.deb`）。
- **只有 Windows、必過**：授權聲明 `--check`、已安裝外殼的終止檢查、已安裝 sidecar 的 smoke 與 `--isolated-env` 監看、onedir ≤ 460 MiB。
- **macOS／Linux 參考（`continue-on-error: true`）**：外殼模式的終止檢查（Linux 以 `xvfb-run -a` 執行）、已安裝安裝包的 smoke、`.dmg`、AppImage、`--isolated-env` 監看。連續兩次全綠後改為必過。
- `ci.yml` 的 `desktop` job 改為三平台矩陣執行 `cargo fmt`／`clippy`／`test`（Linux 先安裝 WebKitGTK 開發套件）；非 Windows 的 job 設定 `RUSTUP_TOOLCHAIN=stable`，`rust-toolchain.toml` 維持 Windows MSVC。

### 8. VTK 替身模組與修剪

- **問題**：OCP 的 `OCP/IVtk*/__init__.py` 以 `try: import vtk` 檢查 VTK 是否存在，而 VTK 套件的 `vtk.py` 會一次載入全部 VTK 模組，使 PyInstaller 收集全部 163 個模組（`vtk.libs` 264 MiB）。CadQuery 本身只使用 `vtkmodules.*`。
- **做法**：`build_sidecar.py` 在建置時產生 `build/sidecar/vtk_shim/vtk.py`（只 `import vtkmodules.vtkCommonCore`），並放在 PyInstaller 搜尋路徑最前面；保留集合由實際執行一次 STEP/STL/DXF/SVG 匯出觀察 `sys.modules` 而得，再以原生相依閉包（`scripts/native_deps.py`：pefile／`otool -L`／`readelf -d`）刪除閉包外、且檔名為 VTK／Viskores 的函式庫。建置會確認打包內的 `vtk` 確實是替身，並在 VTK 依賴解析不到、或超過 `--max-mib` 時失敗。
- **結果**：Windows onedir 547.5 → 402.4 MiB。
- **後果**：打包內無法 import `cadquery.vis`、`cadquery.fig` 與 `cadquery.occ_impl.jupyter_tools`（互動視覺化，生成的 `model.py` 不使用）。`--no-prune` 可跳過刪除（仍使用替身）。若日後 CadQuery 升級後在 `import cadquery` 時就載入 `vis`，因保留集合是實際觀察的，只會讓體積變大，不會壞掉；四種格式的 smoke 測試仍是最終把關。
- **已知限制**：Unix 上若 sidecar 先自行崩潰，外殼的 `try_wait` 會先回收它，之後不再對行程群組送 SIGKILL，仍在執行的 render worker 可能殘留（Windows 由 Job Object 處理）。下一輪改以 `waitid(WNOWAIT)` 保留殭屍行程後無條件 kill 群組。

## 考慮過的替代方案

| 方案 | 優點 | 缺點 | 結論 |
|---|---|---|---|
| unix：stdin EOF + 行程群組 | 外殼當掉也有效（核心關閉 pipe）；macOS／Linux 相同；worker 跟著群組走，不改 `app/services` | sidecar 需新增旗標與監看執行緒；GIL 長時間被占用時反應會延遲（外殼另有 `killpg` 後援） | 採用 |
| unix：`PR_SET_PDEATHSIG` | 核心直接送訊號 | 只有 Linux；綁定執行緒而非行程，Tauri 的非主執行緒啟動會誤觸發；不涵蓋 worker | 不採用：語意不符且不跨平台 |
| unix：sidecar 輪詢 `getppid()` | 實作簡單 | 有輪詢延遲；被 subreaper 收養時判斷錯誤；仍需另外處理 worker | 不採用：不可靠 |
| 鎖檔：三份平台鎖檔（同機產生） | 每個平台的雜湊與版本精確；Windows 聲明可重現並以 `--check` 驗證 | 三份檔案需一起更新（`--upgrade` 一次處理，`lock-check` 防漂移） | 採用 |
| 鎖檔：`uv pip compile --universal` 單一檔 | 一份檔案 | 環境標記使檔案難讀；`--require-hashes` 與平台專屬 wheel 的組合較難稽核 | 不採用：可讀性與稽核性較差 |
| 鎖檔：只固定 cadquery（現況） | 不需維護 | 每次重建可能凍結不同版本，聲明無法驗證 | 不採用：不可重現 |
| 聲明：只提交 Windows 版，unix 在 CI 重生 | 版本庫只有一份權威檔；unix 仍隨包附上正確內容 | unix 版不在版本審查中 | 採用（unix 版以 artifact 留存） |
| 聲明：提交三份平台版 | 每份都可 `--check` | 本機無 macOS／Linux，無法在送出前產生；三份長檔案的審查成本高 | 不採用：無法在本機重現 |
| macOS 以 Developer ID 簽章並公證 | 使用者不需處理隔離屬性 | 需付費帳號與 CI 機密；目前不公開發布 | 暫不採用：取得帳號後重新評估 |
| Linux 改用 Flatpak／Snap | 沙箱與自動更新 | 需額外的 runtime 與 sidecar 權限設定；與 Tauri 內建 bundler 不同 | 不採用：先以 `.deb`／AppImage 驗證 |

## 後果

### 正面

- 三個平台以同一套外殼、sidecar 與驗證腳本建置；unix 的終止機制在外殼當掉時同樣有效。
- `.venv-desktop` 可重現：同一份鎖檔在任何機器上 `uv pip freeze` 結果相同，Windows 授權聲明可由 CI 以 `--check` 把關。
- unix 的終止邏輯與授權聲明的平台差異都集中在純函式中，可在 Windows 上以單元測試涵蓋。

### 負面

- 依賴升級需執行 `desktop_lock.py --upgrade`、重建 `.venv-desktop`、重新產生 Windows 聲明，三步一起提交。
- macOS 與 Linux 的安裝包未簽章，只適合測試；macOS 使用者需手動移除隔離屬性。
- macOS／Linux 的實際驗證只能在 CI 上進行，第一次執行預期會因未登錄的原生函式庫等原因需要補資料。

### 風險與緩解

| 風險 | 影響 | 緩解措施 |
|---|---|---|
| Tauri 複製資源時遺失 PyInstaller 的符號連結或執行權限 | 安裝後的 sidecar 無法執行 | CI 對安裝後的 sidecar 執行 `test -x` 與 smoke；必要時於 `build_sidecar.py` 展開符號連結 |
| AppImage／linuxdeploy 改寫 RPATH | AppImage 內的 sidecar 載入失敗 | AppImage 為參考項目；`.deb` 為必過 |
| VTK 延遲載入了精簡後刪除的函式庫 | 某些匯出格式失敗 | keep-set + 原生相依閉包，只刪 VTK 命名的檔案；四種格式的 smoke；`--no-prune` 逃生門 |
| macOS 對上千個 Mach-O 做 ad-hoc 簽章、或 runner 上無法啟動 GUI | `.app` 外殼檢查不穩定 | 外殼模式檢查為參考項目，兩次全綠後再改必過 |
| sidecar 的監看執行緒因 GIL 長時間被占用而延遲 | 關閉外殼後 sidecar 晚幾秒結束 | 外殼在 3 秒後直接 `kill(-pid, SIGKILL)` |
| 鎖檔漂移（`pyproject.toml` 改了但鎖檔未更新） | 建置使用不一致的版本 | `ci.yml` 的 `lock-check`；`--upgrade` 一次更新三份 |
| hosted runner 升級 glibc | `.deb` 下限悄悄提高 | 固定 `ubuntu-22.04`，下限寫在本 ADR |

## 驗收或回顧條件

- **驗收**：
  - `ci.yml` 在三個平台上 `pytest`、`ruff`、`cargo fmt`／`clippy`／`test` 全綠，`lock-check` 通過。
  - Windows 的 `desktop-bundle.yml` 保留全部既有檢查並全綠，另加授權聲明 `--check`，onedir ≤ 460 MiB（前後量測記錄於 `measurements.md`）。
  - 三平台對精簡後的 onedir 輸出四種格式皆通過；unix sidecar 模式的終止檢查兩種變體 3/3 通過；`.deb` 與 `.app` 已上傳，安裝後的 sidecar 可執行。
  - `desktop_lock.py --check` 無差異；`desktop_env.py --recreate` 重建後 `uv pip freeze` 與鎖檔一致。
- **回顧**：
  - macOS／Linux 的參考項目連續兩次全綠時（改為必過）。
  - 取得 Apple Developer ID 或程式碼簽章憑證時（簽章與公證）。
  - cadquery-ocp 提供更低的 manylinux 標籤、或 CI 需要離開 `ubuntu-22.04` 時（glibc 下限）。
  - 需要支援 Intel macOS、Linux arm64 或 Windows arm64 時（新增鎖檔與矩陣項目）。

## 相關文件

- [0002](0002-desktop-shell-tauri-v2.md)、[0006](0006-geometry-kernel-occt-python-sidecar.md)、[0010](0010-migration-phases.md)
- `scripts/desktop_lock.py`、`scripts/desktop_env.py`、`requirements/desktop-*.txt`
- `scripts/gen_third_party_notices.py`、`THIRD_PARTY_NOTICES.md`
- `scripts/frozen_kill_check.py`、`app/parent_watch.py`、`src-tauri/src/sidecar.rs`、`src-tauri/src/job.rs`
- `src-tauri/tauri.macos.conf.json`、`src-tauri/tauri.linux.conf.json`
- `.github/workflows/ci.yml`、`.github/workflows/desktop-bundle.yml`

## 回顧紀錄

〈Accepted 之後的補充，只增不改。〉
