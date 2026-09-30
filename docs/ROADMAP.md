# 开发状态与后续计划

给下一条会话 / 子代理的交接文档。相关背景见 [README.md](../README.md) 与 [DESIGN.md](DESIGN.md)。

## 现状

Phase 1（MVP）已实现并真机验证：

- 9 项菜单功能（状态行 / 系统代理 / 代理模式 / TUN / 代理分组 / 开机自启动 / 重载配置 / 打开 Web 面板 / 退出两项）全部可用
- 内核自发现（同目录/`bin`/`core` → `PATH`，只搜 `mihomo.exe` 这个名字，命中 Scoop shim 时按 `mihomo.shim` 的 `path` 换成真内核；`mihomo.path` 填了就只认它，改名内核也支持），配置/控制器按 mihomo 自己的规则解析（`mihomo.home`/`config` 或内核带来的那份 + 运行内核的 argv → 环境变量 → 配置文件 → `controller.*` 兜底），没有端口探测；「配置文件里没有 `external-controller`、地址由命令行或环境变量注入」的机器同样能定位
- `cargo fmt --check`、`cargo clippy --release --all-targets`、`cargo test --release` 全绿（89 个单测）
- 实测（0.2.1，rustc 1.98.0）：exe 343,040 B（≈335 KiB）；空闲私有内存 2.2 MB（私有工作集）、工作集 ~15 MB（冷启动 20 s 的读数，见 [README.md](../README.md)）
- 验证方式：真机运行截图 [`app-menu-light.png`](assets/app-menu-light.png) / [`app-menu-dark.png`](assets/app-menu-dark.png) + 分组数据逐项比对 live API + 长列表滚动箭头（[`native-menu-scroll-arrows.png`](assets/native-menu-scroll-arrows.png)）
- 人工验证（2026-09）：清单 **#1–#12、#16 通过**，**#13–#15 待验证**；唯一与预期不符的是 #2 的长列表滚动方式（只有「点击/按住箭头 + 方向键」能用，见下节）

## 人工验证清单（自动化覆盖不到）

2026-09 真机逐项点过：**#1–#12、#16 通过**；**#13–#15 未验证**（仍按原预期待确认）。唯一与预期不符的是 #2 的长列表滚动，人工结果如下：

| 滚动手段 | 真机结果 |
|---|---|
| 点击 / 按住顶、底箭头 | 可滚（按页） |
| 方向键 | 可滚 |
| 悬停箭头 | **不滚（未实现）** |
| 鼠标滚轮 | **不滚（未实现）** |

也就是说原以为的「系统滚动箭头 + 滚轮 + 方向键」这套等价能力**只成立一半**。原因未经证实（能确定的只是：原生菜单 + 本程序当前实现——只设 `MIM_MAXHEIGHT`——下滚轮与悬停箭头都不生效），要补上只能放弃原生菜单（自绘弹窗）或额外接管消息循环，代价见 [DESIGN.md](DESIGN.md) §3.1 与本文件「Phase 2 候选」。长列表的实际可达性 = 点击/按住箭头 + 方向键 +（可选）`groups.page_size` 翻页。

| # | 操作 | 预期 |
|---|---|---|
| 1 | 鼠标右键托盘图标 | 弹出菜单；悬停「代理模式」「代理分组」应展开子菜单 |
| 2 | 长分组（节点多时）顶部/底部箭头 | 箭头出现；点击/按住箭头与方向键可滚动（悬停箭头、滚轮不滚，见上） |
| 3 | 「重载配置」 | 内核重载自己的配置文件（会真的执行 `PUT /configs?force=true`） |
| 4 | 「系统代理」 | 写 `HKCU\...\Internet Settings` 并即时生效（浏览器无需重启） |
| 5 | 「TUN 模式」 | 普通权限下点一次 → 弹一次 UAC，由提权副本重启内核，TUN 变开（图标转蓝）；取消 UAC → 状态不变且提示「提权启动被取消或失败」；内核已是提权实例时不弹 UAC |
| 6 | 「开机自启动」 | 写入并删除 `HKCU\...\Run\mihomo-tray` |
| 7 | 「退出并停止 Mihomo」 | 只结束本程序启动、或路径与发现结果一致的实例；内核是 TUN 提权的实例时会有第二次 UAC（取消则该菜单项只报错、程序继续运行） |
| 8 | Windows 11 托盘溢出区 | 首次运行需手动把图标拖到任务栏固定（系统行为） |
| 9 | 不重启程序，切换 Windows「应用」浅色/深色 | 菜单主题跟着变（浅↔深都生效） |
| 10 | 打开自动组（`URLTest`）子菜单并点一个节点 | 切换成功且组名出现 `· 已固定`；再点「自动（取消固定）」恢复自动 |
| 11 | 「打开 Web 面板」（留空 `ui.web_url`，内核配了 `external-ui`） | 默认浏览器打开 `http://<控制器地址>/ui/`；内核没配 `external-ui` 时页面 404（这是内核侧的事，不是本程序失败） |
| 12 | `ui.web_url` 指向托管面板（如 `https://board.zash.run.place/#/setup?hostname={host}&port={port}&secret={secret}`） | 打开的页面已连上当前内核；若浏览器报 CORS，需在内核的 `external-controller-cors.allow-origins` 里放行该来源 |
| 13 | 键盘：选中托盘图标（Tab/方向键走到通知区域）后按空格或回车 | 菜单同样弹出（`NIN_SELECT`/`NIN_KEYSELECT`）；Esc 或点外部关掉后，焦点回到图标上，再按一次空格仍能弹出 |
| 14 | 显示器缩放改成 150%/200%（或把图标拖到另一台不同 DPI 的显示器）后，等资源管理器重发 `TaskbarCreated`（重启 explorer.exe） | 重新注册的图标按当前 `SM_CXSMICON` 重画，不发虚 |
| 15 | 在主显示器右/下边缘打开菜单，且状态行很长（故意写一个长错误） | 菜单整体被移进工作区，不越出屏幕（`TPM_WORKAREA`） |
| 16 | 程序第一次启动后再启动一次 exe | 弹出「mihomo-tray 已在运行…」提示框后退出，而不是静默消失 |

> 说明：自动化向隐藏窗口 `PostMessage` 弹出菜单时，窗口拿不到前台激活权，模拟鼠标/键盘无法驱动系统菜单内部循环，因此 #1（悬停展开子菜单）、#2（箭头/滚轮）只能人工确认；#13（键盘激活与 Esc 后焦点回归）同样驱动不了，仍在等人工验证。

## 已知差距（2026-09 发布前审查记录）

### 已修复

| # | 位置 | 问题 | 修法 |
|---|---|---|---|
| 2 | `src/win/mod.rs` `show_context_menu` | 未带 `TPM_WORKAREA`，长状态行可能撑出工作区 | 标志加上（锚点钳制保留：它只决定菜单从哪出现，`TPM_WORKAREA` 才管菜单自身宽度） |
| 3 | `src/win/mod.rs` `on_tray_event` | 不处理 `NIN_SELECT`/`NIN_KEYSELECT`，取消后不 `NIM_SETFOCUS` | 键盘激活进同一条菜单路径；菜单关闭后 `App::refocus_icon` 把焦点还给通知区域（`NIN_KEYSELECT` 由已导出的 `NIN_SELECT \| NINF_KEY` 现算） |
| 4 | `src/app.rs` `on_taskbar_created` | 图标尺寸只在 `Icons::new()` 取一次 | `Icons::refresh_size()` 比对当前 `SM_CXSMICON`，变了就整套重画 |
| 5 | `src/win/autostart.rs` `is_enabled` | 只判 `Run` 值是否存在 | 与当前 exe 的带引号路径比对（路径展开后忽略大小写），被搬走的旧条目不再显示"已开启" |
| 7 | `src/mihomo/proc.rs` `image_path` | 520 单元固定缓冲，更长的映像路径记为"不可核验" | 起始 1024 单元、失败即翻倍（上限 32K，即 Windows 自身接受的最长路径）；单测用 4 单元起始缓冲证明会增长而不是判"不可核验" |
| 8 | `src/settings.rs` `page_size` | 未钳制 | 上限 `MAX_PAGE_SIZE = 1000`；`0` 与"非数字"都保持文档里的"不分页" |
| 10 | `src/i18n.rs` `fill` | 占位符从左到右整体替换，值里的 `{kind}` 会被交叉替换 | 单遍扫描模板、边扫边拼，替换结果不再回扫 |
| 11 | `src/main.rs` | 第二个实例静默退出 | `i18n::init()` 提到单实例判定之前，第二实例弹一次说明框（图标在哪）后退出 |
| — | `src/mihomo/api.rs` `Client::new` | `https://` 前缀被去掉后按明文连 | 明确拒绝（`ClientError::TlsUnsupported`），由 `discover` 给出专属文案；不再静默降级 |

> 本表的编号与上一节清单**不同**：#2 指 `TPM_WORKAREA`，对应清单 **#15**；#3 指键盘激活，对应清单 **#13**。这两条都还没在真机点过——#15 需要在一个长状态行、显示器边缘再点一次菜单确认没有越界；#13 需要真机按空格/回车开菜单、Esc 取消后确认焦点还在图标上（自动化点不到系统菜单内部循环，见上一节说明）。

### 决定不改（记录理由，避免下一轮重复讨论）

| # | 位置 | 问题 | 不改的理由 |
|---|---|---|---|
| 1 | `src/mihomo/api.rs` | 无 TLS 客户端（已由上面的显式拒绝覆盖） | 控制器在本机回环；要真支持 TLS 得引入 WinHTTP secure 标志与证书策略（自签名证书默认会被拒，得再做信任决策），收益与风险不成比例 |
| 6 | `src/win/menu.rs` | `AppendMenuW`/`CreatePopupMenu` 失败未检查 | 项数已由 `MAX_ITEMS = 1500` 封顶，失败等价于内存耗尽；要报错得把错误通道穿进 `Menu::build` → `Action`/`Snapshot`/i18n 四处，事后还只能报"菜单不完整" |
| — | `mihomo.auto_start` 与 `Run` 项 `mihomo-tray` | 两个名字都像"开机自启" | 不同设置、都有用：`mihomo.auto_start` 决定**内核**是否随本程序启动，`Run` 项 `mihomo-tray` 决定**本程序**是否随 Windows 启动（菜单里那项的勾选状态来自注册表，永远读的是后者）。两者都保留 |
| 9 | `src/i18n.rs` `fallback` | 语言文件出错时叠加在 en-US 之上 | 这是设计选择：缺的键一律落到内置英文表，不会出现"半个界面空白"。改成"以该语言内置表为底"需要同步改 README/本节与 `a_language_file_is_layered_on_the_fallback` 单测，属产品决策而非缺陷 |

## Phase 2 候选（按价值/成本）

| 功能 | 价值 | 成本 | 备注 |
|---|---|---|---|
| 一键测速（`GET /group/{name}/delay`） | 中 | 小 | 测速后 `/proxies` 才出现 `history`，可顺带把延迟写进菜单文本 |
| 退出时禁用系统代理（`proxy.system_proxy_on_exit`） | 中 | 小 | 配置项已预留语义 |
| ~~关闭所有连接（`DELETE /connections`）~~ | — | — | **已实现**（2026-09）：「更多 ▶ 关闭所有连接」，内核回 `204` 后连接由下个请求重建；需控制器，无则灰显 |
| ~~重启内核（`POST /restart`）~~ | — | — | **已实现**（2026-09，`feat/restart-kernel`）：「更多 ▶ 重启内核」。提权与环境原样保持，客户端改 owned `Option<Client>` 即可热替换——原估的"中等成本"其实是十处签名 |
| ~~强制重启（按 `tray.yml` 归一别人的内核）~~ | — | — | **已实现**（2026-09，同上）：「更多 ▶ 强制重启内核」，提权内核走新副本模式 `--kernel-replace-elevated <pid>`（自己读 `tray.yml`）。见 [RESTART_KERNEL.md](RESTART_KERNEL.md) |
| 订阅 provider 刷新（`PUT /providers/proxies/{name}`） | 中 | 小 | 需先读 `/providers/proxies` 展示 `updatedAt` |
| 长列表滚轮滚动 | 中 | 大 | 清单 #2 的未实现项：原生菜单 + 当前实现下滚轮与悬停箭头都不滚。要支持得换掉原生菜单（自绘弹窗）或额外接管消息循环（可行性本轮未验证），丢掉系统免费提供的定位/子菜单/键盘导航/点外部关闭；现状已由「点击/按住箭头 + 方向键 + `groups.page_size` 翻页」兜住 |
| ~~schtasks 免 UAC 自启（含 TUN 开机即用）~~ | — | — | 已否决：计划任务服务可能被禁用。改为点 TUN 时按需提权（一次性辅助进程重启内核） |
| 节点延迟/健康色点（owner-draw 菜单项） | 中 | 大 | 观感提升明显，但要 `WM_MEASUREITEM`/`WM_DRAWITEM` 全套 |
| 设置界面 | 高 | 大 | 用 Tauri 会毁掉体积/内存优势；建议原生对话框或继续编辑 `tray.yml`。**内核侧的图形化设置已由「打开 Web 面板」覆盖**，这里说的只剩本程序自己的 `tray.yml` |
| 手写 JSON 取值替代 `serde_json` | 低 | 中 | 实测只能省 ~33 KB，不推荐 |

## 硬约束（新会话/子代理必须遵守）

1. **工具链**：`edition = "2024"`、`rust-version = "1.85"`，不启用任何 nightly 特性。`unsafe fn` 内必须显式 `unsafe` 块（edition 2024 的 `unsafe_op_in_unsafe_fn`）。
2. **不加运行时依赖**：现仅 `windows-sys` / `serde_json` / `winreg`，构建期零依赖（清单走 linker 参数）。
3. **产物语言**：代码、注释、commit 英文；文档中文；界面文案一律走 `src/i18n.rs` 的语言表（默认 `zh-CN`、内置 `en-US`），禁止在业务代码里硬编码；新增或修改文案只改 `src/i18n.rs` 的内置英文表，`lang/en-US.yml` 由 `build.rs` 自动重写，不需要手工同步。
4. **收尾三件套必须全绿**：`cargo fmt --check`、`cargo clippy --release --all-targets`、`cargo test --release`。
5. **线程模型不得破坏**：UI 线程只做 Win32 与注册表操作；所有 HTTP 在 worker 线程；`muda`/`HMENU` 这类菜单对象只能在创建它的线程使用。
6. **停止内核不要用 `taskkill /IM`**：只结束自己启动或路径匹配的进程。
7. **控制器设置按 mihomo 自己的优先级解析**：运行内核 argv 的 `-ext-ctl`/`-secret` → `CLASH_OVERRIDE_*` 环境变量 → 内核配置文件里的 `external-controller`/`secret` → `tray.yml` 的 `controller.address`/`secret`（**兜底，不是覆盖**）。禁止再引入端口探测或"猜一个常见端口"，也禁止让 `controller.*` 反向压过内核设置（详见 DESIGN §4）。
8. **提交分组**：一个主题一个 commit；本机 git 一律 `-c core.autocrlf=false`（严格 LF）。
9. **提权只走一次性辅助进程，映像由副本从 PID 读出、用退出码回答**：同一个 exe 的 `--kernel-start-elevated` / `--kernel-stop-elevated` / `--kernel-replace-elevated` 隐藏模式，在单实例与窗口逻辑之前处理；**启动哪个映像必须由副本从那个进程自己读出**（`kernel_image`，不看名字——内核可以改名，而名字从来不是边界：调用方能把自己的文件叫 `mihomo.exe`。它只挡误用，如 PID 复用），**禁止**接受命令行传入的 exe 路径。启动参数：start 副本从那个进程的 argv 读（唯一的忠实来源），replace 副本收托盘给的参数（`tray.yml` 的唯一可读者是托盘：提权副本以另一个账户运行时未必看得见它，让它自己读反而会两边不一致）——**参数通道不是安全边界**（能改 `tray.yml` 的攻击者本来就能让 mihomo 执行任意动作），映像通道才是。启动成功时副本用**退出码回传新内核的 PID**（失败为负数），托盘记下来供后续停止使用；**禁止**用文件/管道回传（等于让管理员按调用者给的路径写文件）。路径比较一律走 `proc::same_image`（junction/大小写归一化），不得直接比字符串。不得引入常驻提权进程、计划任务或服务。
10. **先量后设计**：涉及进程身份、权限或路径的判断，先在同一台机器上实测一次 Win32 行为（映像路径、token、端口归属）再写逻辑；本轮三次误判（shim 拓扑、分类判定）都是没先量造成的。坑清单见 DESIGN §5.1。
11. **结论必须有直接证据**：`stopped > 0`、`denied > 0`、子句柄、菜单勾选都只是间接信号，曾造成"已停止"的假象；结论只能落在"进程还在不在"或"权威方（提权副本）怎么说"。

## 关键事实速查

- mihomo API 的坑（空 body 400、`force=true` 才重建 inbound、TUN 失败仍返回 204、组名必须 percent-encode）：见 [DESIGN.md](DESIGN.md) §5
- **Windows 侧的坑**（junction 导致映像路径与拼写不等、scoop shim 是启动器且与真内核同名的父子关系、`runas` 不传环境/可能换账户、退出码回传 PID、高 IL 进程"句柄能开路径被拒"）：见 [DESIGN.md](DESIGN.md) §5.1
- TUN 提权链路（回读 → 提权副本重启内核 → 重试；取消 UAC 无副作用）：见 [DESIGN.md](DESIGN.md) §8
- 重启内核 / 强制重启：实测数据、流程、待拍板项与起点信息：见 [RESTART_KERNEL.md](RESTART_KERNEL.md)
- 体积/内存实测与实现偏差：见 [DESIGN.md](DESIGN.md) §2、§11
