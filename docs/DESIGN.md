# mihomo-tray-rs 设计文档

Windows 系统托盘工具，用 Rust 管理本机 [mihomo](https://github.com/MetaCubeX/mihomo) 内核与其系统级代理设置。
目标：**二进制小、内存小、依赖少**（不使用 UPX 等压缩手段），功能以「托盘能做的操作」为边界，不复刻 Clash for Windows 的完整界面。

- 界面文案：走 `src/i18n.rs` 的语言表，默认中文（与既有 `mihomo-tray` Go 版一致），另内置英文；使用者可用 `lang/<系统语言>.yml` 覆盖，见 [README](../README.md) 的「界面语言」
- 代码注释 / commit / 本仓库文档语言：见 §11

---

## 1. 范围

### 1.1 功能（MVP）

| 菜单项 | 行为 |
|---|---|
| `Mihomo 状态: 运行中 (rule)` | 灰显状态行；未运行时显示 `未运行`，控制器不可达时显示 `控制器不可达` |
| `系统代理` | 写/清 `HKCU\...\Internet Settings` 的 `ProxyEnable`/`ProxyServer`/`ProxyOverride`，并通知 WinINet 刷新 |
| `代理模式 ▶` | `Rule` / `Global` / `Direct` 单选互斥（原生 radio 标记） |
| `TUN 模式` | `PATCH /configs` 后**回读** `tun.enable` 确认；失败提示（多因未提权） |
| `代理分组 ▶` | `GLOBAL` + 其余可切换组，每组一个子菜单；成员单选切换；只读组（`LoadBalance`/`Relay`）灰显当前值 |
| `开机自启动` | HKCU Run 键增删 |
| `重载配置` | `PUT /configs?force=true`，body `{"path":""}`（让 mihomo 重载它自己的配置文件） |
| `退出 ▶` | `退出并停止 Mihomo`（只结束本程序掌控的进程）/ `仅退出程序` |

附加（非菜单）：单实例互斥；资源管理器重启后自动重新注册托盘图标。

### 1.2 明确不做

设置窗口、自绘弹窗、owner-draw 视觉、节点延迟色点、流量/内存曲线、订阅 provider 刷新、脚本执行、CFW 式的 HTML 界面。

---

## 2. 技术选型（均有实测依据）

| 项 | 选择 | 依据 |
|---|---|---|
| 托盘 | `windows-sys` 直接 `Shell_NotifyIconW` | 与 `tray-icon`+`muda` 相比，实测 exe 160 KB vs 460 KB；菜单观感完全相同（muda 内部就是 `CreatePopupMenu`/`AppendMenuW`/`TrackPopupMenu`） |
| 菜单 | 原生 HMENU + `TrackPopupMenuEx(TPM_RETURNCMD)` | 免费获得定位、子菜单、长列表滚动、键盘导航、点外部关闭；`MFT_RADIOCHECK` 提供原生单选 ● |
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
重载配置
─────────────────────────────
退出 ▶
   退出并停止 Mihomo
   仅退出程序
```

实现要点：

- **命令 id 表**：每次构建菜单生成 `Vec<(u32, Target)>`，id 从 1000 递增（0 保留），`Target` 是 `Mode(..)` / `Tun` / `Group{group, member}` / `Reload` / `Exit(stop_kernel)`；菜单销毁即清空，不做文本反查。
- **单选**：`MF_CHECKED | MFT_RADIOCHECK`。
- **可切换组**：判据不是「`type` 是不是 `Selector`」而是「适配器是否实现 mihomo 的 `outboundgroup.SelectAble`」（`hub/route/proxies.go` 的 `updateProxy` 同此）。该集合恰好是 `Selector`/`URLTest`/`Fallback` 三种，`LoadBalance`/`Relay` 与普通节点会返回 `400 Must be a Selector`。非 `Selector` 的可切换组（自动组）项文本为 `名称 (类型)`，点击成员即 `PUT /proxies/{name}` 固定该节点；`/proxies` 的 `fixed` 非空时标签追加 `· 已固定`，并在成员列表顶部提供「自动（取消固定）」（`DELETE /proxies/{name}`）。
- **只读组**（`LoadBalance` 等）：整组不可点，项文本为 `名称 (类型)`。
- **组排序**：`/proxies` 是 Go map→JSON，顺序即字典序，mihomo 不提供配置顺序。故 `GLOBAL` 固定置顶，其余按不区分大小写字典序；`tray.yml` 的 `groups.order/include/exclude` 可覆盖。
- **菜单在每次右键时重建**：先同步拉 `/configs`（+ `/proxies`）再建菜单，数据永远新鲜；不依赖轮询快照。

### 3.1 长列表（节点很多的分组）

原生菜单在高度超过上限时，**由系统自动加顶/底滚动箭头**，并支持鼠标滚轮与键盘方向键滚动——这部分不需要自己实现。默认上限是「屏幕高度」，在多显示器、或菜单高于整屏时会失效（菜单被屏幕边缘直接裁掉、不出现箭头），因此构建菜单（含各级子菜单）时显式设置最大高度：

```c
MENUINFO mi = { 0 };
mi.cbSize = sizeof(mi);
mi.fMask  = MIM_MAXHEIGHT;
mi.cyMax  = min(工作区高度 * 0.6, 900);   // 物理像素
SetMenuInfo(hmenu, &mi);
```

实测（`docs/assets/native-menu-scroll-arrows.png`，Win11）：`cyMax = 700` 时菜单被截为 666 px，顶部 ▲ 灰显（已在列表开头）、底部 ▼ 可用，系统按状态自动置灰。

注意：CFW 截图里那个「悬停小箭头滚动」是它自绘 HTML 弹窗的行为，**不是**原生菜单能直接复刻的交互细节；原生菜单的等价能力是「系统滚动箭头 + 鼠标滚轮 + 方向键」。因此不把箭头作为唯一手段：

- 默认只设 `MIM_MAXHEIGHT`，让系统处理长列表；
- 若仍不好用，可设 `groups.page_size`（默认 `0` = 关闭）：成员数超过该值的分组自动拆成翻页子菜单（`1–50 ▶` / `51–100 ▶`），确定可达。

---

## 4. 发现链

三层来源不同，必须分开解析。

| 目标 | 顺序 |
|---|---|
| `mihomo.exe` | `tray.yml mihomo.path` → 运行中进程的真实映像路径（Toolhelp32 + `QueryFullProcessImageNameW`，跳过 scoop shim）→ exe 同级 / `bin` / `core` → `PATH` → `%USERPROFILE%\scoop\shims`、`$env:SCOOP\shims` |
| 配置 / 工作目录 | `tray.yml mihomo.config` / `mihomo.args`（`-d`、`-f`）→ `CLASH_HOME_DIR` + `CLASH_CONFIG_FILE` 拼绝对路径 → `%USERPROFILE%\.config\mihomo\config.yaml` |
| controller | `tray.yml controller.address` + `secret` → `CLASH_OVERRIDE_EXTERNAL_CONTROLLER` / `CLASH_OVERRIDE_SECRET` → 上述配置文件里的 `external-controller` / `secret` → **探测** `127.0.0.1:9090` 等常见端口并用 `GET /` 返回 `{"hello":"mihomo"}` 校验 |

> 本机实例：`~/.config/mihomo/config.yaml` 里没有 `external-controller`，源码也没有默认值，但 9090 在响应 —— 说明地址来自命令行/环境变量注入，因此**最后一层探测不能省**。

---

## 5. mihomo API 调用表

全部路径相对 controller 基址；`secret` 非空时加 `Authorization: Bearer <secret>`。

| 用途 | 调用 | 坑 |
|---|---|---|
| 探活 | `GET /` | 返回 `{"hello":"mihomo"}` |
| 读状态 | `GET /configs` | `mode`、`mixed-port`、`tun.enable`（后者是**已生效**值） |
| 切模式 | `PATCH /configs` `{"mode":"rule"}` | 会重建全部 inbound；`tun` 未变时提前返回，不断流 |
| 开关 TUN | `PATCH /configs` `{"tun":{"enable":true}}` | TUN 建立失败只写日志并把 enable 置 false，**HTTP 仍返回 204** → 必须回读确认；建 Wintun 需要管理员 |
| 重载配置 | `PUT /configs?force=true`，body `{"path":""}` | **空 body 会 400**；不带 `force` 不重建 inbound；`path` 为空时 mihomo 回落到自己启动时的配置文件 |
| 分组与节点 | `GET /proxies` | 顺序=字典序；`all` 非空的即分组；`history` 可能不存在（未测速） |
| 切换节点 | `PUT /proxies/{urlencode(group)}` `{"name":"member"}` | 组名/成员名必须 percent-encode（中文必需）；仅 `Selector`/`URLTest`/`Fallback` 可写，其余返回 400 |
| 取消固定 | `DELETE /proxies/{urlencode(group)}` | 只对非 `Selector` 的可写组有效（内核 `ForceSet("")`） |
| （Phase 2）测速 | `GET /group/{name}/delay?timeout=3000&url=…` | 实测返回 `{"成员":延迟}`，并让 `/proxies` 出现 `history` |
| （Phase 2）关闭连接 | `DELETE /connections` | |

---

## 6. 模块与线程模型

```
Cargo.toml                  # windows-sys feature 精确到项；release profile 见 §2
build.rs                    # 仅用于嵌入清单（或改用外部 .manifest，见 §9）
src/main.rs                 # 单实例 → 加载设置 → 发现内核/控制器 → 建托盘 → 消息循环
src/win/mod.rs              # 隐藏消息窗口、WM_APP 分发、TaskbarCreated 重注册
src/win/tray.rs             # Shell_NotifyIconW ADD/MODIFY/DELETE、tooltip、图标切换
src/win/menu.rs             # 菜单构建、id 表、TrackPopupMenuEx、深色模式
src/win/proxy.rs            # 系统代理注册表 + InternetSetOptionW(39/37)
src/win/autostart.rs        # HKCU Run
src/win/elevate.rs          # 管理员检测 + ShellExecuteW("runas") 重启
src/mihomo/api.rs           # WinHTTP 客户端 + 上述端点
src/mihomo/discover.rs      # §4 的三条发现链
src/mihomo/proc.rs          # 启动/停止内核（只停自己启动的 PID）
src/settings.rs             # tray.yml 读取（极简 YAML 子集）
src/state.rs                # 状态结构 + 轮询线程 + 唤醒 UI
src/icon.rs                 # RGBA → HICON（多尺寸、状态色）
```

- **UI 线程**：创建窗口/托盘/菜单并跑 `GetMessageW` 循环。所有菜单操作必须在此线程（Win32 菜单线程亲和）。
- **后台线程**：1 个轮询线程（默认 3 s，`stack_size(256 KB)`）拉 `/configs`，更新图标/tooltip，用 `PostMessageW(WM_APP+n)` 唤醒 UI；菜单项状态在下一次右键重建时同步更新。
- **点击处理**：`TrackPopupMenuEx` 返回命令 id 后**同步**执行（写注册表/发 HTTP 都在本机回环，毫秒级）；HTTP 失败不弹窗，写进 tooltip 并在下一轮自然覆盖。

---

## 7. 设置文件 `tray.yml`

位置：exe 同目录优先（便携），否则 `%APPDATA%\mihomo-tray\tray.yml`；也可 `--config <path>` 指定。
解析：`key: value`、一层嵌套、`#` 注释、`"` 或 `'` 引号；不支持列表内联/锚点/多行块。

```yaml
mihomo:
  path: ""                # 留空 = 自动发现
  args: []                # 额外启动参数，如 -d/-f
  config: ""              # 可选：显式配置文件路径（同时用于读取 external-controller）
  auto_start: true        # 启动时若内核未运行则拉起
controller:
  address: ""             # 如 "127.0.0.1:9090"；留空 = 自动发现
  secret: ""
  timeout_ms: 2000
proxy:
  bypass: []              # 追加到 ProxyOverride（始终包含 <local>）
  system_proxy_on_exit: keep   # keep | disable
groups:
  order: []               # 显式排序；其余按字典序
  include: []             # 留空 = 全部 Selector
  exclude: []
  page_size: 0            # 0 = 不翻页（长列表交给系统滚动箭头/滚轮）；>0 时超出则拆翻页子菜单
ui:
  poll_interval_ms: 3000
  dark_menu: auto         # auto | always | never
```

---

## 8. 状态与错误策略

- 状态来源优先级：controller 实时值 > `tray.yml` > 默认值。`mixed-port` 从 `GET /configs` 取（系统代理指向它），不解析 yml。
- 控制器不可达：状态行 `控制器不可达`，模式/TUN/分组项灰显；不自动重启内核（避免和外部管理方式打架），仅当 `mihomo.auto_start` 且进程确实不存在时才拉起。
- TUN 开启后回读仍为 `false` → tooltip 提示「TUN 未生效（通常需要管理员权限）」，菜单提供「以管理员身份重启」。
- 停止内核只针对本程序启动的 PID（记录 `HANDLE`/进程 ID），不使用 `taskkill /IM`，避免误杀其他实例。
- 系统代理：默认「退出时保持现状」（`keep`）；开启前保存 `ProxyServer`/`ProxyOverride` 快照，便于 Phase 2 的还原。

---

## 9. 构建与清单

- `[profile.release]`：`opt-level="z"`、`lto=true`、`codegen-units=1`、`panic="abort"`、`strip=true`、`incremental=false`。
- `#![windows_subsystem = "windows"]`（无控制台窗口）。
- 应用清单：`Microsoft.Windows.Common-Controls 6.0` + `supportedOS {8e0f7a12-…}`（Win10/11）+ `permonitorv2`。这决定菜单圆角、主题与 DPI 是否正确——已实测（见 `docs/assets/native-menu-light.png`、`docs/assets/native-menu-dark.png`）。
- 清单以外部文件 `<exe>.name.manifest` 或 `build.rs` 嵌入，Phase 1 决定（外部清单便于调试）。

---

## 10. 实施阶段

| 阶段 | 内容 | 状态 |
|---|---|---|
| Phase 0 | 技术验证：raw 托盘 + WinHTTP 调控制器 + 原生菜单 + 清单/深色；已产出体积/内存数据与菜单截图 | **已完成** |
| Phase 1 | MVP：§1.1 全部菜单项 + `tray.yml` + 三条发现链 + 单实例 + TaskbarCreated；真机复测体积/内存并截图 | **已完成** |
| Phase 2（可选） | 只读组 `now` 展示增强、`/group/{name}/delay` 测速项、`DELETE /connections`、退出时禁用系统代理、schtasks 免 UAC 自启 | 视需要 |

---

## 11. 实现记录（与设计的偏差）

1. **当前项用 ✔ 而不是 ●**：模式与分组当前值统一用 `MF_CHECKED`（用户要求「当前节点打对钩」），因此不需要 `MFT_RADIOCHECK`。分组的当前值是嵌套子组时，**子菜单项本身也带 ✔**（`MF_POPUP | MF_CHECKED`）。
2. **worker 先刷新再等待**：原实现先 `recv_timeout(poll)` 再刷新，导致首个菜单（3 s 前打开）显示空状态；现改为循环开头立刻刷新，实现中实测发现并修复。
3. **图标由代码生成**：`CreateIconIndirect` + 32bpp DIB，4× 超采样画圆环与中心点，尺寸取 `SM_CXSMICON`，无资源文件、无图像库。
4. **非 `Selector` 组也带类型**：`自动选择 (URLTest)`；早期版本按「`type == "Selector"` 才可切换」把 `URLTest`/`Fallback` 一起灰显了，实测这两个组在内核里同样接受 `PUT /proxies/{name}`，故改为按 `SelectAble` 判据、并补上「已固定 / 取消固定」。
5. **实测体积/内存**：exe 362 KB（i18n 之前 338 KB，增量见 §11.7），空闲私有内存 ~2.6–3.4 MB、工作集 ~15–18 MB（WinHTTP 内部线程已计入）。`serde_json` 实测约 33 KB，其余为 std 基线与本程序代码。
6. **单测 30 个**：HTTP 层用 `TcpListener` 起本地假控制器，走真实 WinHTTP 断言 method/path/body/Authorization/错误码；菜单层用 `GetMenuStringW`/`GetMenuState` 断言项顺序、勾选与灰显、项数预算与控制字符处理；i18n 层断言中英表键与占位符一一对应、随仓库分发的模板与内置英文表逐条一致、语言文件叠加与回退。
7. **界面文案集中到 `src/i18n.rs`**：原先前述文案散在 11 个文件里，现收进语言表（51 条），语言取 Windows UI 语言标签，`lang/<该标签>.yml` 叠加在内置表上。`settings::load` 相应改为返回结构化 `LoadError`，文案由调用方渲染——否则「读取 `tray.yml` 失败」本身没有语言可依。语言文件的解析**不复用** `settings::strip_comment`：它把空格后的 `#` 当注释、把未配对的引号当成开启的引号串，会静默截断 `Proxy #1` 这类译文，并把行尾注释当成译文显示。
   **代价实测**：exe 由 337,920 B 增至 361,984 B（+24 KB），远高于动工前估的 3–4 KB——语言表本体、51 路 `overlay`、22 个渲染方法与解析器各占一块。读取用的 `HashMap` 已换成线性扫描（51 条只在启动读一次），省回 4.5 KB；读取路径改用 `Vec<(String, String)>` 后不再把 SipHash 与哈希表代码链进这个以 KB 计的项目。

### 代码审查修复（第二轮，10 项 must-fix + 若干 should-fix）

| 问题 | 修复 |
|---|---|
| 动作/启动错误被静默丢弃（`pending_error` 从不进入快照） | 拆成 `action_error`（跨刷新保留，下一次动作清除）与 `controller_error`（可达即清），tooltip 与菜单统一取 `Snapshot::error()` |
| `groups.order` 被第二次排序覆盖而失效 | 合并成单一比较器：配置顺序 → `GLOBAL` → 字典序 |
| 互相引用的分组导致嵌套展开 ~G⁴，右键卡死 UI 线程 | `MAX_ITEMS = 1500` 全局项数预算，超限追加灰显「项目过多，已省略」并停止递归 |
| 重入守卫在 `dispatch` 之前清除，理论上可重入产生别名 `&mut App` | 守卫覆盖整个 `dispatch`（`ShellExecuteW`/`InternetSetOptionW` 会泵消息） |
| 「退出并停止 Mihomo」可能杀掉非本程序启动的实例 | 失败关闭：路径未知或 `OpenProcess` 被拒时不终止，并提示「无法确认归属」 |
| 「以管理员身份重启」与单实例互斥体竞争导致托盘消失 | 提权前先 `instance::release()`（失败则重新获取） |
| `szTip` 可能无 NUL 终止 | 按 UTF-16 单元截断到 127 并留终止位 |
| `mixed-port` 无检查强转 `u16`（70000 → 4464 并写进系统代理） | 越界过滤为 0 |
| `CreateDIBSection` 部分失败时泄漏 `HBITMAP`；AND mask 未初始化 | 失败路径释放，mask 传零填充缓冲 |
| 响应体无上限缓冲 | 8 MiB 上限，超出报「响应过大」 |
| 其它 | 中毒锁 `into_inner`、`WM_DESTROY` 删除图标、`NIM_SETVERSION` 失败即重试、uxtheme 优先按名取、mihomo 配置注释剥离识别引号、`Menu` 实现 `Drop`、启动等待内核限定 5 s 预算 |

### 尚未在自动化中验证、需人工确认的两点

- **菜单内的鼠标交互**（悬停展开子菜单、悬停/点击滚动箭头）：自动化向托盘图标窗口 `PostMessage` 弹出菜单时，隐藏窗口拿不到前台激活权，模拟输入无法驱动系统菜单内部循环。滚动箭头的**存在与状态**已验证（`docs/assets/native-menu-scroll-arrows.png`），真实点击托盘图标时应正常。
- **`PUT /configs?force=true`** 未在本机正在运行的内核上执行（避免改动用户正在使用的代理状态），仅依据上游源码（`hub/route/configs.go:408-444`）与 Go 版既有实现。

完整待办与人工验证清单见 [ROADMAP.md](ROADMAP.md)。

