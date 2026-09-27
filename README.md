# mihomo-tray-rs

Windows 系统托盘工具，用 Rust 管理本机 [mihomo](https://github.com/MetaCubeX/mihomo) 内核与其系统级代理设置。
目标是**二进制小、内存小、依赖少**：原生 Win32 + WinHTTP，不引入 WebView、HTTP 客户端库或 YAML 库。

![托盘菜单](docs/assets/app-menu-light.png)

## 特性

- **状态行** — `Mihomo 状态: 运行中 (rule)` / `未运行` / `控制器不可达`
- **系统代理** — 写 `HKCU\...\Internet Settings` 并通知 WinINet 刷新（只写注册表不会即时生效）
- **代理模式** — Rule / Global / Direct 单选互斥，勾选当前值
- **TUN 模式** — 切换后回读 `tun.enable` 确认（失败通常是缺少管理员权限）
- **代理分组** — `GLOBAL` 与其余可切换组，逐层子菜单；成员打钩表示当前节点。`Selector`、`URLTest`、`Fallback` 的成员都可点（后两者的点击等于手动固定节点，菜单会多出「自动（取消固定）」）；`LoadBalance` 等真正只读的组灰显但展示当前值
- **开机自启动** — HKCU `Run` 键，不触发 UAC
- **重载配置** — `PUT /configs?force=true`，让内核重载它自己那份配置（本程序无需知道 yml 路径）
- **退出** — 「退出并停止 Mihomo」（只结束本程序掌控的实例）/「仅退出程序」
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
| 可执行文件 | ~330 KB (实测 336 KB) |
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
  poll_interval_ms: 3000
  dark_menu: auto     # auto | always | never
```

## 自动发现（三层，来源互不相同）

| 目标 | 顺序 |
|---|---|
| `mihomo.exe` | `mihomo.path` → 运行中进程的真实映像（跳过 scoop shim）→ exe 同级/`bin`/`core` → `PATH` → `$SCOOP\shims`、`~\scoop\shims` |
| 配置文件 | `mihomo.config` → `CLASH_HOME_DIR` + `CLASH_CONFIG_FILE` → `%USERPROFILE%\.config\mihomo\config.yaml` → exe 同级 |
| 控制器地址 | `controller.address` → `CLASH_OVERRIDE_EXTERNAL_CONTROLLER` / `CLASH_OVERRIDE_SECRET` → 上述配置文件里的 `external-controller` / `secret` → 端口探测（9090/9091/9097/9098/6170，用 `GET /` 校验） |

控制器地址不能只靠解析配置文件：它常由 `-ext-ctl` 或环境变量注入，所以最后一步的探测是必需的。

## 已知行为

- **重载配置不需要知道 yml 路径**：`PUT /configs?force=true` 且 `path` 为空时，内核回落到自己启动时的配置文件；`force=true` 才会重建 inbound 监听器。
- **TUN 失败不会返回 HTTP 错误**（内核只记日志并把 `enable` 置 false），因此本程序以回读 `GET /configs` 的结果为准；创建 Wintun 适配器需要管理员权限，菜单提供「以管理员身份重启」。
- **系统代理与 TUN 都不由 mihomo 核心管理**，`Internet Settings` 与监听端口分别由本程序处理；`mixed-port` 从控制器实时读取，不解析 yml。
- 长列表由系统滚动箭头 + 鼠标滚轮 + 方向键处理（构建菜单时设置了 `MIM_MAXHEIGHT`，这也是官方建议的做法——默认以屏幕高度为上限在多显示器下会失效）。
- 只读组（`LoadBalance` 与普通节点）的成员不可点击，仅展示当前值。
- `URLTest`/`Fallback` 组被点选后会进入「已固定」状态（组名带 `· 已固定`），此时内核不再自动测速换节点；用组内的「自动（取消固定）」（`DELETE /proxies/{name}`）恢复自动选择。固定状态由内核写进它自己的缓存，重载配置也会清除。
- Windows 11 默认把新的托盘图标收进溢出区，首次运行需要手动把它拖到任务栏固定。
- 本程序默认以普通权限运行；TUN 需要管理员权限，失败时菜单提供「以管理员身份重启」。

## 与 Go 版 `mihomo-tray` 的差异

不做：设置窗口、自绘弹窗、节点延迟色点、流量曲线、订阅刷新、脚本执行。
另外：停止内核只结束本程序启动或路径匹配的实例（Go 版用 `taskkill /IM mihomo.exe` 会误杀其他实例）；重载与状态读取均走控制器 API。

设计文档见 [docs/DESIGN.md](docs/DESIGN.md)。
