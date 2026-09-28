# 重启内核（设计与交接）

> 给下一次会话的起点文档。**实现前先把 §5 的待定项问清楚**（那是设计决策，不属于实现细节）。

## 1. 三个动作，别混在一起

| 动作 | 做什么 | 用什么 | 需要什么 |
|---|---|---|---|
| **重启内核** | 让当前内核进程重启一次（清内部状态、换二进制、配置大改） | mihomo API `POST /restart` | 控制器可达（要 secret） |
| **重载配置** | 原地重读配置文件，不换进程 | `PUT /configs?force=true`（**已实现**为菜单「重载配置」） | 同上 |
| **强制重启** | 停掉当前内核，按 `tray.yml` 重新拉起（"收养"别人的内核） | 我们自己停 + `discover::launch_args`；提权时经 `--kernel-start-elevated` | 停别人的内核要用户确认；提权内核要 UAC |

菜单里加「重启内核」走 API；「强制重启」是**另一个**动作，独立菜单项、独立确认。

## 2. 事实（源码 + 本机实测）

### 2.1 `POST /restart`

- 源码：`hub/route/restart.go`（`r.Post("/", restart)`），挂载见 `hub/route/server.go`：在**鉴权组内**，`embedMode` 下不挂载。
- 行为：先 `render.JSON({"status":"ok"})` + `Flush`，然后在 goroutine 里 `executor.Shutdown()`；**Windows 是 `exec.Command(execPath, os.Args[1:]...)` 后 `os.Exit(0)`**（不是 exec-replace），Unix 才是 `syscall.Exec`。
- 因此新进程完整继承 **argv / 环境 / 令牌 / 工作目录** ⇒ **提权保持、环境不丢**，提权路径既不需要 UAC 也不需要我们的 helper。
- 本机实测（v1.19.31，一次性内核，控制器 `127.0.0.1:9096` + secret）：

  ```
  POST /restart      → 200 {"status":"ok"}   429 ms
  控制器重新应答      → 445 ms
  新 PID 是旧 PID 的子进程；旧进程随后退出
  日志：Mihomo shutting down → restarting: "<exe>" ["-d" "<同一目录>"] → RESTful API listening at 127.0.0.1:9096
  ```

- **它不解决"设置不可知"**：重启后还是原来那份 argv/env，「别的启动器配的内核」不会变成我们的。归一要靠强制重启。

### 2.2 托盘侧现状（文档落笔时 `main` @ `9e98c14`）

- `Worker.client: Option<&Client>` 是**整个会话不可替换的借用**（`src/app.rs`）；`discover::find_controller` 只在启动时跑一次。
- `discover::launch_args`：展开 args → 拒绝 `-d/-f/-ext-ctl/-secret` → **总是**传绝对 `-d/-f` → 把 `CLASH_OVERRIDE_*` 具体化成 `-ext-ctl/-secret`。
- 已有可复用件：`wait_for_controller(Option<&Client>)`、`KERNEL_STARTUP_BUDGET`(5 s)、`status_note`（「正在启动内核…」/「内核尚未应答，仍在等待」）。
- 身份：`proc::pick_kernel(running, recorded_pid, known_path)`、`state::write_kernel_pid`、`state::kernel_path`、`state::take_kernel_child`。
- 提权副本：`--kernel-start-elevated <pid>`（停旧 + 按旧 argv 起）、`--kernel-stop-elevated <pid>`；只收 PID、退出码回传内核 PID（见 ROADMAP 硬约束 9）。

## 3. 「重启内核」流程

1. 需要 controller：没有就报「没有可用的控制器」（菜单项灰显）。
2. 状态注记「正在重启内核…」→ `POST /restart`。
3. **响应成功 ≠ 内核已起来**（代码先回包再重启）。
4. **清掉记录的 PID 与 child 句柄**：`state::write_kernel_pid(state, None)`，并丢掉 `KernelSlot` 里的 child。
   - 不清 PID 的后果很具体：退出路径会认为"内核 PID 还在"，于是走 `StopResult::NeedsAdmin` 多弹一次 UAC（`src/app.rs` 的 `stop_kernel`）。
   - 失效的 `Child` 目前有 `try_wait` 守卫兜着，但清掉更干净。
5. `wait_for_controller(client)`（5 s 预算）→ 超时报「内核重启后没有应答」。
6. **重新解析控制器并替换正在用的 client**（`discover::find_controller(settings)`）；刷新快照。
7. 重新按路径认内核：`pick_kernel` 能找到新 PID（新进程的 parent 已退出，`family` 逻辑无副作用）。

## 4. 改动清单（真正的成本在 client 热替换）

- `src/mihomo/api.rs`：`pub fn restart(&self) -> Result<(), String>`（`POST /restart`）。
- `src/app.rs`：`Command::RestartKernel` + `execute` 分支 + `restart_kernel()`（§3 七步）；**把 client 从"借用"改成"可替换"**（例如把 `Option<Client>` 放进共享状态，或重启后重建 worker）——这是本功能的主要重构，别低估。
- `src/i18n.rs` + `lang/en-US.yml`：菜单项、状态注记、失败文案（**三处同步**：`messages!` 列表、zh/en 表、语言文件，有单测盯着）。
- `README.md`（特性/已知行为）、`docs/DESIGN.md`（§4、§8）、本文档。
- 测试：`api.rs` 里用本地假控制器断言 `POST /restart` 的 method/path；重启流程能纯函数化的部分（清 PID → 等待 → 替换）单独测。

## 5. 「强制重启」的设计与要拍板的点

目标：把"别的启动器或上一次会话留下的内核"变成**按 `tray.yml` 跑的内核**（设置从此可知）。

- **身份与许可**：只有映像匹配 `mihomo.path`（`proc::same_image`）才静默做；否则弹一次确认，写明映像路径与 PID。**绝不 `taskkill /IM`**（ROADMAP 硬约束 6）。
- **提权保持**：提权内核必须由副本完成"停 + 起"，否则重启后掉权限、TUN 失效。
- **副本怎么知道"要起什么"**（硬约束 9 禁止命令行传 exe/args）：
  - **(a)** 调用方把值写进副本命令行 → 破掉"调用方不能规定以管理员启动什么"的性质；
  - **(b) 副本自己读 `tray.yml`（推荐）**：映像仍由配置决定、调用方只给 PID；代价是副本要带 `settings`/`paths`/`discover`，且它在提权账户下 `%APPDATA%` 不是你的（便携布局那份才可靠）。需要一个新模式（如 `--kernel-replace-elevated <pid>`）。
- **待定**：确认框措辞；是否要求"被替换的内核必须是我们认识的"；强制重启后是否清 `controller.*`（建议不清：它仍是"不动内核时"的兜底）；文档要写清二者关系——**重启/强制重启让内核变成我们认识的样子，`controller.*` 是不动内核、只改连接目标**。

## 6. 已知坑

- `POST /restart` 后旧 `Child` 指向已退出进程；记录的 PID 会误导退出路径（见 §3.4）。
- 响应先于重启返回；控制器有约 0.4 s 的不可用窗口，别把它当失败。
- Windows 上"子进程在父进程 `os.Exit` 之前 bind"的理论竞争：本轮实测控制器端口**没有**复现（445 ms 恢复，日志显示绑定成功）；要压测再下结论。
- `embedMode` 下没有 `/restart` 路由（CLI 内核不受影响）。
- 与别的启动器（clash-verge 等）打架：强制重启后对方可能再把它的内核拉起来。
- 无控制器时无法重启（菜单项灰显），此时唯一手段是强制重启。

## 7. 验收

- 自动化：`cargo fmt --check`、`cargo clippy --release --all-targets`、`cargo test --release`（当前 68 个，期望 +N）。
- 真机：① 普通内核重启（PID 变、设置不变、亚秒级恢复）；② 提权内核重启后**仍提权**（TUN 仍生效）；③ 无控制器时灰显/报错；④ 强制重启：让 `tray.yml` 指向另一份配置，确认重启后设置随 `tray.yml` 生效。

## 8. 起点信息

- 起点：`main` @ `9e98c14`（先 `git log -1` 看当前 tip；本文档之后可能已有新提交）。
- 工作方式：新 worktree `.worktrees/restart-kernel` + 分支 `feat/restart-kernel`；显式 `git add`（禁 `add -A`）；一主题一 commit；提交前跑三件套。
- 必读：`src/app.rs`（worker / `Command` / `refresh` / `stop_kernel`）、`src/mihomo/api.rs`、`src/mihomo/discover.rs`、`src/mihomo/proc.rs`、`docs/DESIGN.md` §4/§8、`docs/ROADMAP.md`（硬约束）。
- 可直接粘贴的开场说明：

  > 项目 `D:\App\github项目\mihomo-tray_rust`，先读 `docs/RESTART_KERNEL.md`（重启内核的设计与交接），按它的 §5 先把待定项问清楚，再动代码。工作方式按仓库约定：新 worktree + `feat/restart-kernel`，一主题一 commit，收尾三件套全绿。
