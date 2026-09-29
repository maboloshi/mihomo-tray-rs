# 重启内核（设计与实现记录）

> 本功能已实现并落在 `main` 上（`feat/restart-kernel`，六个主题提交：client 改 owned → 「更多」子菜单 → `重启内核` → `强制重启`（含提权副本模式）→ 文档）。§5 的待定项已按其中的「拍板记录」落地；§7 的真机部分仍需人工过一遍。

## 1. 三个动作，别混在一起

| 动作 | 做什么 | 用什么 | 需要什么 |
|---|---|---|---|
| **重启内核** | 让当前内核进程重启一次（清内部状态、换二进制、配置大改） | mihomo API `POST /restart` | 控制器可达（要 secret） |
| **重载配置** | 原地重读配置文件，不换进程 | `PUT /configs?force=true`（**已实现**为菜单「重载配置」） | 同上 |
| **强制重启** | 停掉当前内核，按 `tray.yml` 重新拉起（"收养"别人的内核） | 我们自己停 + `discover::launch_args`；提权时经 `--kernel-replace-elevated` | 停别人的内核要用户确认；提权内核要 UAC |

菜单里加「重启内核」走 API；「强制重启」是**另一个**动作，独立菜单项、独立确认。

## 2. 事实（源码 + 本机实测）

### 2.1 `POST /restart`

- 源码：`hub/route/restart.go`（`r.Post("/", restart)`），挂载见 `hub/route/server.go`：在**鉴权组内**，`embedMode` 下不挂载。
- 行为：先 `render.JSON({"status":"ok"})` + `Flush`，然后在 goroutine 里 `executor.Shutdown()`；**Windows 是 `exec.Command(execPath, os.Args[1:]...)` 后 `os.Exit(0)`**（不是 exec-replace），Unix 才是 `syscall.Exec`。
- 因此新进程完整继承 **argv / 环境 / 令牌 / 工作目录** ⇒ **提权保持、环境不丢**，提权路径既不需要 UAC 也不需要我们的 helper。
- 本机实测（v1.19.31；两次都用一次性内核 + 独立端口，跑完即清理，没碰你自己的内核）：

  ```
  第一次（控制器 9096，secret 写在配置文件里）
    POST /restart            → 200 {"status":"ok"}   429 ms   ← 冷启动的首个请求
    控制器重新应答            → 445 ms
    新 PID 是旧 PID 的子进程；旧进程随后退出

  第二次（控制器 9097；secret 只放在 CLASH_OVERRIDE_SECRET 里，配置文件不写；额外带一个 -m）
    PUT /configs?force=true  → 204，PID 不变（这就是 reload 与 restart 的分界）
    POST /restart            → 200 {"status":"ok"}   60 ms
    控制器重新应答            → 76 ms
    新命令行                  = 旧命令行（"-d <dir> -m" 原样）
    无 auth 访问              → 重启后仍 401 ⇒ 环境变量确实随重启保留
    连发 5 次                 → 回包 6/5/7 ms（第 1、5 次 684/509 ms）
                                恢复 13/10/13 ms（同两次 707/515 ms）
    9097 的监听进程            → 始终只有 1 个；日志里 6 次全部 "RESTful API listening at: 127.0.0.1:9097"，
                                没有 listen error / bind 失败
  ```

- **恢复时间不是固定值**：快的时候 ~10 ms，慢的时候 0.5–0.7 s（冷启动、系统繁忙时）。所以「响应 ≠ 已起来」按"可能"写，托盘必须等待 + 状态注记，不能假设一个固定窗口。
- **未实测**：提权内核重启后是否仍提权。机制上必然（Windows 子进程继承父进程的令牌，Go 的 `exec.Command` 也不改令牌），但要先有提权内核 + 一次 UAC 同意才能测，本轮没做——留给真机验收（§7 第②项）。

- **它不解决"设置不可知"**：重启后还是原来那份 argv/env，「别的启动器配的内核」不会变成我们的。归一要靠强制重启。

### 2.2 改动前的托盘侧现状（起点 `main` @ `9e98c14`）

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

实现时的三点与草稿不同，都是有意的：

- **先重新解析、再等待**（草稿是反的）：重启沿用 argv，地址本来就不会变；先解析才能顺带覆盖"内核这次从配置文件读到了另一个控制器"的情况，然后在**当前**的 client 上等应答。
- **解析失败不丢 client**：地址仍然有效（argv 没变），把它换成"没有控制器"反而会把一次成功的重启说成失败；下一次刷新照旧会报告控制器不可达。
- **一并清掉 `Worker` 里缓存的 `version`**（它原本只取一次，见 `refresh`）：重启可能正是在换二进制。

## 4. 改动清单（实际落点）

- `src/mihomo/api.rs`：`Client::restart()`（`POST /restart`）。
- `src/app.rs`：`Command::RestartKernel` / `Action::RestartKernel` + `restart_kernel()`（§3 七步）；`Worker.client` 由 `Option<&Client>` 改为 owned `Option<Client>`，并把 `version` 缓存搬进 `Worker` 一起替换。**"client 热替换"的实际成本远小于草稿估计**：`Client` 本来就是 `Clone`，`discover::find_controller` 本来就返回 owned 值，只是 worker 一直在借它。
- `src/app.rs`：`Command::ForceRestartKernel` + `force_restart_kernel()` / `replace_kernel()`（本地"停 + 起"）/ `elevate_replacement()`（交提权副本）/ `adopt_new_kernel()`（重解析 + 等应答）/ `confirm_foreign_kernel()`。
- `src/win/mod.rs`：`confirm()`（Yes/No，默认「否」；在 worker 线程上弹，与 UAC 一样不占用 UI 线程）。
- `src/mihomo/proc.rs` + `src/main.rs` + `src/win/elevate.rs`：`--kernel-replace-elevated <pid> <参数…>` 副本（映像自己从 PID 读，参数收托盘的）、`kernel_replace_params()`、`proc::quoted_args()`（`ShellExecuteExW` 只能收一个字符串，所以参数的边界要靠引用往返）。
- `src/win/menu.rs`：「更多」子菜单（`重载配置` / `重启内核` / `强制重启内核`）；子菜单在"有控制器**或**有内核在跑"时可展开，项级灰显各按自己的条件。
- `src/i18n.rs` + `lang/en-US.yml`：菜单项、状态注记、失败文案、确认框（**四处同步**：`messages!` 列表、zh/en 表、语言文件；带占位符的模板配一个 `impl Messages` 里的渲染方法，单测盯着键与占位符一一对应）。
- `README.md`（特性/已知行为）、`docs/DESIGN.md`（§3、§5、§8）、本文档。
- 测试（+3，共 71 个）：`api.rs` 用本地假控制器断言 `POST /restart` 的 method/path；`app.rs` 用一个**常驻**假控制器跑完整重启流程（清 PID/句柄、替换 client、清 version —— `api.rs` 那个单发假服务器演不了这个）；`menu.rs` 断言「更多」的内容与两档灰显。

## 5. 「强制重启」的设计（已按下列决定实现）

目标：把"别的启动器或上一次会话留下的内核"变成**按 `tray.yml` 跑的内核**（设置从此可知）。

- **身份与许可**：记录的 PID 匹配，或映像匹配本程序解析出的内核（声明或自发现的结果，`proc::same_image`）才静默做；否则弹一次 Yes/No（默认按钮「否」），写明映像路径与 PID。**绝不 `taskkill /IM`**（ROADMAP 硬约束 6）：只结束选中的那个进程及其启动器家族。
- **提权保持**：提权内核必须由副本完成"停 + 起"，否则重启后掉权限、TUN 失效。
- **副本怎么知道"要起什么"**（硬约束 9：映像不可由调用方指定）：
  - **(a) 否决**：调用方把**映像**写进副本命令行 → 破掉"调用方不能规定以管理员启动什么"的性质，与硬约束 9 冲突；
  - **(b) 旧实现（已废）**：副本自己读 `tray.yml`。代价是它在提权账户下 `%APPDATA%` 不是你的，只有 exe 旁的便携那份一定可见；读不到时回 `-7`（`HELPER_NO_SETTINGS`）并保持旧内核不动——而"参数由调用方给"其实与"副本读 tray.yml"安全性相同（能改 `tray.yml` 的攻击者本来就能让 mihomo 执行任意动作），所以这层代价白付；
  - **(c) 已实现**：映像由副本从 PID 自己读（`kernel_image`，**不看名字**——内核可以改名，`mihomo.path` 是可配置的；名字检查只挡误用，不挡攻击者），参数由托盘算好传进去，新模式 `--kernel-replace-elevated <pid> <参数…>`。跨账户可见性问题整类消失，`HELPER_NO_SETTINGS` 随之删除。**注意**：提权场景下强制重启只换配置与参数，不换 binary（映像还是那个内核自己的）。
- **拍板记录**（本次会话，用户决定）：
  - 「更多」= `重载配置` / `重启内核` / `强制重启内核`；`打开 Web 面板`、`开机自启动`、`退出` 留在根菜单；`强制重启内核` 无内核在跑时灰显。
  - 确认框只在运行内核的映像 ≠ `mihomo.path` 或读不出映像时出现（含提权内核）。
  - **不清** `controller.*`：它仍是"不动内核、只改连接目标"的兜底。
  - `README.md` 的菜单截图（`docs/assets/app-menu-light.png`）本次不动，由用户事后重截。
- 三者关系（文档与 README 都要写清）：**重启/强制重启让内核变成我们认识的样子；`controller.*` 是不动内核、只改连接目标**。

## 6. 已知坑

- `POST /restart` 后旧 `Child` 指向已退出进程；记录的 PID 会误导退出路径（见 §3.4）。
- 响应先于重启返回，所以 **200 不代表内核已起来**；实测恢复时间 10 ms ～ 0.7 s 不等（连发 5 次里 3 次 ~10 ms、2 次 ~0.5–0.7 s），托盘必须等待（`wait_for_controller`），不要假设固定窗口。
- Windows 上"子进程在父进程 `os.Exit` 之前 bind"的理论竞争：**6 次重启（含 5 次连发）都未复现**——控制器端口每次都重新绑定成功、日志无 `listen error`，同一时刻只有一个监听进程。仍属"没证明不存在"，异常时先看内核日志。
- `embedMode` 下没有 `/restart` 路由（CLI 内核不受影响）。
- 与别的启动器（clash-verge 等）打架：强制重启后对方可能再把它的内核拉起来。
- 无控制器时无法重启（菜单项灰显），此时唯一手段是强制重启。
- 「强制重启」期间要结束一个**不是本程序启动的**进程，所以它只认"选中的那一个 + 它的启动器家族"：别的 `mihomo.exe` 只要映像不同就不碰（`pick_kernel` 从不选可读但路径不同的实例）。
- 提权副本不再读 `tray.yml`（见 §5 的(c)），所以"跨账户看不到设置"这类失败不再存在；参数由托盘给出，映像必须仍是那个内核自己的（`mihomo.exe`），否则回 `-2` 且旧内核保持不动。

## 7. 验收

- 自动化：`cargo fmt --check`、`cargo clippy --release --all-targets`、`cargo test --release`（71 个，原 68 +3）。
- 真机（**待人工过一遍**）：① 普通内核重启（PID 变、设置不变、亚秒级恢复）；② 提权内核重启后**仍提权**（TUN 仍生效）；③ 无控制器时「重启内核」灰显、而「强制重启内核」仍可用；④ 强制重启：让 `tray.yml` 指向另一份配置，确认重启后设置随 `tray.yml` 生效；⑤ 让别的启动器拉起内核（映像不同）→ 弹确认框、选「否」时什么都不发生；⑥ 提权内核的强制重启走一次 UAC，UAC 后 TUN 仍生效。
- 需要人工重截：`docs/assets/app-menu-light.png`（菜单结构变了两处：根菜单少了「重载配置」、多了「更多」）。

## 8. 本次落地信息（给下一次会话）

- 起点：`main` @ `68dbf20`（文档里原来的 `9e98c14` 已过期）。
- 落地：**已在 `main` 上**（`--ff-only`，主线 tip `4fe2dc7`，分支 `feat/restart-kernel` 与 worktree `.worktrees/restart-kernel` 已清理）。六个提交：client 改 owned → 「更多」子菜单 → `重启内核` → `强制重启`（本地路径）→ 提权替换副本 → 文档。
- 提交前必过：三件套 + 本仓库 `rust-deterministic-gate` 的本地闭环（fmt/clippy 输出不回传）。
- 下一轮若继续改这块：从**当前主线 tip** 开新 worktree（不要复用已删除的分支名）；本仓库主线要求线性历史，`merge --ff-only`，后落地者负责 rebase。
- 必读：`src/app.rs`（worker / `Command` / `refresh` / `stop_kernel` / `restart_kernel` / `force_restart_kernel`）、`src/mihomo/api.rs`、`src/mihomo/discover.rs`、`src/mihomo/proc.rs`、`docs/DESIGN.md` §3/§5/§8、`docs/ROADMAP.md`（硬约束 9 是提权路径的红线）。
