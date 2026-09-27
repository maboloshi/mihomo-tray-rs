# mihomo-tray-rs

Windows 系统托盘工具，用 Rust 管理本机 [mihomo](https://github.com/MetaCubeX/mihomo) 内核与其系统级代理设置。
目标是**二进制小、内存小、依赖少**：原生 Win32 + WinHTTP，不引入 WebView、HTTP 客户端库或 YAML 库。

![托盘菜单](docs/assets/app-menu-light.png)

## 特性

- **状态行** — `Mihomo 状态: 运行中 (rule)` / `未运行` / `控制器不可达`
- **系统代理** — 写 `HKCU\...\Internet Settings` 并通知 WinINet 刷新（只写注册表不会即时生效）
- **代理模式** — Rule / Global / Direct 单选互斥，勾选当前值
- **TUN 模式** — 切换后回读 `tun.enable` 确认；内核没有管理员权限时弹一次 UAC，由提权的自身副本把内核换成提权实例后重试
- **代理分组** — `GLOBAL` 与其余可切换组，逐层子菜单；成员打钩表示当前节点。`Selector`、`URLTest`、`Fallback` 的成员都可点（后两者的点击等于手动固定节点，菜单会多出「自动（取消固定）」）；`LoadBalance` 等真正只读的组灰显但展示当前值
- **开机自启动** — HKCU `Run` 键，不触发 UAC
- **重载配置** — `PUT /configs?force=true`，让内核重载它自己那份配置（本程序无需知道 yml 路径）
- **打开 Web 面板** — 用默认浏览器打开面板地址：默认是内核自己的 `external-ui`（`http://<控制器地址>/ui/`），也可在 `tray.yml` 里换成外部托管的面板（如 zashboard / metacubexd），`{host}`/`{port}`/`{secret}` 会替换成当前内核的值
- **退出** — 「退出并停止 Mihomo」（只结束本程序掌控的实例；内核被 TUN 提权过时会再要一次 UAC）/「仅退出程序」
- 单实例互斥；资源管理器重启后自动重新注册托盘图标；菜单主题跟随系统，运行中切换明暗即时生效

## 环境要求

- Windows 10/11 (x64)
- [mihomo](https://github.com/MetaCubeX/mihomo) 已安装（本程序也能自行发现并拉起）
- 构建需要 Rust stable（`edition 2024`，MSRV 1.85），无需额外工具链

## 构建

```powershell
cargo build --release      # 产出 target/release/mihomo-tray.exe
cargo test                 # 设置解析 / URL 编码 / 地址解析的单测
```

`[profile.release]` 使用 `opt-level="z" + lto + codegen-units=1 + panic="abort" + strip`，并**不做**任何压缩打包。应用清单（Common-Controls v6 + supportedOS Win10/11 + per-monitor-v2 DPI）由 `build.rs` 用纯 linker 参数嵌入，决定菜单的圆角/主题/DPI 是否正确。

实测（本机 Win11 x64，rustc 1.98）：

| 指标 | 数值 |
|---|---|
| 可执行文件 | ~353 KiB (实测 361,984 B) |
| 空闲私有内存 | ~3.3 MB（含 WinHTTP 内部线程） |
| 空闲工作集 | ~18 MB（多为共享 DLL 页） |
| 依赖 | `windows-sys`、`serde_json`、`winreg`（+ 构建期无依赖） |

## 设置文件

`tray.yml`：**exe 同目录优先**（便携），否则 `%APPDATA%\mihomo-tray\tray.yml`；首次运行会自动写出带注释的默认文件。解析器只支持一个文档化的 YAML 子集：顶层键、一层嵌套、`#` 注释、引号标量与内联 `[a, b]` 列表。

```yaml
mihomo:
  path: ""            # 留空 = 自动发现 mihomo.exe
  args: []            # 额外启动参数，例如 ['-d', 'C:\mihomo']
  config: ""          # 显式配置文件路径（也用于读取 external-controller）
  auto_start: true    # 未运行时自动拉起内核
controller:
  address: ""         # 例如 127.0.0.1:9090；留空 = 自动发现
  secret: ""
  timeout_ms: 2000
proxy:
  bypass: []          # 追加到 ProxyOverride，始终包含 <local>
groups:
  order: []           # 显式分组顺序，其余按字典序（GLOBAL 永远在最前）
  include: []         # 留空 = 全部 Selector
  exclude: []
  page_size: 0        # 0 = 交给系统滚动箭头；>0 时长列表拆成翻页子菜单
ui:
  web_url: ""         # 面板地址；留空 = http://<控制器地址>/ui/（内核自己的 external-ui）
                      # {host}/{port}/{secret} 会替换成当前控制器，例如：
                      # https://board.zash.run.place/#/setup?hostname={host}&port={port}&secret={secret}
  poll_interval_ms: 3000
  dark_menu: auto     # auto | always | never
```

## 界面语言

界面内置中文（默认）与英文，**语言跟随 Windows 的显示语言，没有单独的开关**。其它语言由使用者自行提供：

1. 在 `tray.yml` 的同目录下建 `lang/`（便携模式下就是 exe 同目录，否则是 `%APPDATA%\mihomo-tray\lang\`）；
2. 文件名用**系统语言的 BCP-47 标签**，例如 `ja-JP.yml`、`ko-KR.yml`、`de-DE.yml`——程序按这个标签找文件，所以名字必须和系统语言一致；
3. 内容照抄仓库里的 [`lang/en-US.yml`](lang/en-US.yml)（自带注释，也是模板），把右侧的值换成译文。

查自己的系统标签（PowerShell）：

```powershell
(Get-UICulture).Name
```

译文可以只写一部分：文件里没有的键回退内置英文，值写成空等同于没写。格式是 `段名:` 加两空格缩进的 `键: 值`，**值取第一个 `:` 之后的全部内容**，所以译文里的 `#`、`:`、撇号都不需要转义，只有整行以 `#` 开头才是注释。`{name}`、`{error}` 这类占位符是运行时填入的，必须原样保留。

没有对应语言文件时回退内置表：`zh`、`en` 按主语言匹配（`en-GB` 也能命中英文表），其它未支持的语言一律用默认的中文表，不会出现空界面。

## 自动发现（三层，来源互不相同）

| 目标 | 顺序 |
|---|---|
| `mihomo.exe` | `mihomo.path` → 运行中进程的真实映像（跳过 scoop shim）→ exe 同级/`bin`/`core` → `PATH` → `$SCOOP\shims`、`~\scoop\shims` |
| 配置文件 | `mihomo.config` → `CLASH_HOME_DIR` + `CLASH_CONFIG_FILE` → `%USERPROFILE%\.config\mihomo\config.yaml` → exe 同级 |
| 控制器地址 | `controller.address` → `CLASH_OVERRIDE_EXTERNAL_CONTROLLER` / `CLASH_OVERRIDE_SECRET` → 上述配置文件里的 `external-controller` / `secret` → 端口探测（9090/9091/9097/9098/6170，用 `GET /` 校验） |

控制器地址不能只靠解析配置文件：它常由 `-ext-ctl` 或环境变量注入，所以最后一步的探测是必需的。

## 已知行为

- **重载配置不需要知道 yml 路径**：`PUT /configs?force=true` 且 `path` 为空时，内核回落到自己启动时的配置文件；`force=true` 才会重建 inbound 监听器。
- **TUN 失败不会返回 HTTP 错误**（内核只记日志并把 `enable` 置 false），因此本程序以回读 `GET /configs` 的结果为准：回读仍是 `false` 时，用 `ShellExecuteExW("runas")` 启动自身的一个隐藏副本（`--kernel-start-elevated <pid>`），由它停掉旧内核、并按**那个内核自己的命令行**重新启动，然后重试。取消 UAC 没有任何副作用——终止旧内核发生在提权之后。
- **提权副本只收一个 PID，并用退出码回传新内核的 PID**：映像路径和启动参数都由副本自己从那个进程读出来（`NtQueryInformationProcess`，提权之后连托盘读不到的提权内核也能读）。这既不会把启动参数猜错（`-d`/`-f`/`-ext-ctl` 一律原样保留），也不让副本变成"以管理员身份运行任意程序"的入口——它只接受映像名为 `mihomo.exe` 的进程。退出码 = 新内核 PID 为正，失败为负（`-2` 参数/不是内核、`-3` 停不干净、`-4` 启不来、`-5` 无权、`-6` 读不到命令行）；不走文件/管道回传，因为"由调用者指定路径"就等于给低权限进程一个让管理员写文件的口子。
- **内核身份按「记录的 PID → 路径身份」判定**：提权后托盘读不到那个进程的映像路径，所以后续操作以副本回传的 PID 为准；没有记录时才比路径，而且比较必须**归一化**（`canonicalize` + 忽略大小写）——scoop 的 `apps\<app>\current\mihomo.exe` 是 junction，进程报告的是解析后的 `apps\<app>\<版本>\mihomo.exe`，直接比字符串永远不相等（这正是"提权后内核停不掉"的根因）。
- **本程序自己启动内核时会补上 `-d <配置目录>`**（`mihomo.args` 里已有 `-d`/`--d` 就不重复加）；`mihomo.config` 指向一个具体文件时还会带上 `-f <文件>`，因为那个文件不一定叫 `config.yaml`。提权副本可能以另一个账户运行，`%USERPROFILE%` 和 mihomo 自己的默认配置目录都会跟着变。
- **内核一旦提权就一直提权**（TUN 生效之后）：「退出并停止 Mihomo」会再弹一次 UAC，由提权副本按记录下来的 PID 结束它；取消则只提示「提权启动被取消或失败」，程序不退出。
- **系统代理与 TUN 都不由 mihomo 核心管理**，`Internet Settings` 与监听端口分别由本程序处理；`mixed-port` 从控制器实时读取，不解析 yml。
- 长列表由系统滚动箭头 + 鼠标滚轮 + 方向键处理（构建菜单时设置了 `MIM_MAXHEIGHT`，这也是官方建议的做法——默认以屏幕高度为上限在多显示器下会失效）。
- 只读组（`LoadBalance` 与普通节点）的成员不可点击，仅展示当前值。
- `URLTest`/`Fallback` 组被点选后会进入「已固定」状态（组名带 `· 已固定`），此时内核不再自动测速换节点；用组内的「自动（取消固定）」（`DELETE /proxies/{name}`）恢复自动选择。固定状态由内核写进它自己的缓存，重载配置也会清除。
- **面板是本程序之外的东西**：「打开 Web 面板」只是把地址交给默认浏览器。要内核自己伺服一份静态面板，得在 mihomo 里配 `external-ui`（那份文件挂在控制器的 `/ui/` 下，正是默认地址）；用外部托管的面板（zashboard、metacubexd 等）则**不需要改 mihomo**，只要 `external-controller` 可达、在面板里填地址与 secret 即可——但那个页面与内核不同源时，mihomo 的 `external-controller-cors.allow-origins` 必须放行该来源，否则浏览器会拦在 CORS 上。控制器不可达时这一项与其他操作项一样灰显。
- Windows 11 默认把新的托盘图标收进溢出区，首次运行需要手动把它拖到任务栏固定。
- 本程序（托盘）始终以普通权限运行；只有 TUN 需要管理员权限：点「TUN 模式」而内核没能建起 Wintun 时，弹一次 UAC 让内核变成提权实例，托盘自身不提权。

## 与 Go 版 `mihomo-tray` 的差异

不做：设置窗口、自绘弹窗、节点延迟色点、流量曲线、订阅刷新、脚本执行；需要图形化的设置就用内核自己的面板（菜单里的「打开 Web 面板」）。
另外：停止内核只结束本程序启动或路径匹配的实例（Go 版用 `taskkill /IM mihomo.exe` 会误杀其他实例）；重载与状态读取均走控制器 API。

设计文档见 [docs/DESIGN.md](docs/DESIGN.md)。
