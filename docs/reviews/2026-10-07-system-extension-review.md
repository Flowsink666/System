# 当前项目检查与拓展路线

## 后续修复状态（2026-10-07）

下文保留首次检查时的证据；其中 SYS_YIELD、唤醒匹配和裸指针页清零问题已修复。

- syscall 的 fork、exit、yield、sleep 统一调用 Kernel；调度结果由 Kernel 加载 CPU 上下文、保存现场，并在最后一个 tick 完成后调用统一退出清理。
- yield 优先让其他就绪进程运行，不消耗 tick、burst 或执行时间；sleep(0) 使用相同语义。原先“yield 消耗最后一个 tick 后回收”的验证已改为检查“不执行、不提前退出；实际最后一个 tick 后正常回收”。
- Sleeping 携带事件 token，WaitingChild 携带子进程 PID；唤醒要求完全匹配。退出取消睡眠事件，重复注册替换旧截止时间，溢出注册报错且不改变进程状态。
- Buddy 不再保存裸指针。RAM 改为私有；MemoryManager::allocate_pages_zeroed 使用安全切片填零。AddressSpace::allocate_and_map 和 fork_clone 现在接受 &mut MemoryManager；物理数据通过 read_physical/write_physical 访问。
- tests/review_regressions.rs 接入原 19 个与追加 3 个场景；tests/lifecycle_regressions.rs 增加 14 个验证。全部 51 个测试和严格 Clippy 通过；本次修改的 Rust 文件完成格式化检查。格式化与 Clippy 在受限环境中遇到配置访问拒绝，使用经自动审批允许的检查命令完成。

system-extension-probes-output.txt 已更新为修复后的通过结果。下面的“追加诊断”段落描述修复前发现过程，当前源码已是回归验证。

检查日期：2026-10-07。对象是当前工作目录，包含已有的 21 个未提交修改文件；本次没有修改业务源码，新增本报告和三个诊断验证及其输出。此前的架构报告描述的是修复前状态，不能直接当作当前缺陷清单。

## 当前定位与验证

项目是使用 Rust 标准库、运行在宿主机上的内核机制模拟器。Buddy/Slab、页表/TLB、CFS、VFS/块缓存、syscall、Shell 和 benchmark 已形成清晰模块结构。VirtualCpu::step 目前仅增加周期与 RIP，没有读取和解释程序指令；Ring0/Ring3 是模拟状态。现阶段适合继续发展可运行工作负载的操作系统实验平台。

| 验证 | 本次结果 |
| --- | --- |
| cargo test --all-targets --offline | 15 个集成测试通过；lib/bin 单元测试为 0 |
| 原架构验证重新编译运行 | 19/19 通过，确认原报告复现的问题已获得相应修复 |
| cargo clippy --all-targets --offline -- -D warnings | 通过 |
| cargo fmt --all -- --check | 命令报告“拒绝访问 (os error 5)”，未能验证格式状态 |
| cargo run --release --offline -- --bench | 完成；单轮结果仅用于检查指标含义 |
| 本次追加诊断 | 三个场景全部复现缺陷，详见下文 |

编译器：rustc 1.98.1。原 19 个验证位于 docs/reviews，普通 cargo test 不会运行它们。本次新增诊断也暂放 docs 下，未作为默认必过测试接入。

## 拓展前应修复的约束

### 1. SYS_YIELD 绕过 CPU 切换和退出清理（已复现）

src/syscall/handler.rs:198 直接调用 pm.schedule_step(1)，丢弃返回的调度事件与切换结果，没有经过 Kernel::step 的 CPU 切换及资源清理逻辑。

- 子进程设置独立 RIP 为 0x9000，yield 选中子进程后 CPU 仍保留父进程 RIP 0x1028。
- init 的剩余 burst 为 1 时，通过 yield 自然完成，仍占用 2 个物理页，预期退出清理后为 0。

建议由 Kernel 统一应用调度结果，将推进时间、选择进程、加载 CPU、处理退出事件分开。yield 只提出让出请求，其行为不应顺带消费一个未协调的运行 tick。fork、显式 exit、自然完成、Shell kill 应复用同一套生命周期操作。

### 2. 唤醒没有匹配等待原因（已复现）

父进程 sleep(100)，子进程运行一个 tick 后退出，SYS_EXIT 调用 pm.wake(ppid)，父进程从 Blocked(Sleeping) 提前变成 Ready。Kernel 的其他退出路径也无条件唤醒父进程；定时器只返回 PID，wake 接受任何阻塞原因。

建议等待状态包含对象和事件身份，例如 Sleeping { token, deadline }、WaitingChild { pid }、WaitingPipe { id, operation }。退出只能唤醒相应的 waitpid 等待，睡眠事件只能解除匹配的 sleep；退出时取消或作废等待记录。覆盖不同子进程退出、旧定时事件、重复睡眠和睡眠期间退出。

### 3. 新增页清零实现引入裸指针生命周期风险（静态检查）

src/mm/buddy.rs:83 的公开安全方法 set_ram_buffer 接受任意裸指针及长度，allocate_pages 在第 132 行通过 unsafe 写入。src/mm/mod.rs:19 又公开可替换和扩容的 ram Vec；RAM 被重新分配或丢弃后，Buddy 保存的指针不会自动更新。长度校验无法证明地址仍然有效。还可以独立移出 Buddy，使其存活时间超过 RAM。

没有执行非法指针写入测试。建议 Buddy 只管理页号，将 RAM 清零交给 MemoryManager 的受控分配接口，通过临时切片借用访问内存；隐藏 RAM 和 Buddy 的可变内部状态。只将 setter 改成 unsafe，仍不能解决公开 RAM 被替换后的正常使用路径。

Rust 的安全抽象需要保证安全调用不能引发未定义行为，并通过模块边界维护 unsafe 所依赖的状态约束，参见 [Rustonomicon](https://doc.rust-lang.org/nomicon/working-with-unsafe.html)。这里的具体风险来自本项目代码静态分析。

## 推荐拓展顺序

| 阶段 | 具体内容 | 完成标准 |
| --- | --- | --- |
| 0：巩固基础 | 修复上述问题；统一 Kernel 的 fork/exit/yield/sleep；KernelConfig + try_new；接入已有 19 个验证和修复后的新验证；添加 CI | 常规 cargo test 覆盖关键生命周期；失败初始化无残留；各入口资源守恒且 CPU/PCB 一致 |
| 1：完成 IPC 与设备 FD | 将 FileDescriptorEntry 扩展为文件、管道读端、管道写端、控制台等类型；pipe 创建一对进程 FD；read/write 统一分发；处理等待、EOF、读端关闭和继承引用 | 父子进程通过 pipe 传送数据；缓冲满/空时阻塞并能恢复；最后写端关闭后读取 EOF；进程退出后无管道引用残留 |
| 2：执行用户工作负载 | 先定义简单的模拟指令或任务脚本，包含计算、syscall、sleep、branch、exit；增加 exec 装载并替换地址空间；处理父子 fork 返回值；分离结构化 step_once 与日志格式化 | 两个程序通过系统调用运行、通信并正常退出，Shell 无须直接修改 PCB；固定输入得到可复现事件序列 |
| 3：让文件系统重启后可恢复 | 引入小型 BlockDevice 接口及文件镜像实现；定义超级块、空闲位图、inode 和目录的磁盘编码及挂载流程；注入 I/O 失败 | 写入、sync、关闭、重新挂载后路径和内容仍在；写回失败保留脏数据；空闲块与引用保持一致 |
| 4：按研究目标深化 | 内存方向：mmap/munmap、缺页、页引用计数，再做 COW fork；调度方向：可选策略与混合负载比较；观测方向：结构化 trace、JSON/CSV、回放与可视化 | COW 首次写入才复制且父子隔离；调度报告同时包含吞吐、公平性、等待和唤醒延迟；trace 有界或流式输出 |

阶段 1 是最推荐的首个功能增量：当前 SYS_PIPE 仅返回 pm.pipes 中的编号，SYS_READ/SYS_WRITE 则只解析 VFS FD，两个路径尚未接通。标准 FD 0/1/2 也只有 PCB 占位，VFS 没有对应的打开对象。把这些接起来可以同时检验 FD 所有权、阻塞唤醒和进程退出，范围比网络栈或多核更容易控制。

阶段 2 应先做简单、可测试的模拟程序执行器。当前 VMA 列表与实际映射不是完整契约，栈也未建立真实可访问映射；直接接入 ELF 会同时引入大量装载和执行语义。若目标是可启动的裸机内核，则应单列路线：no_std、引导、硬件异常与中断、真实页表、设备驱动。它需要重做 arch 和部分运行时，并非现有宿主机模拟器增加几个接口即可完成。

阶段 3 不能只把 dev.blocks 写入文件：目前 inode、目录和 free_blocks 都在宿主机 HashMap/Vec 中。只有数据块镜像并不能重建文件系统。日志或崩溃恢复应在磁盘元数据格式与错误语义稳定后加入。

## 工程与性能结论

- 当前目录未发现 CI 工作流。建议首先添加构建、测试、Clippy，格式检查先解决当前执行环境权限问题。
- SYS_STAT 已有分发，当前只返回文件 size，可在明确用户 ABI 后扩展为可复制到用户缓冲区的结构化结果。
- Kernel::step 为每个 tick 创建 String 并累计 Vec。长期仿真宜增加结构化 step_once 和有界事件收集，Shell/Demo 再格式化。
- 本次基准：缓存模型周期收益仍为 4.71x，但宿主机实际耗时为直接访问约 598µs、缓存约 1.37ms。模型收益和宿主机性能应分别展示。
- sched_bench 仍用 CFS pick_next_task 的计数宣称“真实切换”，并用整个调度循环耗时除以该计数；它没有调用 VirtualCpu，不能用该结果代表 CPU 模拟切换耗时。
- README 的吞吐是一次历史运行数据。本轮 Buddy 约 748 万 ops/s、Slab 约 1129 万 ops/s、调度约 221 ns/step；这些变化不足以证明回归。增加预热、多轮统计和容量/负载矩阵，再决定是否优化 LRU 或替换调度容器。
- Slab 空页归还现在会重建整个缓存的索引，已修复页重复所有权，但该分支随 Slab 数量线性增长，文档中的统一 O(1) 表述需收窄。

## 追加诊断重跑

源码：system-extension-probes.rs。输出：system-extension-probes-output.txt。三个测试按预期约束编写，当前失败是检查证据；修复后再整合进 tests。

```powershell
cargo build --lib --offline
New-Item -ItemType Directory -Path target/architecture-review -Force | Out-Null
$extensionLibrary = (Get-ChildItem target/debug/deps -Filter 'libmini_os_kernel*.rlib' |
    Sort-Object LastWriteTime -Descending | Select-Object -First 1).FullName
rustc --edition=2024 --test docs/reviews/system-extension-probes.rs `
    --extern "mini_os_kernel=$extensionLibrary" -L dependency=target/debug/deps `
    -o target/architecture-review/extension-probes.exe
if ($LASTEXITCODE -eq 0) {
    & ./target/architecture-review/extension-probes.exe --test-threads=1 --color never
}
```
