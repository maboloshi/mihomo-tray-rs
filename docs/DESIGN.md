# mihomo-tray-rs 设计文档

Windows 系统托盘工具，用 Rust 管理本机 [mihomo](https://github.com/MetaCubeX/mihomo) 内核与其系统级代理设置。
目标：**二进制小、内存小、依赖少**（不使用 UPX 等压缩手段），功能以「托盘能做的操作」为边界，不复刻 Clash for Windows 的完整界面。

- 界面文案：走 `src/i18n.rs` 的语言表，默认中文（与既有 [`mihomo-tray`](https://github.com/aoiyukizakura/mihomo-tray) Go 版一致），另内置英文；使用者可用 `lang/<系统语言>.yml` 覆盖，见 [README](../README.md) 的「界面语言」
- 代码注释 / commit / 本仓库文档语言：见 §11

---

## 1. 范围

### 1.1 功能（MVP）

| 菜单项 | 行为 |
|---|---|
| `Mihomo 状态: 运行中 (rule)` | 灰显状态行；未运行时显示 `未运行`，控制器不可达时显示 `控制器不可达` |
| `系统代理` | 写/清 `HKCU\...\Internet Settings` 的 `ProxyEnable`/`ProxyServer`/`ProxyOverride`，并通知 WinINet 刷新 |
| `代理模式 ▶` | `Rule` / `Global` / `Direct` 单选互斥（原生 radio 标记） |
| `TUN 模式` | `PATCH /configs` 后**回读** `tun.enable` 确认；仍为 `false` 时用提权副本重启内核再重试（UAC 一次，取消无副作用） |
| `代理分组 ▶` | `GLOBAL` + 其余可切换组，每组一个子菜单；成员单选切换；只读组（`LoadBalance`/`Relay`）灰显当前值 |
| `开机自启动` | HKCU Run 键增删 |
| `重载配置` | `PUT /configs?force=true`，body `{"path":""}`（让 mihomo 重载它自己的配置文件） |
| `打开 Web 面板` | 用默认浏览器打开面板地址（`ShellExecuteW`）：默认 `http://<控制器地址>/ui/`（内核 `external-ui` 的挂载点），可由 `ui.web_url` 换成外部托管面板，`{host}`/`{port}`/`{secret}` 替换成当前控制器的值；控制器不可达时与其他操作项一样灰显 |
| `退出 ▶` | `退出并停止 Mihomo`（只结束本程序掌控或路径匹配的进程；提权内核由提权副本停止，再确认一次 UAC）/ `仅退出程序` |

附加（非菜单）：单实例互斥；资源管理器重启后自动重新注册托盘图标。

### 1.2 明确不做

设置窗口、自绘弹窗、owner-draw 视觉、节点延迟色点、流量/内存曲线、脚本执行、CFW 式的 HTML 界面。延迟以**原生菜单项的文本**呈现（§3.2），订阅刷新是**普通子菜单 + 一次 API 调用**（§3.3），都不为此引入 owner-draw。图形化设置不由本程序提供：交给内核自己的面板（`打开 Web 面板`），本程序只负责把地址交给默认浏览器。

---

## 2. 技术选型（均有实测依据）

| 项 | 选择 | 依据 |
|---|---|---|
| 托盘 | `windows-sys` 直接 `Shell_NotifyIconW` | 与 `tray-icon`+`muda` 相比，实测 exe 160 KB vs 460 KB；菜单观感完全相同（muda 内部就是 `CreatePopupMenu`/`AppendMenuW`/`TrackPopupMenu`） |
| 菜单 | 原生 HMENU + `TrackPopupMenuEx(TPM_RETURNCMD)` | 免费获得定位、子菜单、长列表滚动箭头、键盘导航、点外部关闭（**不含滚轮与悬停箭头**，见 §3.1）；`MFT_RADIOCHECK` 提供原生单选 ● |
| HTTP | WinHTTP（`windows-sys` 的 `Win32_Networking_WinHttp`，`WINHTTP_ACCESS_TYPE_NO_PROXY`） | 已在本机对 v1.19.31 实测连通 `/configs`、`/proxies`；比 `ureq` 省约 200 KB；只用 http（controller 是本机回环） |
| JSON | `serde_json`（只用 `Value`，不引 derive） | 字段少，手取值即可 |
| 设置文件 | 手写极简 YAML 子集解析 | 文件由本程序定义，子集受限已足够；省约 100 KB 与一个依赖 |
| 注册表 | `winreg` 0.56 | 纯 Rust、无 `windows` crate 依赖、API 直接 |
| 图标 | `CreateIconIndirect` 由代码生成 RGBA | 0 依赖；不做纯色方块（多尺寸 16/20/24/32 + 描边 + 状态色） |
| 深色菜单 | 清单 + `uxtheme` 序号 135/133/136 + `SetWindowTheme(hwnd,"DarkMode_Explorer")` | 已实测可强制深色；`WM_SETTINGCHANGE`(`ImmersiveColorSet`) 时重算并 `FlushMenuThemes`，运行中切换系统主题即时生效；失败时降级浅色菜单，不影响功能 |

**体积/内存实测**（`opt-level="z"`、`lto`、`codegen-units=1`、`panic="abort"`、`strip`，无 UPX，x64）：

| 组合 | exe | 空闲私有内存 |
|---|---|---|
| raw Win32 + WinHTTP + serde_json + winreg | 160 KB | ~2.4 MB |
| raw Win32 + ureq(无 TLS) + serde_json + winreg | 361 KB | ~1.9 MB |
| tray-icon + muda + ureq(rustls) + serde_yaml | 1.50 MB | ~2.0 MB |
| Tauri / WebView2（参考基准） | ~26 MB | ~128 MB |

预算：**成品 exe ≤ 250 KB，空闲私有内存 ≤ 2.5 MB**。

---

## 3. 菜单结构

```
Mihomo 状态: 运行中 (rule)        ← 灰显
─────────────────────────────
✔ 系统代理
代理模式 ▶
   ● Rule
   ○ Global
   ○ Direct
✔ TUN 模式
─────────────────────────────
代理分组 ▶
   GLOBAL ▶
      ─────────────
      测速本组节点                 ← 调 GET /group/GLOBAL/delay
      ─────────────
      ● DIRECT
        REJECT
        PROXY ▶            ← 成员本身是组时递归（深度上限 4，带防环集合）
            自动选择 ▶
               ● 荷兰-NL-2-HY2-流量倍率:0.5
               …
            自动选择 (URLTest) ● 荷兰-NL-2-…      ← 自动组：可点，点击即固定
              自动（取消固定）                     ← 仅在该组已固定时出现
─────────────────────────────
✔ 开机自启动
打开 Web 面板
更多 ▶
   重载配置
   刷新订阅 ▶                 ← 每个 HTTP/File 类 provider 一项，标签带「上次更新多久」
   关闭所有连接
   重启内核
   强制重启内核
─────────────────────────────
退出 ▶
   退出并停止 Mihomo
   仅退出程序
```

实现要点：

- **命令 id 表**：每次构建菜单生成 `Vec<Action>`，id 从 100 递增（0 保留，由表内顺序推导），`Action` 是 `SetMode(..)` / `ToggleTun` / `Select{group, member}` / `Unfix` / `Reload` / `OpenWebUi` / `Exit*` 等；菜单销毁即清空，不做文本反查。子菜单项共用同一张表，因此插入一个菜单项会让其后所有 id 位移（点击时按当次构建的 id 回查，不存在跨次构建的稳定性要求）。
- **单选**：`MF_CHECKED | MFT_RADIOCHECK`。
- **可切换组**：判据不是「`type` 是不是 `Selector`」而是「适配器是否实现 mihomo 的 `outboundgroup.SelectAble`」（`hub/route/proxies.go` 的 `updateProxy` 同此）。该集合恰好是 `Selector`/`URLTest`/`Fallback` 三种，`LoadBalance`/`Relay` 与普通节点会返回 `400 Must be a Selector`。非 `Selector` 的可切换组（自动组）项文本为 `名称 (类型)`，点击成员即 `PUT /proxies/{name}` 固定该节点；`/proxies` 的 `fixed` 非空时标签追加 `· 已固定`，并在成员列表顶部提供「自动（取消固定）」（`DELETE /proxies/{name}`）。
- **只读组**（`LoadBalance` 等）：整组不可点，项文本为 `名称 (类型)`。
- **组排序**：`/proxies` 是 Go map→JSON，顺序即字典序，mihomo 不提供配置顺序。故 `GLOBAL` 固定置顶，其余按不区分大小写字典序；`tray.yml` 的 `groups.order/include/exclude` 可覆盖。
- **菜单在每次右键时重建**：先同步拉 `/configs`（+ `/proxies`）再建菜单，数据永远新鲜；不依赖轮询快照。
- **订阅刷新**：「更多 ▶ 刷新订阅」的每一项来自快照里的 provider 列表（读法与坑见 §3.3），只有 `HTTP`/`File` 类会出现；标签是「名称 (多久之前更新)」，点击 `PUT /providers/proxies/{name}`。

### 3.1 长列表（节点很多的分组）

原生菜单在高度超过上限时，**由系统自动加顶/底滚动箭头**——这部分不需要自己实现。默认上限是「屏幕高度」，在多显示器、或菜单高于整屏时会失效（菜单被屏幕边缘直接裁掉、不出现箭头），因此构建菜单（含各级子菜单）时显式设置最大高度：

```c
MENUINFO mi = { 0 };
mi.cbSize = sizeof(mi);
mi.fMask  = MIM_MAXHEIGHT;
mi.cyMax  = min(屏幕高度 * 0.6, 900);   // 物理像素
SetMenuInfo(hmenu, &mi);
```

实测（`docs/assets/native-menu-scroll-arrows.png`，Win11）：`cyMax = 700` 时菜单被截为 666 px，顶部 ▲ 灰显（已在列表开头）、底部 ▼ 可用，系统按状态自动置灰。

**滚动手段（人工验证，2026-09）**：只有两种可用。

| 手段 | 结果 |
|---|---|
| 点击 / 按住顶、底箭头 | 可滚（按页） |
| 方向键 | 可滚 |
| 悬停箭头 | 不滚（未实现） |
| 鼠标滚轮 | 不滚（未实现） |

原因未经证实，能确定的只是：原生菜单 + 本程序当前实现（只设 `MIM_MAXHEIGHT`）下，滚轮与悬停箭头都不生效。所以 CFW 截图里那个「悬停小箭头滚动」**不能**当成原生菜单的等价能力（它是自绘 HTML 弹窗的行为）。要支持滚轮就得放弃原生菜单（自绘弹窗）或额外接管消息循环，代价是丢掉系统免费提供的定位、子菜单、键盘导航与点外部关闭，且与 §1.2「不做自绘视觉」的边界冲突——本轮不做，记在 [ROADMAP.md](ROADMAP.md) 的「Phase 2 候选」。

因此长列表实际靠这几样兜住，优先级从高到低：

- 默认只设 `MIM_MAXHEIGHT`：滚动靠**点击/按住箭头**与**方向键**；
- 若仍不好用，可设 `groups.page_size`（默认 `0` = 关闭）：成员数超过该值的分组自动拆成翻页子菜单（`1–50 ▶` / `51–100 ▶`），不依赖任何滚动行为，确定可达；
- 滚轮滚动：**未实现**（见上表）。

### 3.2 节点延迟（文本，不做 owner-draw）

色点要 `WM_MEASUREITEM`/`WM_DRAWITEM` 全套（`MF_OWNERDRAW` 之后每一项的高度、绘制、深色主题、DPI 都得自己算），并会失去原生菜单免费的定位/圆角/主题，实测同类 Win32 代码链入约 15–35 KiB。这里改用**原生菜单项的文本**，观感是 `名称 (123ms)` / `名称 (超时)`，代价实测 **+6.0 KiB**：

- **触发**：每个组的子菜单顶部一项「测速本组节点」（控制器可达且组非空时可用，只加在顶层组子菜单，嵌套层级不重复，免得吃满 `MAX_ITEMS`）。点击 → `GET /group/{name}/delay?timeout=3000&url=…`。内核把这 3 s 当作**整个组的截止时间**、成员**并发**测（`adapter/outboundgroup/groupbase.go` 的 `GroupBase.URLTest`），所以无论组多大都在 3 s 量级返回；全部成员都没回来时该路由回 `504`。请求侧仍要单独放宽（本程序用 10 s），因为 `controller.timeout_ms` 默认只有 2 s —— 比内核自己的截止时间还短，不放宽就会"内核还在测、WinHTTP 先超时"。
- **副作用（内核行为，不是本程序加的）**：`hub/route/groups.go` 的 `getGroupDelay` 对 `URLTest`/`Fallback` 组会先 `ForceSet("")`，也就是**测速会把已固定的组解固定**。菜单里那项因此不适合当作"只看一眼延迟"的无副作用操作；`Selector` 组不受影响。
- **数据来源（两处，缺一不可）**：每个节点自己的 `history` 最后一项 `delay`（`{"time":…,"delay":N}`，内核固定保留 10 条，未测速时是空数组）。
  - `GET /proxies` 给的是内置适配器（`DIRECT`/`REJECT`/…）与**分组**；
  - **订阅节点不在 `/proxies` 里**——实测本机 v1.19.31 的 `/proxies` 只有 9 个键（6 个内置 + 3 个分组），31 个订阅节点只出现在 `GET /providers/proxies` 的 `providers.<名>.proxies[]`。第一版只读 `/proxies`，于是「节点」那一半永远查不到，菜单里一个数字都不显示（这正是用户反馈的现象）。
  - 两份都是内核自己的测量值（它自己每几分钟健康检查一次），所以**不需要在快照里存第二份结果**，也不会过期；「测速本组节点」只是让内核立刻重测一遍。测失败的记录 `delay` 为 `0`，与「没测过」（空数组）分开：前者显示 `(超时)`，后者不显示后缀。
  - `/providers/proxies` 比 `/proxies` 大得多而且随订阅规模增长（本机 31 节点 ≈ 47 KB，`/proxies` ≈ 4 KB），所以它按**自己的较慢节奏**读（`PROVIDER_LATENCY_REFRESH = 30 s`，在 `src/app.rs`），中间复用上次结果；点一次测速会把这个期限清零，让刚测出来的数字在下一次刷新就到。
- **排序**：只在**有测量值**时按延迟升序（无测量值的排最后、保持内核给的顺序）；没有 `history` 时保持 mihomo `all` 数组的原序——那是配置里的顺序，不该因为没测速就被改掉。排序用自己写的插入排序：标准库的稳定排序是「每个元素类型实例化一次」的大函数，`ordered_groups` 已经付过一次，再来一次实测约 4 KiB，而一个组只有几十个节点。
- **不引入 `HashMap`**：延迟是快照里的 `Vec<(节点名, Option<u32>)>`（§11 记录过 `HashMap` 会把哈希表代码重新链进来，是主动砍掉的 4.5 KiB），菜单按名字线性查找——一个子菜单只查自己的成员一次。

### 3.3 订阅刷新（2026-10）

「更多 ▶ 刷新订阅」列出内核的 proxy-provider，点一个即 `PUT /providers/proxies/{name}`：内核在**请求内**重新读该 provider 的源（`hub/route/provider.go` 的 `updateProvider` → `provider.Update()`），成功回 `204`、读不到回 `503` + 它自己的原因。

- **只列 `HTTP`/`File`**：也只有这两类会去重读源。`adapter/provider/provider.go` 里 `proxySetProvider.Update()` 走 `Fetcher.Update()`（HTTP 重新下载、File 重读文件），而 `compatibleProvider.Update()` 直接 `return nil`、`inlineProvider.Update()` 只把时间戳设成 `time.Now()`。本机实测 `GET /providers/proxies` 返回三项：`MyProvider`（HTTP，31 节点）+ 内核内置的 `PROXY`/`default`（Compatible，`updatedAt` 是 Go 零值 `0001-01-01T00:00:00Z`），按此规则菜单里只出现 `MyProvider`。
- **标签**：`名称 (5 分钟前)` / `(2 小时前)` / `(3 天前)`；零值时间（从未更新）显示 `(从未更新)`，一分钟内显示 `(刚刚)`。阈值与措辞都在 `src/i18n.rs`（`menu_provider_updated`），负数（时钟回拨、或刚刷新完的 `updatedAt` 比取快照时的 now 还新）按 `(刚刚)` 处理。
- **时间怎么算**：`updatedAt` 是 RFC 3339（本机实测 `2026-10-01T16:40:02.7094989+08:00`），**手写解析**成 Unix 秒（`src/mihomo/api.rs` 的 `rfc3339_epoch`：严格校验字段与范围，含 offset 与闰年），因此只做减法、与内核所在时区无关，也不为它引入日期库（§11）。
- **不新增请求**：provider 名单、`updatedAt` 与订阅节点的延迟来自**同一个** `GET /providers/proxies`，也就是 §3.2 那条 30 s 节奏的读（`Client::providers` 一次返回两者）。
- **请求侧超时**：内核给自己的下载留 20 s（`component/resource/vehicle.go` 的 `DefaultHttpTimeout`），比 `controller.timeout_ms` 的 2 s 大得多，所以刷新有专属的 30 s（`PROVIDER_REFRESH_TIMEOUT_MS`，与测速那条是同类处理）。实测本机 31 节点订阅刷新耗时 **2.1 s**。
- **结果怎么回来**：与测速一样，命令本身不回传数据 —— 刷新后把 provider 缓存置空，下一次轮询立刻重读，新节点与新的 `updatedAt` 就出现在菜单与延迟标签里；期间状态行显示「正在刷新订阅 X…」。内核重启/被替换后缓存同样置空：旧内核的 provider 不是新内核的证据。
- **实测（本机 v1.19.32，2026-10）**：本程序的客户端解析真实响应得到 3 个 provider（其中 1 个可刷新）+ 37 个带延迟的节点；`PUT /providers/proxies/MyProvider` 由同一个 WinHTTP 客户端发出返回 `Ok`，`updatedAt` 随即变为当前时间。

---

## 4. 内核、配置与控制器来自哪里

**内核自发现，配置按 mihomo 自己的规则解析，但没有端口探测**：认不出来就报错（报错里指名要改的那一项），而不是猜一个。

| 目标 | 顺序 |
|---|---|
| `mihomo.exe` | `tray.yml mihomo.path`（填了就是它：不存在或相对路径 → 启动期报错，绝不回退到搜索）→ 自发现：本程序同目录（`mihomo.exe`、`bin\mihomo.exe`、`core\mihomo.exe`）→ `PATH` 每个目录（`PATH` 顺序即优先级）。**只搜 `mihomo.exe` 这一个名字**（改名了就在 `mihomo.path` 里写）。命中 `shims\mihomo.exe` 时读同目录 `mihomo.shim` 的 `path = …`（只读 `path`，`args`/`env` 一律不读）换成真内核；`%SCOOP%` 无需单独一层——`%SCOOP%\shims` 本来就在 `PATH` 上 |
| 配置目录 | `mihomo.home` → `mihomo.config` 所在目录 → 内核带来的配置所在目录 → mihomo 自己的默认 `%USERPROFILE%\.config\mihomo`（`XDG_CONFIG_HOME` 仅在该目录不存在时参与，条件与 mihomo 一致） |
| 配置文件 | `mihomo.config` →（`home` 非空）`<home>\config.yaml` → 内核带来的那份：内核在**本程序目录树**里（同目录或 `bin\`/`core\`）时先看本程序所在目录的 `config.yaml`，再看内核同目录的 `config.yaml` → mihomo 默认目录的 `config.yaml`。候选按**文件存在**挑选，都不存在就回落到默认路径（mihomo 对空 `-f` 的规则） |
| 命令行 | 运行中那个内核自己的 argv：`-f`/`-d` 说明它读哪份文件；`-ext-ctl`/`-secret` 非空时覆盖配置文件（mihomo 的优先级）。同用户可读（`NtQueryInformationProcess`），提权内核读不到就跳过这一来源 |
| controller | 上述 argv → `CLASH_OVERRIDE_EXTERNAL_CONTROLLER` / `CLASH_OVERRIDE_SECRET`（正是同名 flag 的默认值）→ 配置文件里的 `external-controller` / `secret` → `tray.yml controller.address` / `secret` |

- **发现的答案钉一次**：启动时解析出的内核路径写进 `state::kernel_path`，停止内核、强制重启、身份判定（`pick_kernel`）全部复用它，不在中途重新搜索——否则"哪个内核是我的"会随 PATH 变化。
- **不认领运行中的内核**：链条里没有"某个正在跑的 mihomo.exe"，把别人的内核变成自己的只有「强制重启」这一条路，而且会先问一次。
- 自发现成功时状态行说明一次（`status.kernel_discovered`，一个轮询周期后清除）；链条全空才会报 `error.kernel_not_found`（文案点明可写 `mihomo.path`）。
- **逐字段**：mihomo 侧某字段没设置或读不到，才用 `tray.yml` 的同名字段；`controller.*` 是兜底而非覆盖（旧实现反过来，会把内核的设置架空）。
- 选哪个内核的 argv：被明确告知读**同一份配置文件**的那个；只有一个 `mihomo.exe` 时就是它；其余情况（多个、都读别的文件）不猜，直接跳过这一来源。相对 `-f`/`-d` 无法解析（要读别的进程的 CWD）→ 同样视为"不可知"。
- 本程序启动内核时**总是**显式传 `-d`/`-f`（绝对路径），并拒绝 `mihomo.args` 里的 `-d`/`-f`/`-ext-ctl`/`-secret`（同一设置两处写法 → 内核与托盘读到的文件会不同）；工作目录设为 `mihomo.exe` 所在目录，让 args 里的相对路径有确定基准。环境里的 `CLASH_OVERRIDE_EXTERNAL_CONTROLLER`/`_SECRET` 同样在启动时**具体化成 `-ext-ctl`/`-secret`**（它们是这两个 flag 的默认值，写出来语义不变），因为 UAC 不传环境而提权副本只会复用命令行——这样 TUN 之后内核用的仍是这里解析出的那个控制器。
- 内核没开 `external-controller`、或只开了 `-tls`/`-unix`/`-pipe`，都明确报出来。唯一看不到的情况：内核由别的启动器拉起、用 `-ext-ctl`/`-secret` 覆盖了配置、其命令行又读不到 → 报不可达，由 `controller.*` 兜底。

---

## 5. mihomo API 调用表

全部路径相对 controller 基址；`secret` 非空时加 `Authorization: Bearer <secret>`。

| 用途 | 调用 | 坑 |
|---|---|---|
| 探活 | `GET /` | 返回 `{"hello":"mihomo"}` |
| 读状态 | `GET /configs` | `mode`、`mixed-port`、`tun.enable`（后者是**已生效**值） |
| 切模式 | `PATCH /configs` `{"mode":"rule"}` | 会重建全部 inbound；`tun` 未变时提前返回，不断流 |
| 开关 TUN | `PATCH /configs` `{"tun":{"enable":true}}` | TUN 建立失败只写日志并把 enable 置 false，**HTTP 仍返回 204** → 必须回读确认；回读仍为 false 说明内核没提权：用 `--kernel-start-elevated <pid>` 提权副本重启内核后重试（建 Wintun 需要管理员） |
| 重载配置 | `PUT /configs?force=true`，body `{"path":""}` | **空 body 会 400**；不带 `force` 不重建 inbound；`path` 为空时 mihomo 回落到自己启动时的配置文件 |
| 重启内核 | `POST /restart` | 先回 `200 {"status":"ok"}` 再关进程，所以"响应成功"**不等于**内核已经起来，必须再探活；Windows 上内核用 `exec.Command`+`os.Exit` 以**自己的 argv 与环境**重建进程，因此提权保持、地址不变，但也意味着"别人的内核"重启后仍然是别人的（设置仍不可知） |
| 分组与节点 | `GET /proxies` | 顺序=字典序；`all` 非空的即分组；**只有内置适配器与分组，订阅节点不在这里**（见下一行）；`history` 未测速时是空数组 |
| 订阅节点 | `GET /providers/proxies` | `providers.<名>.proxies[]` 才列出订阅的节点及其 `history`；响应比 `/proxies` 大得多且随订阅规模增长，按较慢节奏读（§3.2）。同一响应里的 `vehicleType` / `updatedAt` 是「刷新订阅」的菜单数据（§3.3）；Compatible 类的 `updatedAt` 是 Go 零值时间 |
| 刷新订阅 | `PUT /providers/proxies/{urlencode(name)}` | 内核在请求里**同步**重读该 provider，成功 `204`、失败 `503` + 内核自己的原因；只有 HTTP/File 类真的重读源（Compatible 立即返回、Inline 只盖时间戳）。内核给自己的下载留 20 s，比 `controller.timeout_ms` 大得多 → 请求侧单独放宽到 30 s（§3.3）。响应不带数据，新节点与新的 `updatedAt` 由下一次 `GET /providers/proxies` 读回 |
| 切换节点 | `PUT /proxies/{urlencode(group)}` `{"name":"member"}` | 组名/成员名必须 percent-encode（中文必需）；仅 `Selector`/`URLTest`/`Fallback` 可写，其余返回 400 |
| 取消固定 | `DELETE /proxies/{urlencode(group)}` | 只对非 `Selector` 的可写组有效（内核 `ForceSet("")`） |
| 测速 | `GET /group/{name}/delay?timeout=3000&url=…` | 成员**并发**测、3 s 是**整个组**的截止（不是每个节点）；全失败回 `504`。响应 `{"成员":延迟}` 只说明谁回来了，真正的数据落在各节点的 `history`，由 `GET /proxies` + `GET /providers/proxies` 读回（§3.2）。请求侧单独放宽（本程序 10 s），不能用 `controller.timeout_ms`。**对 `URLTest`/`Fallback` 组会先解固定**。探测 URL 用 mihomo 自己的默认值（`https://www.gstatic.com/generate_204`）：内核源码里明确警告 http 测试地址可能被机场劫持而测失败 |
| 关闭所有连接 | `DELETE /connections` | 内核遍历连接表逐个关闭后回 `204`（`hub/route/connections.go` 的 `closeAllConnections`）；连接不是设置，下一个请求会重建，所以是"立刻丢弃已开的连接"而非一个保持关闭的开关，也无需二次确认 |

### 5.1 Windows 侧的坑（本轮全部实测过）

| 坑 | 现象 / 证据 | 结论 |
|---|---|---|
| 进程映像路径是**解析后**的真实路径 | 用 junction 拼写启动 `apps\mihomo-v3\current\mihomo.exe`，进程报告 `apps\mihomo-v3\1.19.31\mihomo.exe`（`GetFinalPathNameByHandleW` 两种拼写解析结果一致） | 任何"路径即身份"的比较都必须 `canonicalize` + 忽略大小写（`proc::same_image`）；直接比字符串**永远不相等** |
| scoop shim 也叫 `mihomo.exe` | 枚举里同时出现 shim 与真内核，且真内核是 shim 的**子进程**；按映像路径杀只会杀掉 shim | 停内核要按**父子家族**收敛（`proc::family`），不能只看名字或单个路径 |
| `runas` 不继承调用者的进程环境 | 提权副本里看不到 `CLASH_HOME_DIR`、`CLASH_OVERRIDE_EXTERNAL_CONTROLLER`/`_SECRET` | 提权重启只能靠**内核自己的命令行**：所以 `-d`/`-f` 总是显式传，`CLASH_OVERRIDE_*` 也在启动内核时被具体化成 `-ext-ctl`/`-secret`（同一件事，但写得下来） |
| `runas` 可能以**另一个账户**授权 | 凭据式授权时副本属于另一个用户，读/杀本账户的内核会被拒 | 固有边界，无解；这种机器上只能人工以管理员权限启动内核（§8 已知限制） |
| 退出码是 32 位 | `GetExitCodeProcess` 拿得到完整值（`%ERRORLEVEL%`/`cmd` 会截断到 8 位） | 可用"正数 = PID、负数 = 错误"回传结果；**不要**改成"由调用者指定路径写文件"——那等于给低权限进程一个提权写文件原语 |
| 高完整性级别进程：句柄能开、路径可能被拒 | `OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION)` 成功，`QueryFullProcessImageNameW` 失败 | 这种进程记为**不可核验**，不能归进"不是我们的"——否则会得出"没有可停的了"的假结论 |
| 终止需要同级权限 | 同用户同 IL 可直接 `TerminateProcess`；跨 IL 被拒 | "没提权能杀、提权后杀不掉"就是这条；UAC 只在该弹时弹 |
| 读目标进程的命令行 | `NtQueryInformationProcess(ProcessCommandLineInformation = 60)` 把命令行拷进**调用者自己的缓冲区**，无需 PEB、与目标位数无关；返回的 `UNICODE_STRING` 落在字节缓冲里 | "原样重启内核"最可靠的来源；读这个结构要用 `read_unaligned` |
| `ShellExecuteExW` 的引用规则 | `lpFile` 由 shell 自己正确加引号（带空格的 exe 路径实测 OK）；`lpParameters` 必须自己按 `CommandLineToArgvW` 的规则引用 | 参数拼装要有 round-trip 单测；`SEE_MASK_NOASYNC` 才不依赖调用线程的消息泵 |

**流程上的两条教训（本轮来回三次的根因）**

1. **先量后设计**：涉及进程身份/权限/路径的判断，先在同一台机器上实测一次 Win32 行为（映像路径、token、端口归属），再写逻辑。本轮依次猜"shim 拓扑"、猜"分类判定"，最后量到**路径身份**才是真根因。
2. **不要拿间接信号当结论**：`stopped > 0`、`denied > 0`、子句柄、菜单勾选状态都曾造成"成功"假象；结论只能落在"进程还在不在"或"权威方（提权副本）怎么说"。

---

## 6. 模块与线程模型

```
Cargo.toml                  # windows-sys feature 精确到项；release profile 见 §2
build.rs                    # 仅用于嵌入清单（app.manifest，见 §9）
src/main.rs                 # 单实例 → 加载设置 → 建托盘 → 消息循环；内核交给后台线程
src/win/mod.rs              # 隐藏消息窗口、WM_APP 分发、TaskbarCreated 重注册、托盘图标（Shell_NotifyIconW）
src/win/menu.rs             # 菜单构建、id 表、TrackPopupMenuEx、深色模式
src/win/proxy.rs            # 系统代理注册表 + InternetSetOptionW(39/37)
src/win/autostart.rs        # HKCU Run
src/win/elevate.rs          # 提权副本的参数（PID + 替换用的内核参数）+ ShellExecuteExW("runas") + 等退出码
src/win/shell.rs            # ShellExecuteW：把 URL 交给默认浏览器
src/mihomo/api.rs           # WinHTTP 客户端 + 上述端点
src/mihomo/discover.rs      # §4 的来源解析（内核自发现 + 配置/控制器）+ `-d`/`-f` 组装 + 运行内核 argv
src/mihomo/proc.rs          # 启动/停止内核（只停路径匹配的 PID）+ 按 PID 读映像/命令行 + 提权副本主体
src/paths.rs                # tray.yml 路径：`%NAME%` 按 cmd 展开，且必须绝对
src/settings.rs             # tray.yml 读取（极简 YAML 子集）+ 模板（include_str! 仓库里的两份示例）
src/i18n.rs                 # 界面语言表（内置 zh-CN/en-US）+ lang/<系统标签>.yml 叠加
src/state.rs                # 状态结构 + 内核路径/句柄槽
src/icon.rs                 # RGBA → HICON（多尺寸、状态色）
```

- **UI 线程**：创建窗口/托盘/菜单并跑 `GetMessageW` 循环。所有菜单操作必须在此线程（Win32 菜单线程亲和）。
- **启动顺序**：先注册托盘图标（灰色）并立刻发一条「正在查找内核…」状态注记，解析控制器、拉起内核、等内核应答都在后台线程（等待上限 5 s）。解析过程本身不许拖：读一个进程快照、最多读几个内核的 argv、打开一份配置，都是本机毫秒级操作——原来那套"并行探测 5 个端口、每次 800 ms"的实现已随端口探测一起删除（实测本机那些黑洞端口会让内核启动推迟好几秒）。等待期间写 `Snapshot.status_note`，tooltip 与菜单显示「正在启动内核…」；预算内没应答就换成「内核尚未应答，仍在等待」并一直留着，直到控制器真的答话才清掉。内核慢启动因此只影响状态行，不再推迟图标出现。
- **后台线程**：1 个轮询线程（默认 3 s，`stack_size(256 KB)`）拉 `/configs`，更新图标/tooltip，用 `PostMessageW(WM_APP+n)` 唤醒 UI；菜单项状态在下一次右键重建时同步更新。
- **点击处理**：`TrackPopupMenuEx` 返回命令 id 后**同步**执行（写注册表/发 HTTP 都在本机回环，毫秒级）；HTTP 失败不弹窗，写进 tooltip 并在下一轮自然覆盖。

---

## 7. 设置文件 `tray.yml`

位置：exe 同目录优先（便携），否则 `%APPDATA%\mihomo-tray\tray.yml`；同目录那份**必须有内容**才算数（scoop 清单会建 0 字节文件，空/不可读一律回退 `%APPDATA%`）。首次运行按当前界面语言写出模板：仓库里的 `tray_Sample_zh.yml` / `tray_Sample_en.yml`（`include_str!` 进二进制，仓库与生成物永不漂移）。
解析：`key: value`、一层嵌套、`#` 注释、`"` 或 `'` 引号；不支持列表内联以外的 YAML 特性（无锚点、无多行块）、不支持列表跨行。文件可带 UTF-8 BOM；引号只在同一行内有配对时才开启，所以 `don't` 这样的撇号不会把行尾注释吞进值里。**路径三项（`path`/`home`/`config`）先做 `%NAME%` 展开（`ExpandEnvironmentStringsW`，与 cmd 一致）再要求绝对**：相对路径、`~`、`$VAR` 一律拒绝并报出原值与展开值（`src/paths.rs`）；`mihomo.args` 里的 `-d`/`-f`/`-ext-ctl`/`-secret` 直接被拒（同一设置两处写法）。`https://` 的控制器地址被拒绝而不是去掉前缀（见 §8 已知限制）。

```yaml
mihomo:
  path: ""                # 必填：mihomo.exe（绝对路径）
  home: ""                # 内核 -d；留空 = 内核默认 %USERPROFILE%\.config\mihomo
  config: ""              # 内核 -f；留空且 home 非空 = <home>\config.yaml
  args: []                # 额外启动参数（原样透传，%NAME% 也会展开）
  auto_start: true        # 启动时若内核未运行则拉起
controller:
  address: ""             # 兜底：内核没设置 external-controller、或它的配置读不到时才用
  secret: ""              # 兜底：同上
  timeout_ms: 2000
proxy:
  bypass: []              # 追加到 ProxyOverride（仅在它不存在时写入，已有列表不覆盖）
  system_proxy_on_exit: keep   # 预留（Phase 2 未实现，当前一律「退出时保持现状」）
groups:
  order: []               # 显式排序；其余按字典序
  include: []             # 留空 = 全部 Selector
  exclude: []
  page_size: 0            # 0 = 不翻页（滚动靠箭头点击/按住与方向键，见 §3.1）；>0 时超出则拆翻页子菜单
ui:
  web_url: ""             # 面板地址；留空 = http://<控制器地址>/ui/；{host}/{port}/{secret} 由当前控制器填入
  poll_interval_ms: 3000
  dark_menu: auto         # auto | always | never
```

---

## 8. 状态与错误策略

- 状态来源优先级：controller 实时值 > `tray.yml` > 默认值。`mixed-port` 从 `GET /configs` 取（系统代理指向它），不解析 yml。
- 图标四态（优先序）：TUN=蓝 > 系统代理开=橙 > 内核可用=绿 > 控制器不可达=灰。注册表说系统代理开着但内核不应答时仍是灰——那才是这一刻真正要看见的状态；「内核可用」和两种接管方式是三件事，所以三个颜色。
- 控制器不可达：状态行 `控制器不可达`，模式/TUN/分组项灰显；不自动重启内核（避免和外部管理方式打架），仅当 `mihomo.auto_start` 且进程确实不存在时才拉起。
- TUN 开启后回读仍为 `false` → 内核没有管理员权限：启动自身的提权副本（`--kernel-start-elevated <pid>`）重启内核后重试；再失败则 tooltip 提示「TUN 未生效（通常需要管理员权限）」。取消 UAC 报「提权启动被取消或失败」，且此时旧内核还没被终止。
- 提权副本只从 PID 读**映像**，绝不接受调用方给的映像路径：映像与（start 模式的）命令行都从那个进程读（`NtQueryInformationProcess(ProcessCommandLineInformation)`，见 §5）。**不看映像名**——内核可以改名（`mihomo.path` 是可配置的），而名字检查本来就拦不住谁（调用方能把文件命名成 `mihomo.exe`），只挡误用（PID 复用）。否则副本就等于一个"UAC 弹窗写着本程序、实际以管理员身份运行任意程序"的提权原语。读不到命令行时报「无法读取内核自己的启动参数，未重启内核」并放弃本次重启。**替换助手** `--kernel-replace-elevated <pid> <参数…>` 是唯一的例外，而且只在**参数**上例外：它的目的就是让内核换成"本程序会启动的那一个"，参数不可能来自被替换的进程；映像仍从 PID 读（所以提权场景下只换配置、不换 binary），参数由托盘给出——`tray.yml` 的唯一可读者是托盘（提权副本以另一个账户运行时未必看得见它），让它自己读反而会两边不一致。参数通道不是安全边界：能改 `tray.yml` 的攻击者本来就能让 mihomo 执行任意动作，把住的只能是映像通道。
- 「更多」下的几件事不同（见 [RESTART_KERNEL.md](RESTART_KERNEL.md) §1）：`重载配置` 原地重读配置文件；`关闭所有连接` 让内核丢弃当前连接表（`DELETE /connections`，一次请求，连接随后由下一个请求重建）；`重启内核` 让内核自己换一个进程（argv/环境/令牌原样继承，提权保持，不需要 UAC，但"设置不可知"照旧）；`强制重启内核` 结束当前内核、按 `tray.yml` 拉起本程序自己的内核（设置从此可知）。前两者与重启内核需要控制器（无则灰显），强制重启只需要"有内核在跑"。
- 强制重启的内核未必是本程序启动的，所以先认身份再动手：记录的 PID 或映像匹配本程序解析出的内核（`mihomo.path`，或自发现命中的那个）才静默做；映像可读但不同（别人的内核）弹一次 Yes/No，写明映像与 PID（默认按钮是"否"）；映像读不出来（多半是提权内核）先问同样的问题，再由提权副本完成"停 + 起"——只停不启会让新内核丢掉那份额外权限。`taskkill /IM` 从不使用，只结束选中的那个进程及其启动器家族；启动前用进程列表确认旧内核真的没了，避免两个内核抢同一批端口。
- 副本用**退出码**回答：启动成功 = 新内核的 PID（正数），失败 = 负数（`-2` 参数/不是内核、`-3` 停不干净、`-4` 启不来、`-5` 无权、`-6` 读不到命令行、`-7` `tray.yml` 里没有可启动的内核）；停止助手成功 = 0。之所以不用文件/管道：那需要把路径交给提权进程，低权限调用者就能借一次 UAC 让管理员写任意文件。
- 托盘把回传的 PID 记进 `Snapshot.kernel_pid`，后续「退出并停止 Mihomo」直接用它（提权进程的映像路径读不出来，PID 是托盘唯一能持有的身份）；PID 不在进程列表里即作废。没有记录时才比路径。
- 路径比较必须归一化（[proc.rs](../src/mihomo/proc.rs) `same_image`：`canonicalize` + 忽略大小写）。实测：scoop 的 `apps\mihomo-v3\current\mihomo.exe` 是 junction，进程报告的是 `apps\mihomo-v3\1.19.31\mihomo.exe`，字符串直接比较**永远不相等**——旧代码因此每次都靠"子句柄兜底"才能停掉非提权内核，而提权后子句柄已作废，于是托盘"成功退出"、提权内核留下。
- 停内核按**家族**停（[proc.rs](../src/mihomo/proc.rs) `family`）：scoop shim 也叫 `mihomo.exe`、真内核是它的子进程，只按映像路径杀会留下真内核；家族只从"已匹配到的那一个进程"向上找 `mihomo.exe` 父、向下找子，不会牵连无关进程。
- 判定"是否停掉了"不看分类而看结果：只要还有**路径匹配**或**路径读不出来（可能就是我们提权后的那个）**的进程，就交给提权副本；**可读但路径不同的实例是别人的内核，永远不交给副本**（不弹 UAC、不误杀），托盘直接退出。
- 提权副本是同一个 exe 的隐藏模式，在任何单实例/窗口逻辑之前处理；托盘进程自身永远不提权，也不预置计划任务（计划任务服务可能被禁用）。
- 系统代理：默认「退出时保持现状」（不保存原值、退出时不改写）；`ProxyOverride` **只在注册表里没有该值时才写入**（`bypass` + `<local>`），用户自己整理过的列表不会被覆盖。
- **已知限制**（都由"只有内核需要提权"这一件事决定，不是缺陷）：
  - UAC 若用**另一个管理员账户**的凭据授权，副本以那个账户身份运行，读不到也停不掉本账户的内核（回 `-2`/`-5`）；这种机器上只能人工以管理员权限启动内核。
  - 副本复用**内核自己的命令行**，所以"只存在于环境变量里"的配置能不能过 UAC，取决于它有没有被写进命令行：`-d`/`-f` 总是显式传，`CLASH_OVERRIDE_EXTERNAL_CONTROLLER`/`_SECRET` 在启动内核时被具体化成 `-ext-ctl`/`-secret`，因此这两类都跟得过去；`CLASH_HOME_DIR`/`CLASH_CONFIG_FILE` 则完全不参与（被显式 `-d`/`-f` 钉住）。只有"内核由别的启动器拉起"时托盘才可能读不到它的设置，那时用 `controller.*` 兜底（`mihomo.args` 不接受 `-d`/`-f`/`-ext-ctl`/`-secret`）。
  - 记录的 PID 只在本会话内有效（跨托盘重启靠路径身份兜底）；PID 号被复用的窗口极小，但仍以"它还在 `mihomo.exe` 列表里"为唯一校验。
  - 替换助手要**自己读 `tray.yml`**，所以它看到的布局取决于它以哪个账户运行：`%APPDATA%` 若不是本账户的，只有 exe 旁的便携 `tray.yml` 一定可见；这种情况下它回 `-7` 并保持旧内核不动，而不是按半份设置启动一个新内核。
  - 关闭 TUN **不回读**（开启必须回读）：静默失败只会表现为菜单勾选状态没变。
  - **控制器只按明文 http 访问**：`Client::new` 接受 `host:port` 与 `http://host:port`，写 `https://host:port` 会被明确拒绝（`ClientError::TlsUnsupported`，文案见 `error.controller_tls`）。WinHTTP 请求一律不带 `WINHTTP_FLAG_SECURE`——去掉前缀再按明文连，等于把明文请求发到一个只答 TLS 的端口上，所以宁可报错。控制器在本机回环上时这不是问题（mihomo 的 `external-controller` 本身就是明文 http）；跨机需要加密时自行套隧道，或在托管面板里填 https 地址（那只影响浏览器）。

---

## 9. 构建与清单

- `[profile.release]`：`opt-level="z"`、`lto=true`、`codegen-units=1`、`panic="abort"`、`strip=true`、`incremental=false`。
- `#![windows_subsystem = "windows"]`（无控制台窗口）。
- 应用清单：`Microsoft.Windows.Common-Controls 6.0` + `supportedOS {8e0f7a12-…}`（Win10/11）+ `permonitorv2`。这决定菜单圆角、主题与 DPI 是否正确——已实测（见 `docs/assets/app-menu-light.png`、`docs/assets/app-menu-dark.png`）。
- 清单以外部文件 `<exe>.name.manifest` 或 `build.rs` 嵌入，Phase 1 决定（外部清单便于调试）。

---

## 10. 实施阶段

| 阶段 | 内容 | 状态 |
|---|---|---|
| Phase 0 | 技术验证：raw 托盘 + WinHTTP 调控制器 + 原生菜单 + 清单/深色；已产出体积/内存数据与菜单截图 | **已完成** |
| Phase 1 | MVP：§1.1 全部菜单项 + `tray.yml` + 三条发现链 + 单实例 + TaskbarCreated；真机复测体积/内存并截图；人工验证清单 #1–#12、#16 通过（#13–#15 待验证，见 [ROADMAP.md](ROADMAP.md)） | **已完成** |
| Phase 2（可选） | 只读组 `now` 展示增强、退出时禁用系统代理、schtasks 免 UAC 自启 | 视需要 |
| Phase 2 已做 | 「更多」子菜单 + `重启内核`（`POST /restart`）+ `强制重启内核`（按 `tray.yml` 归一别人的内核，含提权副本 `--kernel-replace-elevated`）+ `关闭所有连接`（`DELETE /connections`） | **已完成**，重启内核见 [RESTART_KERNEL.md](RESTART_KERNEL.md) |
| Phase 2 已做 | 组内「测速本组节点」（`GET /group/{name}/delay`）+ 成员延迟文本 `(123ms)`/`(超时)` 与按延迟排序（§3.2；不做 owner-draw 色点） | **已完成**（2026-10） |

---

## 11. 实现记录（与设计的偏差）

1. **当前项用 ✔ 而不是 ●**：模式与分组当前值统一用 `MF_CHECKED`（用户要求「当前节点打对钩」），因此不需要 `MFT_RADIOCHECK`。分组的当前值是嵌套子组时，**子菜单项本身也带 ✔**（`MF_POPUP | MF_CHECKED`）。
2. **worker 先刷新再等待**：原实现先 `recv_timeout(poll)` 再刷新，导致首个菜单（3 s 前打开）显示空状态；现改为循环开头立刻刷新，实现中实测发现并修复。
3. **图标由代码生成**：`CreateIconIndirect` + 32bpp DIB，4× 超采样画圆环与中心点，尺寸取 `SM_CXSMICON`（`Icons::new` 与 `TaskbarCreated` 重注册时各量一次，主显示器 DPI 变化后重画而不是沿用旧尺寸），无资源文件、无图像库。
4. **非 `Selector` 组也带类型**：`自动选择 (URLTest)`；早期版本按「`type == "Selector"` 才可切换」把 `URLTest`/`Fallback` 一起灰显了，实测这两个组在内核里同样接受 `PUT /proxies/{name}`，故改为按 `SelectAble` 判据、并补上「已固定 / 取消固定」。
5. **实测体积/内存**：exe 329.0 KB（336,896 B；2026-09 体积复查后的数字，复查前 389.0 KB / 398,336 B，见本节「二进制体积复查」），空闲私有内存 ~2.6–3.4 MB、工作集 ~15–18 MB（WinHTTP 内部线程已计入）。`serde_json` 实测约 33 KB，其余为 std 基线与本程序代码。
6. **单测 89 个**：HTTP 层用 `TcpListener` 起本地假控制器，走真实 WinHTTP 断言 method/path/body/Authorization/错误码（含 `https://` 地址被拒），进程层对真实进程断言映像路径可读（含"起始缓冲不够就翻倍"）、并断言托盘给提权副本的那条命令行经 `CommandLineToArgvW` 往返后逐字不变（含尾反斜杠与空参数），图标层断言当前尺寸与 `SM_CXSMICON` 一致；菜单层用 `GetMenuStringW`/`GetMenuState` 断言项顺序、勾选与灰显、项数预算与控制字符处理；重启流程另用一个**常驻**假控制器端到端跑（清 PID/句柄、替换 client、清 version——单发假服务器演不了）；i18n 层断言中英表键与占位符一一对应、随仓库分发的模板就是 `build.rs` 的生成物（比对构建脚本盖进二进制的指纹，手改生成段即失败）且逐条等于内置英文表、语言文件叠加与回退、替换不回扫（组名里的 `{kind}` 原样显示）；设置层断言子集解析的边界（BOM、撇号、引号内逗号、`page_size` 钳制、路径与 CJK 值），路径层断言 `%NAME%` 展开与绝对性判定，来源层断言内核候选链的顺序与"真 binary 优先于 shim"、配置候选的顺序（捆绑根 → 内核同目录 → mihomo 默认）与存在性挑选、控制器链的优先级（argv → 环境变量 → 配置文件 → `controller.*`）与运行内核 argv 的挑选规则。
7. **界面文案集中到 `src/i18n.rs`**：原先前述文案散在 11 个文件里，现收进语言表（62 条），语言取 Windows UI 语言标签，`lang/<该标签>.yml` 叠加在内置表上。`settings::load` 相应改为返回结构化 `LoadError`，文案由调用方渲染——否则「读取 `tray.yml` 失败」本身没有语言可依。语言文件的解析**不复用** `settings::strip_comment`：它把空格后的 `#` 当注释、把未配对的引号当成开启的引号串，会静默截断 `Proxy #1` 这类译文，并把行尾注释当成译文显示。
   **模板由构建脚本生成**：`lang/en-US.yml` 的标记行以下部分是 `build.rs` 从 `src/i18n.rs` 重写出来的——键取自 `messages!` 列表（只有那里拼键名），值取自内置 `EN_US` 表，生成文本的 FNV-1a 指纹经 `cargo:rustc-env` 交给单测，手改生成段或改文案没重建都会让单测失败。标记行以上是手写说明，构建脚本原样保留。于是「新增或修改文案」只剩改 `src/i18n.rs` 一处，不再需要手工同步语言文件。
   **代价实测**：exe 由 337,920 B 增至 361,984 B（+24 KB，当时的数字），远高于动工前估的 3–4 KB——语言表本体、62 路 `overlay`、22 个渲染方法与解析器各占一块。读取用的 `HashMap` 已换成线性扫描（62 条只在启动读一次），省回 4.5 KB；读取路径改用 `Vec<(String, String)>` 后不再把 SipHash 与哈希表代码链进这个以 KB 计的项目。

### 节点延迟（文本呈现，2026-10）

原「Phase 2 候选」里这条的估价是**成本大**，因为健康色点必须 owner-draw。最终采用不引入任何新 Win32 机制的文本方案（§3.2），实测体积从 **343,040 B → 349,184 B（+6,144 B）**，单测 89 → 97。

- 期间试过、并**被实测否掉**的两条路：把延迟放进 `Snapshot` 的 `std::collections::HashMap`（把 SipHash/hashbrown 重新链进来，与 §11.7 的做法相反），以及用 `serde_json::Value::to_string()` 把 `/group/{name}/delay` 的响应直接显示出来（首次触发 `serde_json` 的**序列化器**，此前全仓库只用 `from_str`）。两者合计让二进制到 363,008 B（+19.5 KiB）。改为「只读 `history` 的数字 + `Vec<(String, Option<u32>)>`」后回到 +4.5 KiB。
- 排序是第二块成本：`slice::sort_by_key`/`sort_by_cached_key` 会为 `(String, Option<u32>)` 再实例化一次标准库稳定排序，实测 **+4.1–4.6 KiB**；改写为十几行的稳定插入排序后该成本归零（组内节点数由 `MAX_ITEMS` 兜底）。
- 由此固定的两条实现边界：**快照里不存测速结果**（数据源就是内核的 `history`，下一次 `/proxies` 自然带回来），以及**没有测量值时不重排**（mihomo 的 `all` 顺序就是配置顺序）。
- **实现前后各读了一遍上游源码**（`hub/route/groups.go`、`adapter/outboundgroup/groupbase.go`、`adapter/adapter.go`、`constant/adapters.go`），改掉了三个凭印象写错的点：组测速是**并发**且 `timeout` 是**整组**截止（不是每节点 3 s，所以请求侧 30 s 是白等，改为 10 s）；`history` **始终存在**（未测速时是空数组，不是缺字段），失败的记录 `delay` 为 `0`；探测 URL 用 mihomo 自己的 `https` 默认值（它源码里警告 http 测试地址可能被机场劫持）。另外记下一条内核行为：组测速会**解固定** `URLTest`/`Fallback` 组。
- **另一处只有真机能发现的坑**：`/proxies` **不含订阅节点**。本机 v1.19.31 的 `/proxies` 只有 9 个键（`COMPATIBLE`/`DIRECT`/`PASS`/`PASS-RULE`/`REJECT`/`REJECT-DROP` + 3 个分组），31 个订阅节点只出现在 `/providers/proxies`。第一版按「`/proxies` 里每个节点自己的 `history`」实现，于是正好对用户最在意的那些节点一个数字都不显示——**读文档不如对着运行中的控制器看一眼**，这条记进 §5 的调用表。

### 订阅 provider 刷新（2026-10）

「更多 ▶ 刷新订阅」（§3.3），实测量体积 **349,184 B → 354,816 B（+5,632 B）**，单测 97 → 103。

- **体积去向**：`Provider`/`Providers` 两个类型、手写 RFC 3339 → Unix 秒的解析（含 `days_from_civil`、月份/闰年校验，约 60 行）、菜单标签、7 条文案（中英各一份）。与节点延迟那条的 +6,144 B 同量级——都换来一个原生菜单项，仍远低于 owner-draw 那条链（§3.2 的 15–35 KiB 估）。
- **为什么手写日期解析**：只为把 `updatedAt` 换成相对时间就引入日期库不划算（与 §2 放弃 HTTP 库、§11.7 放弃 `HashMap` 是同一取舍）。解析严格校验字段与范围，15 条非法输入进了单测。
- **为什么是相对时间而不是绝对时间**：绝对时间要按本机时区渲染（要么开 `Win32_System_Time` 用 `SystemTimeToTzSpecificLocalTime`，要么假定内核与本机同区）；相对时间只把 RFC 3339 的 offset 减掉，与内核时区无关，也正好回答"这个订阅有多旧"。
- **失败不再清缓存**：`/providers/proxies` 读失败时保留上一次结果（此前一律换成空）。上一份数据是几十秒前、同一个内核的读数，保留它比让菜单里所有 provider 一起消失更诚实；这条同样是「刷新订阅」点名要用的数据。
- **真机核对**（v1.19.32）：解析真实响应得到 3 个 provider（1 个 HTTP 可刷新 + 2 个 Compatible 零值）+ 37 个带延迟的节点；`PUT /providers/proxies/MyProvider` 由本程序的 WinHTTP 客户端发出返回 `Ok`，耗时 2.1 s，`updatedAt` 随即变成当前时间。**只有对着运行中的控制器跑一次才能确认**的事：Compatible 的 `updatedAt` 是零值（不是缺字段，`omitempty` 对 `time.Time` 无效）、`PUT` 无 body 也被内核接受。

### 二进制体积复查（2026-09）

发布前用 `cargo bloat --release` 加 PE 段表复核了一遍（不是估）。复查前 exe 389.0 KB：`.text` 291.0 / `.rdata` 81.5 / `.pdata` 10.5 / `.data` 2.5 / `.rsrc` 1.5 / `.reloc` 1.0 KiB，段和等于文件大小，没有对齐浪费；`.text` 里 std 占 169.1 KiB（58.1%）、本程序占 81.6 KiB（28.1%）。三块与业务无关的重量：

| 重量 | 实测 | 处理 |
|---|---|---|
| `std::process::Command` 及其环境块、参数转义、管道代码 | 同组符号 51.1 KiB，其中 `spawn_with_attributes` 单独 21.8 KiB | 程序只启动一个进程，改为直接调 `CreateProcessW`：**−55.5 KiB**（`6b0ef37`） |
| panic 打印、backtrace 与 stderr 写入链 | 12.1 + ~4.4 KiB | 只服务「panic 时写给没人看的 stderr」；要砍需 nightly 的 `build-std` + `panic_immediate_abort`，本轮**未做** |
| i18n 表构造 | `i18n::init` 24.2 KiB（当时二进制里最大的单个函数） | 内置表改 `static`（字段全是 `Cow::Borrowed`，本来就能进 `.rdata`）：**−4.5 KiB**（`e2a1495`）。比预期小：`Messages::clone` 与 62 路 `overlay` 仍在，`i18n::resolve` 现在 19.9 KiB |

结果：exe 398,336 B → **336,896 B（−60.0 KiB，−15.4%）**，`.text` 291.0 → 239.5 KiB。仍高于 §2 的 250 KB 预算（那条是 Phase 0 的选型预算）。

复查顺带记下、**本轮未处理**的几项，都属于便宜且独立的小改动：`Cargo.toml` 的 `Win32_Security` 实际被 `src/mihomo/proc.rs`（`SECURITY_ATTRIBUTES`，用于 `CreateFileW` 与进程启动）引用，不可删；`Cargo.lock` 里 `serde_derive`/`proc-macro2`/`syn`/`quote`/`unicode-ident` 仍由 `serde_core`（`serde_json` 依赖链）拉入构建图，不能单独去掉；`win/{mod,elevate,menu,shell}.rs` 各带一份 `wide()`；`std::env::var`（18 处）会把 `to_lowercase`（3.9 KiB）链进来，换 `GetEnvironmentVariableW` 可去掉。另有一项既有失败：`cargo test`（debug）下 `icon.rs:109` 的 `color * 3` u8 溢出使图标那项失败，`--release` 全绿——与本次改动无关，主树同样失败。（`lang/en-US.yml` 曾在列表里，现已由 `build.rs` 生成，见本节 §7。）

### 代码审查修复（第二轮，10 项 must-fix + 若干 should-fix）

| 问题 | 修复 |
|---|---|
| 动作/启动错误被静默丢弃（`pending_error` 从不进入快照） | 拆成 `action_error`（跨刷新保留，下一次动作清除）与 `controller_error`（可达即清），tooltip 与菜单统一取 `Snapshot::error()` |
| `groups.order` 被第二次排序覆盖而失效 | 合并成单一比较器：配置顺序 → `GLOBAL` → 字典序 |
| 互相引用的分组导致嵌套展开 ~G⁴，右键卡死 UI 线程 | `MAX_ITEMS = 1500` 全局项数预算，超限追加灰显「项目过多，已省略」并停止递归 |
| 重入守卫在 `dispatch` 之前清除，理论上可重入产生别名 `&mut App` | 守卫覆盖整个 `dispatch`（`ShellExecuteW`/`InternetSetOptionW` 会泵消息） |
| 「退出并停止 Mihomo」可能杀掉非本程序启动的实例 | 失败关闭：路径未知或 `OpenProcess` 被拒时不终止，并提示「无法确认归属」 |
| 提权副本重启内核后，托盘仍持有旧内核的 `Child` 句柄，「退出并停止」会以为已经停掉 | 句柄只在 `try_wait()` 仍是 `Ok(None)`（进程还活着）时才算证据；内核身份改由「副本回传的 PID → 归一化路径 → 唯一不可读的那个」判定，不再维护句柄作废消息 |
| 提权后的内核停不掉：托盘"成功退出"、提权 `mihomo.exe` 仍在 | 根因是路径比较：`current\mihomo.exe` 与进程报告的 `1.19.31\mihomo.exe` 永不相等（junction 在 CreateProcess 时被解析），旧代码因此落到"旧句柄兜底"或当作已停止。改为 `same_image` 归一化比较 + `denied ⇒ NeedsAdmin` + 副本按家族停止 |
| `szTip` 可能无 NUL 终止 | 按 UTF-16 单元截断到 127 并留终止位 |
| 用 `NOTIFYICON_VERSION_4` 后悬停**完全没有 tooltip**（菜单正常，故一直没被发现） | `NOTIFYICONDATAW.uFlags` 必须带 `NIF_SHOWTIP`：v4 下标准 tooltip 默认被抑制、让位给应用自绘弹窗（见 MS Learn `NOTIFYICONDATAW`）。`icon_data` 的 ADD/MODIFY 都带该标志 |
| `mixed-port` 无检查强转 `u16`（70000 → 4464 并写进系统代理） | 越界过滤为 0 |
| `CreateDIBSection` 部分失败时泄漏 `HBITMAP`；AND mask 未初始化 | 失败路径释放，mask 传零填充缓冲 |
| 响应体无上限缓冲 | 8 MiB 上限，超出报「响应过大」 |
| 其它 | 中毒锁 `into_inner`、`WM_DESTROY` 删除图标、`NIM_SETVERSION` 失败即重试、uxtheme 优先按名取、mihomo 配置注释剥离识别引号、`Menu` 实现 `Drop`、启动等待内核限定 5 s 预算、`Client::new` 返回具名错误（`NotAnAddress`/`TlsUnsupported`）、进程映像路径缓冲按需翻倍（起始 1024、上限 32K）、`groups.page_size` 上限 1000 |

### 代码审查修复（第三轮，发布前）

| 问题 | 修复 |
|---|---|
| UTF-8 BOM 让 `tray.yml` / `lang/<tag>.yml` 的第一节变成 `\u{feff}mihomo`、`\u{feff}menu`，其下所有键静默失效 | 两个解析器都先 `strip_bom` |
| 未加引号的撇号（`don't`）开启了一个永不闭合的引号串，行尾注释被并进值里 | 只有同一行内存在配对的引号时才开启引号区 |
| `["a,b"]` 被切成两个条目 | 只按引号外的逗号切分 |
| `-f` 只在 `tray.yml` 显式指定时补，`CLASH_CONFIG_FILE` 指向的非默认文件名不会告诉内核，内核于是去读 `-d` 下的 `config.yaml`（不是托盘刚读过的那份） | 文件名不是 `config.yaml` 就补 `-f`（显式配置一律补） |
| `controller_from_config` 不区分缩进，嵌套的 `secret:` 会覆盖控制器的 | 只认顶层键 |
| scoop 清单在 exe 同目录建了个 0 字节 `tray.yml` 并 persist 它，便携布局于是永久压过用户真正编辑的 `%APPDATA%\mihomo-tray\tray.yml`：填什么都不生效 | portable 文件必须有内容才算数（空/不可读一律回退 `%APPDATA%`）；打包改为装 `tray_Sample_*.yml` |
| `controller.address` 一旦非空就直接返回、配置文件里的 `external-controller`/`secret` 被架空，内核改了 secret 托盘还拿旧值去连 | 控制器改为按 mihomo 自己的优先级解析（argv → 环境变量 → 配置文件 → `controller.*` 兜底），`controller.*` 只在 mihomo 没设置/读不到时生效 |
| 内核在"列出进程"和"终止"之间退出时，`TerminateProcess` 对已退出进程同样返回 access-denied，被记成"拒绝" → 中止其余终止、提权重启失败 | 用进程对象是否已 signaling 区分"已退出"与"无权"；`OpenProcess` 报 87 也按已退出处理 |
| 命令行无参数时 `argv[1..]` 越界 panic（提权副本路径） | 改用 `get(1..)` |
| `is_shim` 用子串匹配，`shims-backup` 被误判为 scoop shim | 按父目录名是否为 `shims` 判断 |
| `WinHttpQueryHeaders` 返回值被忽略，失败时状态码停在 0，界面显示 "HTTP 0" | 查询失败即报请求失败 |
| `CreateMutexW` 成功时不重置 last error，线程此前留下的 `ERROR_ALREADY_EXISTS` 会让第一个实例误判"已在运行"而静默退出 | 调用前 `SetLastError(0)` |
| `ProxyOverride` 读不出字符串就被当成空并覆盖 | 只看该值是否存在，存在就保持原样 |

同轮记录、**未在本次修复**的差距见 [ROADMAP.md](ROADMAP.md) 的「已知差距」。

### 人工验证结果（2026-09，自动化覆盖不到的部分）

原先「尚未在自动化中验证、需人工确认」的三点已逐项点过，清单与偏差记录在 [ROADMAP.md](ROADMAP.md)（#1–#12、#16 通过；#13–#15 待验证）：

- **菜单内的鼠标交互**：悬停展开子菜单正常（清单 #1）。滚动方式的结论与原先的假设不同：**点击/按住顶底箭头、方向键可滚，悬停箭头与鼠标滚轮不滚**（详见 §3.1）——原以为「系统滚动箭头 + 滚轮 + 方向键」三样都免费拿到，实测只成立一半。键盘激活与 Esc 后的焦点回归属清单 #13，仍待验证。
- **`PUT /configs?force=true`**：已在本机运行中的内核上人工执行，「重载配置」确实让内核重读了自己的配置文件（清单 #3）；上游源码依据（`hub/route/configs.go:408-444`）不变。
- **真实内核的启动路径**：`start` 改走 `CreateProcessW`（`6b0ef37`）之后，提权重启（清单 #5 的提权副本用它拉起真内核）与「退出并停止 Mihomo」（清单 #7）都在真 `mihomo.exe` 上跑过（同一条路径的单测仍是真的起 `cmd.exe`、断言 PID 与退出）；`-d`/`-f` 的来源解析仍按 §4。

完整待办与人工验证清单见 [ROADMAP.md](ROADMAP.md)。

