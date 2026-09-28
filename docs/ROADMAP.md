# 开发状态与后续计划

给下一条会话 / 子代理的交接文档。相关背景见 [README.md](../README.md) 与 [DESIGN.md](DESIGN.md)。

## 现状

Phase 1（MVP）已实现并真机验证：

- 9 项菜单功能（状态行 / 系统代理 / 代理模式 / TUN / 代理分组 / 开机自启动 / 重载配置 / 打开 Web 面板 / 退出两项）全部可用
- 内核/配置/控制器都按 mihomo 自己的规则解析（`mihomo.path`/`home`/`config` + 运行内核的 argv → 环境变量 → 配置文件 → `controller.*` 兜底），没有搜索、没有端口探测；「配置文件里没有 `external-controller`、地址由命令行或环境变量注入」的机器同样能定位
- `cargo fmt --check`、`cargo clippy --release --all-targets`、`cargo test --release` 全绿（68 个单测）
- 实测：exe 373 KiB（381,952 B）；空闲私有内存 2.6–3.4 MB，工作集 ~15 MB
- 验证方式：真机运行截图（`assets/app-menu-light.png`）+ 分组数据逐项比对 live API + 长列表滚动箭头（`assets/native-menu-scroll-arrows.png`）

## 需要人工点一遍的清单（自动化覆盖不到）

| # | 操作 | 预期 |
|---|---|---|
| 1 | 鼠标右键托盘图标 | 弹出菜单；悬停「代理模式」「代理分组」应展开子菜单 |
| 2 | 长分组（节点多时）顶部/底部箭头 | 悬停或按住箭头可滚动；滚轮、方向键同样可滚 |
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

> 说明：自动化向隐藏窗口 `PostMessage` 弹出菜单时，窗口拿不到前台激活权，模拟鼠标/键盘无法驱动系统菜单内部循环，因此第 1、2 项必须人工确认。

## 已知差距（发布前审查记录，未修复）

均为审查确认存在、但影响面小或需要真机交互验证才能定论的问题，留待后续：

| # | 位置 | 问题 | 影响 |
|---|---|---|---|
| 1 | `src/mihomo/api.rs` `Client::new` | `https://` 前缀被去掉后按明文 http 连（不带 `WINHTTP_FLAG_SECURE`） | 已在 README/DESIGN §8 记为已知限制；要支持 TLS 需加 secure 标志与证书策略 |
| 2 | `src/win/mod.rs` `TrackPopupMenuEx` | 未带 `TPM_WORKAREA`，长状态/错误行只靠锚点钳制 | 菜单可能横向越出工作区；需真机点一次确认是否真发生 |
| 3 | `src/win/mod.rs` `on_tray_event` | 只处理 `WM_*BUTTONUP`/`WM_CONTEXTMENU`，未处理 `NIN_SELECT`/`NIN_KEYSELECT`，也未在取消后 `NIM_SETFOCUS` | 键盘（空格/回车）无法打开菜单；需真机验证 |
| 4 | `src/app.rs` `on_taskbar_created` | 图标像素尺寸只在 `Icons::new()` 取一次；主显示器 DPI 变化时 `TaskbarCreated` 也会广播 | 重新注册的图标可能按旧尺寸缩放而偏糊 |
| 5 | `src/win/autostart.rs` `is_enabled` | 只判断 `Run` 值是否存在，不比对当前 exe 路径 | 程序被移动后仍显示"已开启" |
| 6 | `src/win/menu.rs` | `AppendMenuW`/`CreatePopupMenu` 失败未检查 | 极端情况下菜单静默少项 |
| 7 | `src/mihomo/proc.rs` `image_path` | 520 单元缓冲区不够长（>520 字符的映像路径）时记为"不可核验" | 会被当成"可能是我们的内核"，多弹一次 UAC |
| 8 | `src/settings.rs` `page_size` | 未像 `timeout_ms`/`poll_interval_ms` 那样钳制，非数字静默变 0 | 无实际危害，仅缺诊断 |
| 9 | `src/i18n.rs` `fallback` | 语言文件存在但为空/节名写错时，叠加在 en-US 之上 | 中文系统的用户文件写错会看到英文界面（README 已说明回退规则） |
| 10 | `src/i18n.rs` `fill` | 占位符从左到右整体替换，组名里含 `{kind}` 这类文本会被交叉替换 | 面板/分组名恰好含占位符文本时显示异常 |
| 11 | `src/main.rs` | 第二个实例静默退出，不提示 | Windows 11 托盘溢出区里用户可能以为"没启动" |

## Phase 2 候选（按价值/成本）

| 功能 | 价值 | 成本 | 备注 |
|---|---|---|---|
| 一键测速（`GET /group/{name}/delay`） | 中 | 小 | 测速后 `/proxies` 才出现 `history`，可顺带把延迟写进菜单文本 |
| 退出时禁用系统代理（`proxy.system_proxy_on_exit`） | 中 | 小 | 配置项已预留语义 |
| 关闭所有连接（`DELETE /connections`） | 低 | 小 | 一行 API |
| 订阅 provider 刷新（`PUT /providers/proxies/{name}`） | 中 | 小 | 需先读 `/providers/proxies` 展示 `updatedAt` |
| ~~schtasks 免 UAC 自启（含 TUN 开机即用）~~ | — | — | 已否决：计划任务服务可能被禁用。改为点 TUN 时按需提权（一次性辅助进程重启内核） |
| 节点延迟/健康色点（owner-draw 菜单项） | 中 | 大 | 观感提升明显，但要 `WM_MEASUREITEM`/`WM_DRAWITEM` 全套 |
| 设置界面 | 高 | 大 | 用 Tauri 会毁掉体积/内存优势；建议原生对话框或继续编辑 `tray.yml`。**内核侧的图形化设置已由「打开 Web 面板」覆盖**，这里说的只剩本程序自己的 `tray.yml` |
| 手写 JSON 取值替代 `serde_json` | 低 | 中 | 实测只能省 ~33 KB，不推荐 |

## 硬约束（新会话/子代理必须遵守）

1. **工具链**：`edition = "2024"`、`rust-version = "1.85"`，不启用任何 nightly 特性。`unsafe fn` 内必须显式 `unsafe` 块（edition 2024 的 `unsafe_op_in_unsafe_fn`）。
2. **不加运行时依赖**：现仅 `windows-sys` / `serde_json` / `winreg`，构建期零依赖（清单走 linker 参数）。
3. **产物语言**：代码、注释、commit 英文；文档中文；界面文案一律走 `src/i18n.rs` 的语言表（默认 `zh-CN`、内置 `en-US`），禁止在业务代码里硬编码；新增或修改文案要同步 `lang/en-US.yml`。
4. **收尾三件套必须全绿**：`cargo fmt --check`、`cargo clippy --release --all-targets`、`cargo test --release`。
5. **线程模型不得破坏**：UI 线程只做 Win32 与注册表操作；所有 HTTP 在 worker 线程；`muda`/`HMENU` 这类菜单对象只能在创建它的线程使用。
6. **停止内核不要用 `taskkill /IM`**：只结束自己启动或路径匹配的进程。
7. **不要靠解析 `config.yaml` 推断控制器地址**：它常由 `-ext-ctl` / 环境变量注入，解析只能作为兜底，最终以 `GET /` 探测为准。
8. **提交分组**：一个主题一个 commit；本机 git 一律 `-c core.autocrlf=false`（严格 LF）。
9. **提权只走一次性辅助进程，且只收 PID、用退出码回答**：同一个 exe 的 `--kernel-start-elevated` / `--kernel-stop-elevated` 隐藏模式，在单实例与窗口逻辑之前处理；内核的映像与启动参数必须由副本从那个进程自己读出（映像名必须是 `mihomo.exe`），**禁止**接受命令行传入的 exe 路径或启动参数——否则就是一个"UAC 弹窗写着本程序、实际以管理员运行任意程序"的提权原语。启动成功时副本用**退出码回传新内核的 PID**（失败为负数），托盘记下来供后续停止使用；**禁止**用文件/管道回传（等于让管理员按调用者给的路径写文件）。路径比较一律走 `proc::same_image`（junction/大小写归一化），不得直接比字符串。不得引入常驻提权进程、计划任务或服务。
10. **先量后设计**：涉及进程身份、权限或路径的判断，先在同一台机器上实测一次 Win32 行为（映像路径、token、端口归属）再写逻辑；本轮三次误判（shim 拓扑、分类判定）都是没先量造成的。坑清单见 DESIGN §5.1。
11. **结论必须有直接证据**：`stopped > 0`、`denied > 0`、子句柄、菜单勾选都只是间接信号，曾造成"已停止"的假象；结论只能落在"进程还在不在"或"权威方（提权副本）怎么说"。

## 关键事实速查

- mihomo API 的坑（空 body 400、`force=true` 才重建 inbound、TUN 失败仍返回 204、组名必须 percent-encode）：见 [DESIGN.md](DESIGN.md) §5
- **Windows 侧的坑**（junction 导致映像路径与拼写不等、scoop shim 是启动器且与真内核同名的父子关系、`runas` 不传环境/可能换账户、退出码回传 PID、高 IL 进程"句柄能开路径被拒"）：见 [DESIGN.md](DESIGN.md) §5.1
- TUN 提权链路（回读 → 提权副本重启内核 → 重试；取消 UAC 无副作用）：见 [DESIGN.md](DESIGN.md) §8
- 体积/内存实测与实现偏差：见 [DESIGN.md](DESIGN.md) §2、§11
