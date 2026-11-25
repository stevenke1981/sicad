# CadDocument 2.0 DSL（凍結規格）

> 狀態：**階段 0 規格凍結**（2026-11-26）。本文是 CadDocument 2.0 的規範性文件，取代 [`CONCEPT_DIMENSIONAL_LADDER.md`](CONCEPT_DIMENSIONAL_LADDER.md) 中與本文衝突的敘述；概念文件保留設計理由與端到端敘事。第 12 節標示「待負責人簽核」的建議預設值不屬於凍結範圍。
>
> **實作位置：** 依 [ADR 0010](adr/0010-migration-phases.md) 第 2 階段與 [ADR 0005](adr/0005-rust-core-document-model.md)、[ADR 0008](adr/0008-single-schema-source-of-truth.md)，2.0 的型別、驗證器、求解前處理與 patch 引擎在 **Rust core**（`serde` + `schemars`）實作，Pydantic 與 TypeScript 型別由其生成。**不在 Python 實作 2.0。** 概念文件中的 `app/models/cad2.py`、`app/services/migrate.py`、`app/services/solver.py`、`app/services/expr.py`、`app/services/ports.py` 等 Python 提案檔名一律作廢。
>
> 1.0 格式見 [`CAD_DSL.md`](CAD_DSL.md)，不受本文影響；v0.1.0 的程式只接受 1.0。

---

## 0. 階段 0 決議摘要

概念文件 §14.4 列出的七個缺口，凍結結果如下：

| §14.4 | 問題 | 決議 | 本文章節 |
|---|---|---|---|
| 1 | 求解器的循環規則自相矛盾 | 改為「以 `fixed` 為根、逐軸追蹤已知性、依文件順序迭代到不動點」；兩端皆已知時判定冗餘或矛盾；`constraint_cycle` 退役 | [§5.5](#55-平移求解不動點規則)、[§9.3](#93-退役代碼) |
| 2 | `PatchOp.value: JsonValue` 無法用 strict 模式表達 | 改為以 `op` 判別的封閉聯集，每個 op 有固定欄位；每個角色的 schema 是該聯集的子集 | [§8](#8-patch-協定) |
| 3 | `origin` 名稱衝突 | 元件欄位改名為 `provenance`；`origin` 只作為埠名；欄位引用改用 `id:field`，埠引用維持 `id.port` | [§1.3](#13-引用語法)、[§3.1](#31-共通欄位) |
| 4 | 2.0 新增孔的方向未定義 | 切除方向由入口所在面的法向決定，否則取 `shape.axis` 反向；深度自入口起算；貫穿類孔跨越所屬零件包圍盒 | [§7](#7-孔的方向與切刀) |
| 5 | 核心層錯誤來得太晚 | 核心層診斷使用同一個 issue 結構並標 `layer: "kernel"`；核心層 `error` 使工作 `status="failed"`，保留來源產物，移除幾何產物 | [§9.4](#94-核心層錯誤語意) |
| 6 | 權限矩陣沒有涵蓋綁定 | `set_binding`／`remove_binding` 需要與目標欄位 `set_field` 相同的權限；被取代的假設只能由同一 patch 內明確的 `remove_assumption` 移除 | [§8.5](#85-權限矩陣)、[§8.7](#87-假設的移除) |
| 7 | 其他細節 | `distance`：`(a − origin_b)·n_b = value`；`mate`：`n_a = −n_b`、沿 `n_b` 的間隙 ≥ 0；`flush`：`n_a = n_b`；`intersect` 只接受 Volume 或 `envelope` Empty；1.0 孔遷移為每孔 5 筆紀錄；CadQuery 呼叫實測移到第 2 階段（Docker） | [§5.2](#52-約束種類與正負號約定)、[§6.1](#61-body-布林)、[§10](#10-10-遷移)、[§13](#13-未凍結延後事項) |

概念文件 §5.2「2.0 不引入數值求解器」的立場讓位給 [ADR 0007](adr/0007-constraint-solver.md)：本文 §5.5 的不動點規則就是 ADR 0007 允許的「可循序求值的前處理層」，對軸對齊文件是規範行為；數值求解器的選擇仍在第 3 階段決定，並放在同一個求解介面之後。

---

## 1. 通則

### 1.1 數值與單位

| 名稱 | 定義 |
|---|---|
| 單位 | 固定為 `mm`（`unit: "mm"`） |
| `Dim` | `0 < x ≤ MAX_DIMENSION_MM`（`100000`） |
| `Coord` | `−100000 ≤ x ≤ 100000` |
| 比較容差 | `1e-6` mm（冗餘判定、`expected_resolved` 比對、綁定快取比對） |
| `AxisDir` | `"+x"`、`"-x"`、`"+y"`、`"-y"`、`"+z"`、`"-z"` |

所有物件拒絕未知欄位（Rust `deny_unknown_fields`），所有清單都有上限。

### 1.2 識別字與命名空間

`Id` 的正規表示式為 `^[a-z][a-z0-9_]{0,31}$`。識別字建立後不可改名，且**永遠不會成為產生程式中的變數名**（沿用 1.0 的 `hole_{index}` 做法）。

| 命名空間 | 成員 | 理由 |
|---|---|---|
| N1 | `components[*].id` ∪ `constraints[*].id` | `id:field` 欄位引用可能指向元件或約束，必須無歧義 |
| N2 | `parts[*].id` ∪ `assemblies[*].id` | `root` 與 `assemblies[*].children` 可同時指向零件或組裝 |
| N3 | `parameters[*].name` | 運算式中的裸名稱一律是參數；`min`、`max` 為保留字，不可作為參數名 |

同一命名空間內重複即 `duplicate_id`。不同命名空間可以同名，但不建議。

### 1.3 引用語法

| 語法 | 正規表示式 | 出現位置 | 例子 |
|---|---|---|---|
| `id` | `Id` | `shape` 內的升維來源、`body[*].component`、`children`、`root`、`set_body.part_id` | `v_plate` |
| `id.port`（埠引用） | `^[a-z][a-z0-9_]{0,31}\.[a-z][a-z0-9_]{0,31}$` | 約束的 `a`、`b` | `v_plate.top`、`e_h1.axis` |
| `id:field`（欄位引用） | `^[a-z][a-z0-9_]{0,31}:[a-z][a-z0-9_]{0,31}$` | 綁定的 `target`、`expr` 內、`set_field`／`set_binding`／`remove_binding` 的 `target` | `v_body:height`、`k_d1:u` |

兩種語法必須分開，因為同一個名字可以同時是欄位與埠：`hole` 空的 `shape.axis` 是欄位（`"z"`），`e_h1.axis` 是埠（軸線）。`e_h1:axis` 指欄位，`e_h1.axis` 指埠，永不混淆。

`id:field` 中的 `field`：

- 元件：`shape` 內的**數值欄位**名（不含 `kind`，也不含引用欄位與列舉欄位），見 §3.3 各表的「可綁定欄位」。
- 約束：`value`、`u`、`v` 三者之一。

其他欄位（列舉、引用、`frame`、零件欄位）不可作為欄位引用，違者 `unknown_field`。

### 1.4 正規序列化

正規化器輸出的 `spec.json`：所有欄位都寫出（含 `null` 與空清單），鍵的順序依本文各表的列出順序，數值以 JSON number 輸出。這與 1.0 `spec.json` 寫出全部孔欄位的做法一致，也是 patch 雜湊（`patch_sha256`）與往返測試比對的基礎。

---

## 2. 文件頂層

| 欄位 | 型別 | 上限 | 說明 |
|---|---|---|---|
| `schema_version` | `"2.0"` | | 判別欄位；API 以它區分 1.0 與 2.0 |
| `name` | `str` | 1–80 | 預設 `"promptcad-design"` |
| `source_prompt` | `str` | 1–20000 | |
| `unit` | `"mm"` | | |
| `revision` | `int ≥ 0` | | 由協調器在 patch 被接受時加一 |
| `parameters` | `Parameter[]` | 64 | §4.1 |
| `bindings` | `Binding[]` | 128 | §4.2 |
| `components` | `Component[]` | 1–256 | §3 |
| `constraints` | `Constraint[]` | 512 | §5 |
| `parts` | `Part[]` | 1–32 | §6 |
| `assemblies` | `Assembly[]` | 16，巢狀深度 4 | §6 |
| `root` | `Id` | | 由協調器維護，見 §6.3 |
| `assumptions` | `str[]` | 32 | 同 1.0 |
| `notes` | `str[]` | 32 | 同 1.0 |
| `planner` | `PlannerMetadata` | | 同 1.0：`{planner, confidence, review_required}` |

說明：

- 2.0 沒有文件層的 `material`、`base`、`holes`、`fillets`、`chamfers`；它們移到零件與元件（§10）。
- 多 agent 迴圈進行期間，文件可以暫時沒有元件或零件（`components` 與 `parts` 的下限在**完成文件**時才檢查，中間修訂允許空清單；見 §8.6）。

---

## 3. 元件

### 3.1 共通欄位

| 欄位 | 型別 | 說明 |
|---|---|---|
| `id` | `Id` | N1 命名空間 |
| `kind` | `"empty"`／`"dot"`／`"line"`／`"plane"`／`"volume"` | 判別欄位 |
| `role` | `"cut"`／`"reserve"`／`"keep_out"`／`"envelope"` | **只有** `kind="empty"` 有此欄位，且必填 |
| `shape` | 依 `kind` 的判別聯集（以 `shape.kind` 判別） | §3.3 |
| `label` | `str \| null`，最長 80 | 只給人看；不進入程式碼，也不回饋給其他 agent |
| `provenance` | `{agent, patch_id} \| null` | 由協調器蓋章 |

`provenance`：

| 欄位 | 型別 | 說明 |
|---|---|---|
| `agent` | `"space_planner"`／`"component_smith"`／`"joiner"`／`"assembler"`／`"human"`／`"migration"` | 建立此元件的來源 |
| `patch_id` | `str \| null`，最長 64 | 建立此元件的 patch；協調器以 `r<revision_after>` 命名（例如 `"r2"`）；`migration` 時為 `null` |

規則：

- `provenance` 記錄**建立**該元件的 patch；之後的修改只記在 `oplog.jsonl`，不改寫 `provenance`。
- agent 不能自己寫 `provenance`：LLM 可見的 `add_component` payload 中沒有這個欄位（§8.2），協調器在套用時填入。
- **`origin` 不再是元件欄位。** `origin` 只是埠名（§3.4）。文件中若出現元件欄位 `origin`，以未知欄位拒絕。

### 3.2 獨立元件與衍生元件

| 類別 | 成員 | 位置自由度 |
|---|---|---|
| 獨立元件 | Dot `point`；Plane `rect`、`disc`；Volume `plate`、`cylinder`、`ring`、`l_bracket`、`enclosure`；Empty `box`、`cylinder`、`hole` | 自有座標框：3 個平移未知數（每個世界軸一個）加姿態 |
| 衍生元件 | Line 全部；Plane `profile`、`section`；Volume `extrude`、`revolve`；Empty `of` | 沒有自己的未知數；位置與姿態完全由來源元件決定 |

衍生元件的埠可以出現在約束中作為「已知端」，但約束永遠不會指定衍生元件的位置（§5.5）。

### 3.3 形狀

**Empty**（必須帶 `role`）

| `shape.kind` | 欄位 | 可綁定欄位 |
|---|---|---|
| `box` | `length`、`width`、`height`（`Dim`） | 三者 |
| `cylinder` | `diameter`、`height`（`Dim`） | 兩者 |
| `hole` | `axis`（`"x"`／`"y"`／`"z"`，預設 `"z"`）、`diameter`、`hole_type`（同 1.0 六種）、`depth`、`thread`、`counterbore_diameter`、`counterbore_depth`、`countersink_diameter`、`countersink_angle` | `diameter`、`depth`、`counterbore_diameter`、`counterbore_depth`、`countersink_diameter`、`countersink_angle`（欄位為 `null` 時不可綁定） |
| `of` | `source`（Volume 的 `id`） | 無 |

`hole` 沿用 1.0 `HoleFeature.validate_subtype` 的全部規則（盲孔需 `depth`、沉孔需兩個沉孔欄位且直徑大於孔徑，依此類推）。`thread` 收緊為 `^M\d{1,3}(\.\d)?(x\d(\.\d{1,2})?)?$`（例如 `M6`、`M6x1`、`M8x1.25`）。

**Dot**

| `shape.kind` | 欄位 | 可綁定欄位 |
|---|---|---|
| `point` | 無 | 無 |

**Line**（全為衍生元件）

| `shape.kind` | 欄位 | 可綁定欄位 |
|---|---|---|
| `segment` | `start`、`end`（Dot `id`） | 無 |
| `polyline` | `points`（Dot `id`，2–64 個）、`closed`（bool） | 無 |
| `arc` | `start`、`through`、`end`（Dot `id`） | 無 |
| `circle` | `center`（Dot `id`）、`diameter` | `diameter` |

**Plane**

| `shape.kind` | 欄位 | 可綁定欄位 |
|---|---|---|
| `rect` | `length`、`width` | 兩者 |
| `disc` | `diameter` | `diameter` |
| `profile` | `outer`（封閉 Line `id`）、`inners`（封閉 Line `id`，最多 16） | 無 |
| `section` | `source`（Volume `id`）、`offset`（`Coord`） | `offset` |

**Volume**

| `shape.kind` | 欄位 | 可綁定欄位 |
|---|---|---|
| `plate` | `length`、`width`、`thickness` | 全部 |
| `cylinder` | `diameter`、`height` | 全部 |
| `ring` | `outer_diameter`、`inner_diameter`、`height` | 全部 |
| `l_bracket` | `width`、`depth`、`vertical_height`、`thickness` | 全部 |
| `enclosure` | `length`、`width`、`height`、`wall_thickness` | 全部 |
| `extrude` | `profile`（Plane `id`）、`height` | `height` |
| `revolve` | `profile`（Plane `id`）、`axis`（`segment` Line `id`）、`angle`（`0 < angle ≤ 360`） | `angle` |

五種捷徑形狀與 1.0 的 `PlateBase` 等欄位與驗證規則完全相同（含 `RingBase`、`LBracketBase`、`EnclosureBase` 的 model validator）。

### 3.4 埠表（凍結）

埠由 `shape.kind` 固定推導，agent 只能引用，不能宣告。下表的座標是**元件區域座標**；世界座標 = 姿態 R × 區域座標 + 平移 t（§5.4、§5.5）。

埠的四種型別：

| 型別 | 幾何內容 |
|---|---|
| `point` | `position` |
| `axis` | `position`、`direction`（`AxisDir`） |
| `face` | `origin`（面中心）、`normal`、`u`、`v`（`AxisDir`，`u × v = normal`）、`extent: [沿 u 長度, 沿 v 長度]` |
| `region` | 軸對齊包圍盒 `min`、`max`，加上形狀引用（干涉判定時使用實際形狀） |

**盒狀形狀**：Volume `plate`（高 = `thickness`）、`enclosure`（高 = `height`）、`l_bracket`（見下）、Empty `box`（高 = `height`）。以 `L` = 長（X）、`W` = 寬（Y）、`H` = 高（Z），原點在底面中心：

| 埠 | 型別 | 區域幾何 |
|---|---|---|
| `origin` | point | `(0, 0, 0)`；**座標框埠**（`fixed` 的 `a`） |
| `center` | point | `(0, 0, H/2)` |
| `axis_z` | axis | 位置 `(0, 0, 0)`，方向 `+z` |
| `top` | face | 中心 `(0, 0, H)`，法向 `+z`，u `+x`，v `+y`，範圍 `[L, W]` |
| `bottom` | face | 中心 `(0, 0, 0)`，法向 `-z`，u `+x`，v `-y`，範圍 `[L, W]` |
| `x_pos` | face | 中心 `(L/2, 0, H/2)`，法向 `+x`，u `+y`，v `+z`，範圍 `[W, H]` |
| `x_neg` | face | 中心 `(−L/2, 0, H/2)`，法向 `-x`，u `-y`，v `+z`，範圍 `[W, H]` |
| `y_pos` | face | 中心 `(0, W/2, H/2)`，法向 `+y`，u `-x`，v `+z`，範圍 `[L, H]` |
| `y_neg` | face | 中心 `(0, −W/2, H/2)`，法向 `-y`，u `+x`，v `+z`，範圍 `[L, H]` |
| `region` | region | `[−L/2, −W/2, 0]`–`[L/2, W/2, H]` |

`enclosure` 另有（`w` = `wall_thickness`）：

| 埠 | 型別 | 區域幾何 |
|---|---|---|
| `cavity_floor` | face | 中心 `(0, 0, w)`，法向 `+z`，u `+x`，v `+y`，範圍 `[L − 2w, W − 2w]` |
| `cavity` | region | `[−L/2 + w, −W/2 + w, w]`–`[L/2 − w, W/2 − w, H]` |

`l_bracket`（`W` = `width` 沿 X、`D` = `depth` 沿 Y、`t` = `thickness`、`h` = `vertical_height`）的盒狀埠以其包圍盒 `L = W`、寬 = `D`、`H = h + t` 計算（與 1.0 `_extents()` 相同），另有：

| 埠 | 型別 | 區域幾何 |
|---|---|---|
| `base_top` | face | 中心 `(0, t/2, t)`，法向 `+z`，u `+x`，v `+y`，範圍 `[W, D − t]` |
| `upright_front` | face | 中心 `(0, −D/2 + t, t + h/2)`，法向 `+y`，u `-x`，v `+z`，範圍 `[W, h]` |

**圓柱狀形狀**：Volume `cylinder`、`ring`、Empty `cylinder`。`D` = 直徑（`ring` 用 `outer_diameter`）、`H` = `height`：

| 埠 | 型別 | 區域幾何 |
|---|---|---|
| `origin` | point | `(0, 0, 0)`；座標框埠 |
| `center` | point | `(0, 0, H/2)` |
| `axis` | axis | 位置 `(0, 0, 0)`，方向 `+z` |
| `top` | face | 中心 `(0, 0, H)`，法向 `+z`，u `+x`，v `+y`，範圍 `[D, D]` |
| `bottom` | face | 中心 `(0, 0, 0)`，法向 `-z`，u `+x`，v `-y`，範圍 `[D, D]` |
| `region` | region | `[−D/2, −D/2, 0]`–`[D/2, D/2, H]` |

`ring` 另有 `bore`（region）：`[−d/2, −d/2, 0]`–`[d/2, d/2, H]`，`d` = `inner_diameter`。

**Empty `hole`**：

| 埠 | 型別 | 幾何 |
|---|---|---|
| `entry` | point | `(0, 0, 0)`；座標框埠 |
| `axis` | axis | 位置 = `entry`，方向 = 解出的切除方向（§7.1） |
| `region` | region | 從 `entry` 到主孔終點（盲孔為 `entry + depth`，其他為零件包圍盒的遠端邊界）、直徑取 `max(diameter, counterbore_diameter, countersink_diameter)` 的圓柱之包圍盒；不含 §7.2 的延伸餘量 |

**Empty `of`**：沿用 `source` Volume 的全部埠與座標。

**Dot `point`**：`point`（point，`(0, 0, 0)`；座標框埠）。

**Line**（衍生，世界座標直接由來源 Dot 求得）：

| `shape.kind` | 埠 |
|---|---|
| `segment` | `start`、`end`、`mid`（point）、`axis`（axis：位置 `start`，方向為 `start → end`，必須軸對齊，否則 `profile_not_axis_aligned`） |
| `polyline` | `start`、`end`（point） |
| `arc` | `start`、`end`、`center`（point） |
| `circle` | `center`（point）、`axis`（axis：位置 `center`，方向 = 圓所在平面的法向，預設 `+z`） |

**Plane**：

| `shape.kind` | 埠 | 備註 |
|---|---|---|
| `rect`、`disc` | `center`（point，座標框埠）、`normal`（axis，`+z`）、`face`（face，中心 `(0,0,0)`，法向 `+z`，u `+x`，v `+y`） | 獨立元件 |
| `profile` | 同上 | 衍生：`outer` 的所有點必須落在同一個軸對齊平面（否則 `profile_not_axis_aligned`）；`center` = 外輪廓點的包圍盒中心；`normal` 依外輪廓從法向正側看為逆時針決定 |
| `section` | 同上 | 衍生：`source` 在其區域 Z = `offset` 的水平截面，法向 `+z`（世界方向隨來源姿態） |

**衍生 Volume**：`extrude` 的座標框 = `profile` 的 `face` 座標框（法向為 +Z），埠表同盒狀形狀但以包圍盒計算，`top`／`bottom` 為拉伸的兩端面；`revolve` 只有 `origin`（= `axis` 線段的 `start`）與 `region`。

---

## 4. 參數與綁定

### 4.1 Parameter

| 欄位 | 型別 | 說明 |
|---|---|---|
| `name` | `Id`（N3），不可為 `min`／`max` | |
| `value` | `float` | |
| `min` | `float \| null` | |
| `max` | `float \| null` | |
| `note` | `str \| null`，最長 120 | |

`value` 超出自身 `min`／`max` 即 `parameter_out_of_range`。

### 4.2 Binding

| 欄位 | 型別 | 說明 |
|---|---|---|
| `target` | `id:field`（§1.3） | 被綁定的欄位；同一 `target` 最多一筆，否則 `duplicate_binding` |
| `expr` | `str`，1–160 | 封閉文法，見下 |

### 4.3 運算式文法（封閉）

```ebnf
expr    = term , { ("+" | "-") , term } ;
term    = unary , { ("*" | "/") , unary } ;
unary   = [ "-" ] , primary ;
primary = number
        | field_ref
        | param
        | ("min" | "max") , "(" , expr , { "," , expr } , ")"
        | "(" , expr , ")" ;
number    = digit , { digit } , [ "." , digit , { digit } ] ;   (* 不支援指數、前導小數點 *)
param     = ident ;                                              (* 不可為 min／max *)
field_ref = ident , ":" , ident ;
ident     = lower , { lower | digit | "_" } ;                    (* 總長 ≤ 32 *)
```

- 允許空白；其餘字元（含 `.`、`_` 開頭、引號、`**`、`%`、`[`、`=`）一律 `binding_parse_error`。
- `min`／`max` 至少一個參數，最多 8 個。
- 由自寫的遞迴下降解析器求值（Rust core），**不使用** `eval`、`exec` 或任何通用運算式引擎。
- 裸名稱一律是參數；找不到即 `binding_unknown_name`。`id:field` 找不到或不是可綁定欄位即 `unknown_field`。
- 除以零或結果非有限數即 `binding_eval_error`。

### 4.4 求值語意

1. 被綁定的欄位**仍保留一個數值**作為快取。正規化器每次重算全部綁定並覆寫快取值；`spec.json` 中的快取值必須等於重算結果（容差 `1e-6`）。
2. 綁定依引用相依的拓撲順序求值；`expr` 中引用的 `id:field` 若本身沒有綁定，取其目前的數值。綁定之間互相依賴即 `binding_cycle`。
3. 結果超出欄位本身的範圍（例如 `Dim` 必須 > 0，或 1.0 model validator 的規則）即 `binding_out_of_range`。
4. 被綁定的欄位不能用 `set_field` 修改（patch 拒絕原因 `field_is_bound`）；必須先 `remove_binding`（可以在同一個 patch 中）。`remove_binding` 後欄位保留最後的快取值作為普通數值。

---

## 5. 約束

### 5.1 Constraint 欄位

| 欄位 | 型別 | 說明 |
|---|---|---|
| `id` | `Id`（N1） | |
| `kind` | `fixed`／`coincident`／`on_face`／`coaxial`／`mate`／`flush`／`distance`／`inside`／`clearance` | |
| `a` | `id.port` | |
| `b` | `id.port \| null` | `fixed` 時必須為 `null`，其餘必填 |
| `value` | `Coord \| null` | `mate`（預設 0）、`flush`（預設 0）、`distance`、`clearance` 使用；其他 kind 必須為 `null` |
| `u`、`v` | `Coord \| null` | 只有 `on_face` 使用（預設 0） |
| `frame` | `Frame \| null` | 只有 `fixed` 使用且必填 |

`Frame`：`{origin: [Coord, Coord, Coord], x_axis: AxisDir, z_axis: AxisDir}`，`x_axis` 必須垂直於 `z_axis`，否則 schema 錯誤；`y_axis = z_axis × x_axis`。

### 5.2 約束種類與正負號約定

記號：`p` 為 point 埠位置；`o`、`n`、`û`、`v̂` 為 face 埠的中心、法向與 u、v 方向；`q`、`d` 為 axis 埠的位置與方向。全部是**世界座標**。

| `kind` | 埠型別 a–b | 方程式（凍結） | 姿態要求 | 約束的世界軸 |
|---|---|---|---|---|
| `fixed` | 座標框埠–無 | a 的元件座標框 = `frame` | 姿態 = `frame` | x、y、z |
| `coincident` | point–point | `p_a = p_b` | 無 | x、y、z |
| `on_face` | point–face | `p_a = o_b + u·û_b + v·v̂_b` | 無 | x、y、z |
| `coaxial` | axis–axis | `q_a` 與 `q_b` 在垂直於 `d_b` 的兩軸上相等 | `d_a = ±d_b` | 垂直於 `d_b` 的兩軸 |
| `mate` | face–face | `(o_a − o_b)·n_b = value`，`value ≥ 0` | `n_a = −n_b` | `n_b` 所在軸 |
| `flush` | face–face | `(o_a − o_b)·n_b = value` | `n_a = n_b` | `n_b` 所在軸 |
| `distance` | point–face | `(p_a − o_b)·n_b = value` | 無 | `n_b` 所在軸 |
| `distance` | face–face | `(o_a − o_b)·n_b = value` | `n_a = ±n_b` | `n_b` 所在軸 |
| `inside` | region–region | a 完全位於 b 之內 | 無 | 無（檢查型） |
| `clearance` | region–region | a 與 b 的最小間距 ≥ `value` | 無 | 無（檢查型） |

正負號約定：

- 一律**沿 b 的法向量**量測，正值表示 a 位於 b 的法向正側（「外側」）。
- `mate` 的 `value` 是間隙，必須 ≥ 0（< 0 為 schema 錯誤），表示兩面相對、中間留 `value` 的空隙；`value = 0` 即貼合。
- `flush` 的 `value` 可為負；`value = 0` 表示兩面共面且同向。
- `distance` 的 `value` 可為負（a 位於 b 的內側）。範例 B 的 `k_pcb_z`：`e_pcb.bottom` 的中心 z = 7，`v_body.cavity_floor` 的中心 z = 2、法向 `+z`，`(7 − 2)·1 = 5`。

### 5.3 埠相容性矩陣

| a ＼ b | `point` | `axis` | `face` | `region` |
|---|---|---|---|---|
| `point` | `coincident` | — | `on_face`、`distance` | — |
| `axis` | — | `coaxial` | — | — |
| `face` | — | — | `mate`、`flush`、`distance` | — |
| `region` | — | — | — | `inside`、`clearance` |

「—」的組合即 `port_type_mismatch`。`fixed` 的 `a` 必須是**獨立元件的座標框埠**（盒狀與圓柱狀為 `origin`、Dot 為 `point`、`hole` 為 `entry`、`rect`／`disc` 為 `center`），否則 `invalid_fixed_target`。

### 5.4 姿態

每個元件的姿態是 24 種軸對齊姿態之一，以 `(x_axis, z_axis)` 表示。標準列舉順序：`z_axis` 依 `+z, -z, +x, -x, +y, -y`，每個 `z_axis` 之下 `x_axis` 依 `+x, -x, +y, -y, +z, -z` 並略過不垂直者。第一個是恆等姿態 `(+x, +z)`。

姿態決定順序（全部確定、與初值無關）：

1. 有 `fixed` 的元件：姿態 = `frame`。
2. Dot：恆為恆等（點沒有方向）。
3. Empty `hole`：等於其所屬零件 `body[0]` Volume 的姿態（孔隨零件旋轉）；不屬於任何零件時為恆等。
4. 衍生元件：由來源決定。
5. 其餘獨立元件：依文件順序反覆掃描具姿態要求的約束（`coaxial`、`mate`、`flush`、face–face `distance`），直到沒有變化。若一端元件的姿態已知、另一端是姿態未知的獨立元件，則取標準列舉中**第一個**同時滿足「該元件與所有姿態已知元件之間全部姿態要求」的姿態。找不到即 `orientation_unsatisfiable`。
6. 仍未決定者取恆等姿態，不產生問題。
7. 全部決定後逐筆複核姿態要求，不滿足者 `orientation_unsatisfiable`。

### 5.5 平移求解：不動點規則

因為姿態軸對齊，每個埠在世界軸 k 上的座標只取決於擁有者在 k 軸上的平移：`world_k = t_k + (R·local)_k`。因此每筆約束可拆成數個**單軸方程式** `X_a,k − X_b,k = c`，三個軸互不耦合。

**已知性**：獨立元件 c 在軸 k 上「已知」，當且僅當 `t_{c,k}` 已被指定。衍生元件的埠在軸 k 上已知，當且僅當它所有來源獨立元件在軸 k 上都已知。

演算法：

1. 前置：任何獨立元件存在而文件中沒有任何 `fixed` → `no_fixed_root`（error），不求解。
2. **根**：依文件順序處理每筆 `fixed`：若其元件的三軸尚未指定，指定為 `frame.origin` 並記錄「由此約束指定」；若已指定（同一元件有第二筆 `fixed`），該筆的三個方程式視為未使用，留待第 5 步判定。
3. **傳播**：依文件順序掃描所有非 `fixed` 約束的每個單軸方程式：
   - 一端已知、另一端未知，且未知端屬於**單一獨立元件**（不是衍生元件，也不是自己）→ 由方程式指定該元件在此軸的平移，標記此方程式「已使用」並記錄指定來源。
   - 其他情況（兩端皆已知、兩端皆未知、未知端為衍生元件）→ 跳過。
   整輪掃描沒有任何新指定時停止；否則再掃一輪。
4. **欠約束預設**：若仍有獨立元件存在未知軸，取文件順序中**第一個**這樣的元件，把它的未知軸設為 0（世界原點），發出 `under_constrained`（warning，附元件 `id` 與軸），然後回到第 3 步。直到所有獨立元件三軸皆已知。求解器**不改寫** `spec.json` 的 `assumptions`；這個預設只出現在驗證報告與 `resolved.json` 的 `defaulted_axes`，`warning` 本身已使 `review_required = true`。
5. **冗餘與矛盾**：對每筆約束中所有「未使用」的單軸方程式（此時兩端必然已知），計算殘差 `|X_a,k − X_b,k − c|`：
   - 全部 ≤ `1e-6` → `redundant_constraint`（info，附軸）。
   - 任一 > `1e-6` → `conflicting_constraints`（error，`target` 為此約束，`related` 為指定兩端該軸的約束 `id`，訊息附殘差）。
   - 兩端屬於同一個獨立元件的方程式一律走這一步（它是常數檢查）。
6. 檢查型約束（`inside`、`clearance`）與空的隱含檢查在全部位置確定後執行。

性質：結果只取決於文件內容與文件順序；同一份文件重複求解，`resolved.json` 位元組相同。循環的約束圖（範例 B 的上蓋同時以 `coaxial` 與 `mate` 連到本體）不是錯誤：每個單軸方程式要嘛被用來指定，要嘛在第 5 步被判定為冗餘或矛盾。**`constraint_cycle` 因此退役。**

### 5.6 `resolved.json`

求解結果是衍生資料，寫入工作目錄的 `resolved.json`，不寫回 `spec.json`：

```json
{
  "schema_version": "2.0",
  "revision": 5,
  "components": {
    "v_lid": {"origin": [0, 0, 33], "x_axis": "+x", "z_axis": "+z", "defaulted_axes": []}
  },
  "ports": {
    "v_lid.bottom": {"type": "face", "origin": [0, 0, 33], "normal": "-z", "u": "+x", "v": "-y", "extent": [100, 70]},
    "e_pcb.region": {"type": "region", "min": [-40, -25, 7], "max": [40, 25, 32]}
  },
  "holes": {
    "e_h1": {"direction": "-z", "cutter_start": [-50, -20, 11], "cutter_end": [-50, -20, -1], "diameter": 6.6}
  },
  "issues": []
}
```

`ports` 收錄文件中被約束引用的埠與每個元件的座標框埠；`holes` 收錄 §7 解出的切除方向與主孔切刀範圍；`issues` 只收錄求解器本身的診斷（§5.4、§5.5、§7.1 的代碼），其餘驗證問題在 `validation.json`。編譯器只讀 `resolved.json` 的座標，不自行求解。

---

## 6. 零件、組裝與布林

### 6.1 `body` 布林

`BodyOp`：`{op: "add" | "cut" | "intersect", component: Id}`。

| `op` | 可用元件 | 語意 |
|---|---|---|
| `add` | Volume | 聯集 |
| `cut` | `role = "cut"` 的 Empty | 減除 |
| `intersect` | Volume，或 `role = "envelope"` 的 Empty | 交集（以包絡裁切成品） |

- `body[0]` 必須是 `add`，否則 `body_first_not_add`。
- 其他組合（例如 `cut` 一個 `reserve`、`intersect` 一個 `keep_out`）即 `invalid_body_component`。
- 每個 Volume 與 `cut` Empty 最多屬於一個零件的 `body`（`multi_owner`）；不屬於任何零件即 `orphan_component`（warning）。`envelope` Empty 可被多個零件 `intersect` 引用，不受 `multi_owner` 限制。

### 6.2 Part 與 Assembly

`Part`：`{id, name (1–80), material (同 1.0 列舉或 null), quantity (1–999), source ("made" | "purchased"), body (1–128), fillets (≤8), chamfers (≤8)}`；`fillets`／`chamfers` 的型別與 1.0 相同。

`Assembly`：`{id, name (1–80), children (1–32 個 N2 的 id)}`。組裝圖必須是樹（每個子項最多一個父組裝），深度 ≤ 4。

**零件座標框** = 其 `body[0]` 元件解出的座標框。編譯器在零件座標框內建立零件幾何，組裝時才套用 `cq.Location`，因此逐零件 STEP／STL 是擺正的。

### 6.3 `root`

`root` 由協調器在每次接受 patch 後重算，agent 不能設定：

- 有組裝時：`root` = 唯一一個不是任何組裝之子項的組裝；有兩個以上或為零個即 `invalid_root`。
- 沒有組裝且恰有一個零件時：`root` = 該零件。
- 其他情況（多個零件但沒有組裝）：`invalid_root`。

人工編輯的 `spec.json` 中 `root` 若與上述規則不符，同樣回報 `invalid_root`。

---

## 7. 孔的方向與切刀

### 7.1 切除方向

對每個 `role = "cut"`、`shape.kind = "hole"` 的 Empty `e`：

1. 候選面：所有 `kind = "on_face"` 且 `a` 為 `e.entry` 的約束之 `b` 面；以及所有與 `e.entry` 以 `coincident` 直接相連（任一端）的 Dot `d`，其 `d.point` 為 `a` 的 `on_face` 約束之 `b` 面。只看一層，不做遞移。
2. 若有候選面：切除方向（向內）= −`n`（該面的世界法向）。候選面的法向若不全相同 → `hole_direction_conflict`（error）。
3. 若沒有候選面：切除方向 = −（`shape.axis` 經孔姿態轉換後的世界方向），例如姿態恆等、`axis = "z"` 時為 `-z`。這與 1.0「從該軸正向外表面向內建立」一致。
4. 解出的方向必須與 `shape.axis` 的世界方向平行，否則 `hole_axis_mismatch`（error）。

`e.axis` 埠的方向即此切除方向。

### 7.2 深度與切刀範圍

| 孔／特徵 | 切刀 | 與 1.0 的關係 |
|---|---|---|
| 主孔，`hole_type = "blind"` | 自 `entry` 向外退 0.1 mm 起，沿切除方向長 `depth + 0.2` | 同 `_main_hole_cutter()` 盲孔分支 |
| 主孔，其他五種（`through`、`clearance`、`tapped`、`counterbore`、`countersink`） | 沿切除方向跨越**所屬零件的包圍盒**（`body` 中全部 `add` Volume 的世界包圍盒），兩端各延伸 1.0 mm；側向位置取 `entry` | 同 `_main_hole_cutter()` 貫穿分支 |
| 沉孔 `counterbore` | 自 `entry` 向外退 0.1 mm 起，長 `counterbore_depth + 0.2` | 同 `_hole_lines()` |
| 沉頭 `countersink` | 圓錐自 `entry` 起（**無**退刀），高 `(countersink_diameter − diameter) / (2·max(0.01, tan(angle/2)))` | 同 `_hole_lines()` |

- `depth` 一律自 `entry` 沿切除方向量測。
- 「通孔」不再需要 1.0 的全域 `max_z`：它的意義是「貫穿所屬零件的包圍盒」。
- 範例 A：`e_h1` 入口在 `(−50, −20, 10)`、方向 `-z`、零件包圍盒 z ∈ [0, 10]，切刀從 z = 11 到 z = −1。

---

## 8. Patch 協定

### 8.1 Patch

| 欄位 | 型別 | 說明 |
|---|---|---|
| `agent` | `"space_planner"`／`"component_smith"`／`"joiner"`／`"assembler"` | `human` 不在此列舉中，只能由協調器依請求來源蓋章 |
| `base_revision` | `int ≥ 0` | 必須等於目前 `revision` |
| `rationale` | `str`，1–500 | |
| `resolves` | `str[]`，≤ 16 | 本 patch 要處理的驗證問題代碼 |
| `ops` | `PatchOp[]`，1–64 | |

### 8.2 PatchOp：以 `op` 判別的封閉聯集

每個變體的欄位固定、全部必填（可為 `null` 者另行標註），沒有任意 JSON 值，因此能以 strict `json_schema` 模式（`additionalProperties: false`）表達。

| `op` | 欄位 | 語意 |
|---|---|---|
| `add_component` | `component: ComponentInput` | 新增元件；`ComponentInput` 即 §3 的 `Component` 去掉 `provenance`；`id` 不得已存在於 N1 |
| `remove_component` | `id: Id` | 移除元件；以它為目標的綁定一併移除；仍被其他約束、`shape`、`body` 引用則整個 patch 以 `dangling_reference` 拒絕（不連鎖刪除他人的資料） |
| `set_field` | `target: id:field`、`value: number` | 設定元件數值欄位或約束的 `value`／`u`／`v`；目標已被綁定則拒絕（`field_is_bound`） |
| `set_binding` | `target: id:field`、`expr: str` | 新增或取代該目標的綁定，立即求值並覆寫快取 |
| `remove_binding` | `target: id:field` | 移除綁定，欄位保留最後的快取值 |
| `add_constraint` | `constraint: Constraint` | 新增約束；`id` 不得已存在於 N1 |
| `remove_constraint` | `id: Id` | 移除約束；以它為目標的綁定一併移除 |
| `set_parameter` | `parameter: Parameter` | 依 `name` 新增或取代參數 |
| `add_part` | `part: Part` | 新增零件；`id` 不得已存在於 N2 |
| `set_body` | `part_id: Id`、`body: BodyOp[]` | 取代既有零件的 `body` |
| `add_assembly` | `assembly: Assembly` | 新增組裝 |
| `add_assumption` | `text: str`（1–300） | 附加一條假設 |
| `remove_assumption` | `text: str`（1–300） | 移除完全相同的一條假設；找不到則拒絕（`assumption_not_found`）；限制見 §8.7 |
| `add_note` | `text: str`（1–300） | 附加一條備註 |

Rust core 的型別草圖（規範以上表為準）：

```rust
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum PatchOp {
    AddComponent { component: ComponentInput },
    RemoveComponent { id: Id },
    SetField { target: FieldRef, value: f64 },
    SetBinding { target: FieldRef, expr: Expr },
    RemoveBinding { target: FieldRef },
    AddConstraint { constraint: Constraint },
    RemoveConstraint { id: Id },
    SetParameter { parameter: Parameter },
    AddPart { part: Part },
    SetBody { part_id: Id, body: Vec<BodyOp> },
    AddAssembly { assembly: Assembly },
    AddAssumption { text: Text300 },
    RemoveAssumption { text: Text300 },
    AddNote { text: Text300 },
}
```

2.0 刻意不提供 `remove_part`、`remove_assembly`、`set_part`（修改材質、數量、圓角）；需要時由人工編輯，或在 2.1 追加（追加 op 不破壞既有 patch）。

### 8.3 每個角色的 schema 子集

送給 LLM 的 schema 是上述聯集的**子集聯集**（ADR 0005「具型別的領域操作」）：每個角色只看得到自己可用的 `op`，且 `add_component` 的 `ComponentInput` 也縮小到該角色可建立的 `kind`／`role`：

| 角色 | 可用 `op` | `ComponentInput` 子集 |
|---|---|---|
| `space_planner` | `add_component`、`remove_component`、`set_field`、`set_binding`、`remove_binding`、`set_parameter`、`add_assumption`、`remove_assumption`、`add_note` | Empty，`role ∈ {reserve, keep_out, envelope}` |
| `component_smith` | `add_component`、`remove_component`、`set_field`、`set_binding`、`remove_binding`、`set_parameter`、`add_assumption`、`remove_assumption`、`add_note` | Dot、Line、Plane、Volume、Empty `role = cut` |
| `joiner` | `add_constraint`、`remove_constraint`、`set_field`、`set_binding`、`remove_binding`、`add_assumption`、`remove_assumption`、`add_note` | 無 |
| `assembler` | `add_part`、`set_body`、`add_assembly`、`add_assumption`、`remove_assumption`、`add_note` | 無 |

schema 子集只是降低產生錯誤的機率；**權限仍由協調器依 §8.5 逐筆強制**，不依賴 schema 或提示詞。

### 8.4 欄位分類

`set_field`／`set_binding`／`remove_binding` 的權限取決於目標所屬的類別：

| 類別 | 目標 |
|---|---|
| 保留類 | `role ∈ {reserve, keep_out, envelope}` 的 Empty 的欄位 |
| 實體類 | 其他元件（Dot、Line、Plane、Volume、`cut` Empty）的欄位 |
| 約束類 | 約束的 `value`、`u`、`v` |

### 8.5 權限矩陣

| patch 操作 | 空間規劃師 | 元件工匠 | 接合師 | 組裝師 |
|---|---|---|---|---|
| `set_parameter` | 可 | 可（限 `resolves` 非空的修正 patch） | 否 | 否 |
| `add_component`：保留類 Empty | 可 | 否 | 否 | 否 |
| `add_component`：實體類 | 否 | 可 | 否 | 否 |
| `remove_component`、`set_field`、`set_binding`、`remove_binding`：保留類 | 可 | 否 | 否 | 否 |
| `remove_component`、`set_field`、`set_binding`、`remove_binding`：實體類 | 否 | 可 | 否 | 否 |
| `add_constraint`、`remove_constraint`；`set_field`、`set_binding`、`remove_binding`：約束類 | 否 | 否 | 可 | 否 |
| `add_part`、`set_body`、`add_assembly` | 否 | 否 | 否 | 可 |
| `add_assumption`、`add_note` | 可 | 可 | 可 | 可 |
| `remove_assumption` | 可（§8.7） | 可（§8.7） | 可（§8.7） | 可（§8.7） |

- **綁定的權限等同 `set_field`**：誰能改這個欄位，誰才能綁定或解除綁定它。`expr` 可以引用任何參數與欄位，引用不需要權限。
- 範例 B 的回合 6 中，元件工匠以 `set_binding` 綁定 `v_body:height`（實體類，可）；它若嘗試修改 `e_pcb:height`（保留類）會被拒絕，「把保留區縮小」這條捷徑在權限層就被擋掉。
- `human` 不受此矩陣限制。

### 8.6 套用步驟與拒絕原因

協調器套用 patch，全部或全無：

1. 以該角色的 `Patch` schema 驗證（`schema_invalid`）。
2. `base_revision` 必須等於目前 `revision`（`stale_revision`）。
3. 逐筆檢查 §8.5（`permission_denied`）與 §8.7。
4. 在文件副本上**依序**套用全部 op（後面的 op 看得到前面 op 的結果）；目標不存在即 `unknown_target`。
5. 正規化：重算綁定、重算 `root`、蓋 `provenance`。
6. 執行驗證：§9.2 中「patch 阻擋類」的任何 error 都使整個 patch 被拒絕；「設計類」問題不阻擋 patch，而是回饋給修正迴圈。標準件複核（例如 M6 間隙孔徑以 `standards.py` 的表列值複核）不符時拒絕（`standards_mismatch`）。
7. 接受則 `revision + 1`，寫入 `oplog.jsonl`；拒絕則文件不變，同樣寫入日誌（附拒絕原因代碼）。

`components`、`parts` 的下限（≥ 1）與 `root` 的存在只在「完成文件」時檢查（迴圈結束、或經 `/validate`、`/generate-from-spec` 輸入時），中間修訂允許暫缺。

### 8.7 假設的移除

- 協調器**永遠不自動移除**假設（修正概念文件 §12.2「由協調器在綁定建立時移除」的敘述）。
- 被取代的假設只能由**同一個 patch** 中明確的 `remove_assumption` 移除，而且該 patch 必須同時包含至少一個設計 op（`add_assumption`、`remove_assumption`、`add_note` 以外的任何 op），否則拒絕（`annotation_only_removal`）。這防止 agent 只為了讓 `review_required` 消失而刪除假設。
- 所有移除都留在 `oplog.jsonl`，人類審查時可見。

---

## 9. 驗證

### 9.1 ValidationIssue 2.0

1.0 的 `ValidationIssue`（`severity`、`code`、`message`、`feature_index`）新增四個可選欄位，1.0 報告不受影響：

| 欄位 | 型別 | 說明 |
|---|---|---|
| `target` | `str \| null` | 元件、約束、零件或綁定目標的 `id`／`id:field` |
| `related` | `str[]` | 其他涉及的 `id` |
| `owner` | 角色或 `null` | 應處理此問題的角色（下表） |
| `layer` | `"analytic"`／`"kernel"`，預設 `"analytic"` | 產生此問題的驗證層 |

`message` 只能嵌入 `id`、數值與固定範本文字；`label`、`notes`、`assumptions` 等自由文字不得出現在會回饋給 agent 的訊息中。`valid` 與 `review_required` 的計算規則同 1.0。

### 9.2 代碼清單（凍結）

類別：**P** = patch 阻擋類（出現即拒絕 patch；人工輸入時阻擋渲染），**D** = 設計類（不拒絕 patch，`error` 阻擋渲染）。P 類問題的 `owner` 一律是提交該 patch 的角色。

**結構與引用**（解析層）

| 代碼 | 嚴重度 | 類別 | owner | 規則 |
|---|---|---|---|---|
| `duplicate_id` | error | P | 提交者 | §1.2 命名空間內重複 |
| `dangling_reference` | error | P | 提交者 | `shape`、約束、`body`、`children`、`root`、綁定引用的 `id` 不存在 |
| `unknown_port` | error | P | 提交者 | 埠名不在 §3.4 埠表 |
| `unknown_field` | error | P | 提交者 | `id:field` 不是可綁定欄位（§1.3、§3.3） |
| `reference_cycle` | error | P | 提交者 | 引用圖不是 DAG |
| `invalid_promotion` | error | P | 提交者 | 升維來源的 `kind` 錯誤 |
| `open_profile` | error | P | 提交者 | `profile.outer`／`inners` 不是封閉線 |
| `profile_not_axis_aligned` | error | P | 提交者 | 輪廓不在軸對齊平面上，或 `segment` 軸不是軸對齊 |
| `multi_owner` | error | P | 提交者 | §6.1 |
| `body_first_not_add` | error | P | 提交者 | §6.1 |
| `invalid_body_component` | error | P | 提交者 | §6.1 的 op／元件組合不合法 |
| `invalid_root` | error | P（完成文件時） | assembler | §6.3 |
| `port_type_mismatch` | error | P | 提交者 | 違反 §5.3 |
| `invalid_fixed_target` | error | P | 提交者 | `fixed.a` 不是獨立元件的座標框埠 |
| `orphan_component` | warning | D | assembler | Volume 或 `cut` Empty 不屬於任何零件 |

**參數與綁定**

| 代碼 | 嚴重度 | 類別 | owner | 規則 |
|---|---|---|---|---|
| `duplicate_binding` | error | P | 提交者 | 同一 `target` 有兩筆綁定 |
| `binding_parse_error` | error | P | 提交者 | `expr` 不符合 §4.3 |
| `binding_unknown_name` | error | P | 提交者 | 運算式中的參數不存在 |
| `binding_cycle` | error | P | 提交者 | 綁定互相依賴 |
| `binding_eval_error` | error | P | 提交者 | 除以零或非有限數 |
| `binding_out_of_range` | error | P | 提交者 | 結果超出欄位範圍或形狀驗證規則 |
| `parameter_out_of_range` | error | P | 提交者 | 參數值超出自身的 `min`／`max` |

**約束與求解**

| 代碼 | 嚴重度 | 類別 | owner | 規則 |
|---|---|---|---|---|
| `no_fixed_root` | error | D | joiner | §5.5 第 1 步 |
| `conflicting_constraints` | error | D | joiner | §5.5 第 5 步 |
| `under_constrained` | warning | D | joiner | §5.5 第 4 步 |
| `redundant_constraint` | info | D | joiner | §5.5 第 5 步 |
| `orientation_unsatisfiable` | error | D | joiner | §5.4 |
| `hole_direction_conflict` | error | D | joiner | §7.1 第 2 步 |
| `hole_axis_mismatch` | error | D | component_smith | §7.1 第 4 步 |

**空**

| 代碼 | 嚴重度 | 類別 | owner | 規則 |
|---|---|---|---|---|
| `reserve_violated` | error | D | component_smith | 任何零件實體與 `reserve` 區相交（附侵入量與方向） |
| `keep_out_violated` | error | D | component_smith | 同上，對象為 `keep_out` |
| `envelope_exceeded` | error | D | component_smith | 實體超出 `envelope` |
| `empty_cut_no_effect` | warning | D | component_smith | `cut` 空與所屬零件實體不相交 |
| `empty_cut_removes_all` | error | D | component_smith | `cut` 之後零件沒有剩餘材料 |
| `reserve_overlap` | warning | D | space_planner | 兩個 `reserve` 區重疊 |
| `reserve_unreachable` | info | D | space_planner | `reserve` 區被實體完全包住且沒有開口 |

**零件間**

| 代碼 | 嚴重度 | 類別 | owner | 規則 |
|---|---|---|---|---|
| `part_interference` | 解析層 warning；核心層 error | D | joiner | 兩個零件實體相交 |
| `clearance_violated` | error | D | joiner | `clearance` 約束不成立 |
| `inside_violated` | error | D | joiner | `inside` 約束不成立 |
| `floating_part` | warning | D | joiner | 零件與組裝中其他零件沒有任何接合 |

**沿用 1.0（改以元件 `id` 定位，邏輯不變，類別 D）**

`thin_wall`、`thin_bracket`、`extreme_aspect_ratio`、`hole_outside_part`、`hole_in_inner_void`、`hole_intersects_inner_void`、`low_edge_clearance`、`overlapping_holes`、`blind_depth_exceeds_material`、`counterbore_depth_exceeds_material`、`countersink_depth_exceeds_material`、`tapped_hole_without_thread`、`thread_annotation_only` 的 owner 為 component_smith；`fillet_too_large`、`chamfer_too_large` 的 owner 為 assembler；`assumption`（info）沒有 owner。

**核心層**（`layer = "kernel"`，類別 D）

| 代碼 | 嚴重度 | owner | 規則 |
|---|---|---|---|
| `brep_invalid` | error | component_smith | 零件實體 `isValid()` 為否 |
| `empty_solid` | error | component_smith | 布林後體積為零 |
| `part_interference` | error | joiner | 精確干涉確認 |
| `multi_body_part` | warning | component_smith | 一個零件布林後成為多個不相連實體 |
| `feature_skipped` | warning | assembler | 圓角或倒角在核心中失敗而被略過（沿用現有 `try`／`except` 行為） |

**patch 拒絕原因**（只出現在 `oplog.jsonl` 與回給 agent 的拒絕訊息，不是 ValidationIssue）：`schema_invalid`、`stale_revision`、`permission_denied`、`unknown_target`、`field_is_bound`、`assumption_not_found`、`annotation_only_removal`、`standards_mismatch`，以及任何 P 類代碼。

### 9.3 退役代碼

| 代碼 | 原定義 | 退役理由 |
|---|---|---|
| `constraint_cycle` | 約束圖無法拓撲排序 | 約束是無向的，拓撲排序沒有定義；§5.5 的不動點規則把循環分解為「指定」或「冗餘／矛盾」，循環本身不再是錯誤 |

退役代碼不得被重新使用於其他意義。

### 9.4 核心層錯誤語意

解析層在渲染前執行（驗證閘門），核心層在 renderer 子行程（Python sidecar，ADR 0006）內、幾何建立之後執行：

1. sidecar 的診斷使用與解析層**相同的 issue 結構**，每筆 `layer = "kernel"`，寫入工作目錄的 `kernel-validation.json`（格式同 `ValidationReport`），與 `render-warnings.json` 並列。
2. 核心層出現任何 `error`：
   - 工作 `status = "failed"`；`renderer_used` 仍記錄實際使用的 renderer（例如 `cadquery`），不是 `validation-blocked`。
   - **保留**來源產物：`spec.json`、`validation.json`（閘門當時的解析層報告，不改寫）、`resolved.json`、`model.py`、`model.scad`、`preview.svg`、`render-warnings.json`、`kernel-validation.json`、`oplog.jsonl`、`bom.json`。
   - **移除**幾何產物：`model.step`、`model.stl`、`model.dxf`、`model.svg`、`parts/*`、`spaces.step` 不出現在 manifest 的 `artifacts` 中，也不留在工作目錄，避免使用者下載無效幾何。
   - manifest 的 `warnings` 附一行固定範本：`kernel validation failed: <代碼清單>`。
3. 核心層只有 `warning`／`info`：`status` 照常（`completed`），幾何產物保留，`kernel-validation.json` 的 `review_required = true`。
4. 團隊模式：核心層 error 依 owner 回饋給修正迴圈，計入回合上限；renderer 不可用（`source_only`）時核心層不執行，報告中不產生核心層問題。

---

## 10. 1.0 遷移

遷移在 Rust core 實作（`upgrade_1_to_2`、`downgrade_2_to_1`，ADR 0005 第 6 項）。

### 10.1 `upgrade_1_to_2`（全函數）

| 1.0 | 2.0 |
|---|---|
| `base` | Volume `v_base`（`shape` 為同一物件）；`fixed` 約束 `k_base` 把 `v_base.origin` 固定在 `[0, 0, 0]`、恆等姿態 |
| `holes[i]`（`n = i + 1`） | **每孔 5 筆紀錄**：Dot `d_h{n}`；Empty `e_h{n}`（`role = cut`、`shape.kind = hole`，欄位照搬，去掉 `x`、`y`、`z`）；`fixed` 約束 `k_dh{n}`（`a = d_h{n}.point`）；`coincident` 約束 `k_eh{n}`（`e_h{n}.entry`–`d_h{n}.point`）；`body` 操作 `{op: cut, component: e_h{n}}` |
| `fillets`、`chamfers`、`material` | `parts[0]` 的同名欄位 |
| `name`、`source_prompt`、`unit`、`assumptions`、`notes`、`planner` | 原樣搬移 |
| 無 | `parts = [{id: "p_main", name: <name>, quantity: 1, source: "made", body: [add v_base, cut e_h1, …]}]`、`root = "p_main"`、`revision = 0`、所有元件 `provenance = {agent: "migration", patch_id: null}` |

孔點的位置：把入口 Dot 放在**正向外表面**上，讓 §7.1 的一般規則（沒有 `on_face` → 方向為 −`shape.axis`）自動重現 1.0 的切刀，不需要任何「舊版孔」旗標。以 1.0 `_extents()` 的 `(max_x, max_y, max_z)`：

| `axis` | `k_dh{n}.frame.origin` |
|---|---|
| `z` | `(x, y, max_z)` |
| `x` | `(max_x, y, z)` |
| `y` | `(x, max_y, z)` |

1.0 的沿軸座標（例如 Z 軸孔的 `z`）在 1.0 編譯器與驗證器中本來就不被使用，因此上表不改變幾何。

`thread` 正規化：`t.strip().upper().replace("X", "x")`（`" m6x1 "` → `"M6x1"`）。正規化後仍不符合 §3.3 的正規表示式（例如 `"M6 tapped"`）時，該孔 `thread = null`，原值合併為**一行**備註 `1.0 thread 原值：e_h1=<原值>；e_h3=<原值>` 附加到 `notes`；`notes` 已滿 32 筆時，附加到最後一筆之後。如此 `upgrade_1_to_2` 對任何合法 1.0 文件都成立，且不新增任何假設。

### 10.2 `downgrade_2_to_1`（部分函數）

只有下列條件全部成立時回傳 1.0 文件，否則回傳 `None`：單一零件；`body` 為一個捷徑 Volume 的 `add` 加上若干 `hole` 空的 `cut`；Volume 與所有孔的姿態為恆等，Volume 原點在世界原點；每個孔的切除方向等於 −`shape.axis`。

- 綁定可以存在：降級使用快取值，參數化關係不保留（降級本來就是有損的）。
- 孔點座標取自求解結果（因此 `on_face` 定位的孔也可降級），並把**沿軸座標投影回 0**（1.0 正規形式）。
- `holes` 的順序依 `body` 中 `cut` 的順序。

### 10.3 往返不變量

對**正規化的 1.0 文件**（每個孔的沿軸座標為 0；`thread` 已依 §10.1 正規化、不符者為 `null` 且原值已在 `notes`），`downgrade_2_to_1(upgrade_1_to_2(d)) == d`。以 property-based 測試涵蓋；`examples/generated/plate-four-holes/spec.json` 是第一個固定案例。

---

## 11. 範例與 `expected_resolved`

| 檔案 | 內容 |
|---|---|
| [`examples/cad2/plate-four-holes.cad2.json`](../examples/cad2/plate-four-holes.cad2.json) | 概念文件 §12.1 範例 A 的最終文件（修訂 4）：一個 `plate` 加四個 M6 間隙孔，孔點以 `on_face` 定位、偏移以綁定表達；另把板的三個尺寸綁定到 `len`、`wid`、`thk`，使修改參數時板與孔位一起變動 |
| [`examples/cad2/enclosure-with-lid.cad2.json`](../examples/cad2/enclosure-with-lid.cad2.json) | 概念文件 §12.2 範例 B 在修正回合之後的最終文件（修訂 5）：本體高度由綁定 `wall + standoff_h + pcb_stack + 1 = 33` 推導，上蓋原點在 z = 33，PCB 保留區佔 z = 7 到 32 |

兩個檔案都以凍結語法撰寫，並在頂層附加 **`expected_resolved`**：手算的求解結果（元件世界原點與姿態、關鍵埠座標、孔的切刀範圍、求解器問題），格式為 §5.6 `resolved.json` 的子集。

**`expected_resolved` 只是範例檔的註記，不屬於 CadDocument 2.0 schema。** 因為文件拒絕未知欄位，任何工具在載入範例前都必須先移除這個鍵；Rust core 的黃金測試以它比對求解器輸出。`tests/test_cad2_examples.py` 以純 Python（只用 `json` 與 `re`）檢查兩個範例：JSON 可解析、`id` 唯一且符合正規表示式、所有元件／埠／欄位引用都能解析（埠名必須在 §3.4 埠表內）、每個 `body` 以 `add` 開頭、元件使用 `provenance` 而沒有 `origin` 欄位、每筆綁定以封閉文法求值後等於欄位快取值，並以 §5.5 的不動點規則重算 `expected_resolved`。這個測試不是 2.0 的實作，只是凍結規格的紙上驗證。

---

## 12. 開放問題的建議預設值（待負責人簽核）

概念文件 §14.3 的十個問題目前**沒有**決定。以下是建議預設值，每一項都標示為 **待負責人簽核**；簽核前，第 2 階段的實作以這些預設值為準，但不得寫成不可逆的資料格式。

| # | 問題 | 建議預設值 | 狀態 |
|---|---|---|---|
| 1 | 空只做三維，或做成各維度的「負極性」 | 2.0 只做三維的空（`box`、`cylinder`、`hole`、`of`）；草圖上的負面（Plane `profile.inners`）已足以表達 2D 的孔；負極性留到 2.1 評估 | 待負責人簽核 |
| 2 | 是否只支援 24 種軸對齊姿態 | 是。斜撐與傾斜面板留到 2.1，屆時隨 ADR 0007 第 3 階段的數值求解器一起引入，並以新 ADR 記錄 | 待負責人簽核 |
| 3 | 欠約束在團隊模式是否改為 `error` | 維持 `warning`；團隊模式下修正顧問會把它指派給接合師，至少要求一次修正回合，但不阻擋輸出 | 待負責人簽核 |
| 4 | `reserve` 的最終去向 | 永遠留在文件中作為意圖紀錄，不強制「落實」；BOM 不包含 `reserve`，除非人工新增對應的 `purchased` 零件 | 待負責人簽核 |
| 5 | 外購件與標準件 | 2.0 不建標準件幾何庫；外購件以 `source = "purchased"` 的零件表示，可用簡單 Volume 當佔位幾何，列入 BOM，不輸出製造檔 | 待負責人簽核 |
| 6 | agent 的編制 | 先以兩個角色上線：規劃師（= 空間規劃師 ∪ 組裝師的權限）與建造師（= 元件工匠 ∪ 接合師的權限）；§8.5 的四欄矩陣照樣存在，兩角色模式只是欄的聯集 | 待負責人簽核 |
| 7 | 每個角色用哪個模型 | 允許逐角色覆寫，未設定時沿用 `PROMPTCAD_LLM_MODEL` 與 `llm_structured_mode`；設定名稱在 ADR 0009 的實作時決定 | 待負責人簽核 |
| 8 | 公差與配合 | 不屬於約束層；`mate.value` 只是名目間隙；配合等級（H7／g6）與 3D 列印補償另立一層，2.0 只能以 `notes` 記錄 | 待負責人簽核 |
| 9 | 拓撲命名 | 2.0 保留 `all`／`vertical`／`top`／`bottom` 選擇器；第 2 階段在 Docker 的 CadQuery 2.8.0 上以多體布林實測，不符預期時於 2.1 改為以埠指定邊 | 待負責人簽核 |
| 10 | 1.0 的退場時程 | 長期接受 1.0 `spec.json` 輸入（只需保留 `upgrade_1_to_2`）；1.0 專用的 Python 驗證器與編譯器在 ADR 0010 第 2 階段完成條件達成（等價測試通過）後移除 | 待負責人簽核 |

---

## 13. 未凍結／延後事項

- **CadQuery 呼叫名稱與多載簽名**（概念文件 §10.2，例如 Workplane 套用 `cq.Location` 需要 `.val().moved(...)`、`Solid.extrudeLinear`／`Solid.revolve` 的簽名）不屬於 DSL 規格，移到第 2 階段在 Docker 的 CadQuery 2.8.0 環境實測，列為合併條件。
- **數值約束求解器**：第 3 階段依 ADR 0007 評估 planegcs 與 libslvs；本文 §5.5 是其前處理層與軸對齊文件的規範行為。
- **Line／Plane 升維鏈的細節**（`arc` 的平面判定、`revolve` 的完整埠表）在第 4 階段（概念文件 §13）實作前可再細化，但不得改變本文已列出的埠名與欄位名。
- **新增 op 或代碼**只能追加，不得改變既有 op、欄位與代碼的意義。
