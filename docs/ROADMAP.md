# 开发状态与后续计划

给下一条会话 / 子代理的交接文档。相关背景见 [README.md](../README.md) 与 [DESIGN.md](DESIGN.md)。

## 现状

Phase 1（MVP）已实现并真机验证：

- 8 项菜单功能（状态行 / 系统代理 / 代理模式 / TUN / 代理分组 / 开机自启动 / 重载配置 / 退出两项）全部可用
- 三层发现链（内核 exe、配置文件、控制器地址）在「配置文件里没有 `external-controller`、地址由环境变量注入」的机器上仍能正确定位
- `cargo fmt --check`、`cargo clippy --release --all-targets`、`cargo test --release` 全绿
- 实测：exe ~353 KiB（361,984 B）；空闲私有内存 2.6–3.4 MB，工作集 ~15 MB
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

> 说明：自动化向隐藏窗口 `PostMessage` 弹出菜单时，窗口拿不到前台激活权，模拟鼠标/键盘无法驱动系统菜单内部循环，因此第 1、2 项必须人工确认。

## Phase 2 候选（按价值/成本）

| 功能 | 价值 | 成本 | 备注 |
|---|---|---|---|
| 一键测速（`GET /group/{name}/delay`） | 中 | 小 | 测速后 `/proxies` 才出现 `history`，可顺带把延迟写进菜单文本 |
| 退出时禁用系统代理（`proxy.system_proxy_on_exit`） | 中 | 小 | 配置项已预留语义 |
| 关闭所有连接（`DELETE /connections`） | 低 | 小 | 一行 API |
| 订阅 provider 刷新（`PUT /providers/proxies/{name}`） | 中 | 小 | 需先读 `/providers/proxies` 展示 `updatedAt` |
| ~~schtasks 免 UAC 自启（含 TUN 开机即用）~~ | — | — | 已否决：计划任务服务可能被禁用。改为点 TUN 时按需提权（一次性辅助进程重启内核） |
| 节点延迟/健康色点（owner-draw 菜单项） | 中 | 大 | 观感提升明显，但要 `WM_MEASUREITEM`/`WM_DRAWITEM` 全套 |
| 设置界面 | 高 | 大 | 用 Tauri 会毁掉体积/内存优势；建议原生对话框或继续编辑 `tray.yml` |
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
9. **提权只走一次性辅助进程，且只收 PID**：同一个 exe 的 `--kernel-start-elevated` / `--kernel-stop-elevated` 隐藏模式，在单实例与窗口逻辑之前处理；内核的映像与启动参数必须由副本从那个进程自己读出（映像名必须是 `mihomo.exe`），**禁止**接受命令行传入的路径或参数——否则就是一个"UAC 弹窗写着本程序、实际以管理员运行任意程序"的提权原语。不得引入常驻提权进程、计划任务或服务。

## 关键事实速查

- mihomo API 的坑（空 body 400、`force=true` 才重建 inbound、TUN 失败仍返回 204、组名必须 percent-encode）：见 [DESIGN.md](DESIGN.md) §5
- TUN 提权链路（回读 → 提权副本重启内核 → 重试；取消 UAC 无副作用）：见 [DESIGN.md](DESIGN.md) §8
- 体积/内存实测与实现偏差：见 [DESIGN.md](DESIGN.md) §2、§11
