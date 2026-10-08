# 当前项目优化检查（2026-10-08）

当前项目已经有完整的机制测试和可构建的原生内核。最值得优先处理的是共享页生命周期、磁盘布局与失败操作的回滚；这些问题会造成数据损坏、任务阻塞或宿主程序崩溃。性能优化应在这些约束修复后开展。

检查对象是当前工作目录，包含已有未提交修改。检查没有修改业务源码、已有测试或格式；仅新增本报告和独立复现材料。旧报告中的已修复问题没有直接作为当前缺陷引用。

## 已完成的验证

| 检查 | 结果 |
| --- | --- |
| 宿主机 `cargo test --all-targets --offline` | 78 项集成测试全部通过 |
| 原生内核库测试 | 4 项全部通过 |
| 两个项目的严格 Clippy | 宿主机全部目标、原生库与 UEFI 目标通过 |
| UEFI release 构建 | 通过 |
| 格式检查 | 原生项目通过；宿主机项目有格式差异 |
| Release 基准 | 完成首次运行，并追加 7 轮相同负载比较 |
| 独立边界验证 | 内存/执行的 8 个约束测试、Shell 的 2 个约束测试失败；另外复现 5 个文件/IPC 异常观察和 2 个原生输入/启动问题 |

格式与原生 Clippy 首次受到工具配置读取权限限制，经自动审批后完成检查。没有执行格式化修改。本次没有启动 QEMU；原生硬件路径的运行性能与实际固件兼容性仍需后续验证。独立验证失败是本次发现的证据，不等于原有测试失败。

## P1：优先修复的数据与执行问题

### 1. COW 共享页被提前回收，破坏父子隔离

位置：[munmap](C:/Users/Flowsink/Documents/GitHub/System/src/kernel.rs:429)、[程序重装](C:/Users/Flowsink/Documents/GitHub/System/src/kernel.rs:444)、[直接回收页](C:/Users/Flowsink/Documents/GitHub/System/src/mm/page_table.rs:277)。

`munmap` 和程序重装仍走直接归还 Buddy 的旧路径，绕过新增的物理页共享引用计数。探针中，子进程解除共享映射后重新分配并写入 `NEW!`，父进程的 `OLD!` 同步变为 `NEW!`；子进程重装程序还会将父进程的 `SECRET` 清零。

建议：所有映射替换、解除映射和地址空间销毁统一通过 MemoryManager 释放引用，仅最后一个引用消失时归还页。收拢旧的直接释放接口，保护“共享页仍有引用时不得复用”的约束。[内存验证输出](C:/Users/Flowsink/Documents/GitHub/System/docs/reviews/2026-10-08-probes/memory-output.txt)

### 2. 文件数据会被磁盘元数据覆盖

位置：[VFS 初始化](C:/Users/Flowsink/Documents/GitHub/System/src/fs/vfs.rs:69)、[磁盘布局](C:/Users/Flowsink/Documents/GitHub/System/src/fs/disk.rs:28)、[写入 inode 表](C:/Users/Flowsink/Documents/GitHub/System/src/fs/disk.rs:445)。

VFS 新建时只保留块 0，持久化格式实际占用块 0～34。64 块磁盘上，先提交根目录，再创建 5 个各 3072B 的文件，最后一个文件分配到块 34、33。提交返回成功，重挂载后该文件的 3072B 中只有 2048B 保持原内容。

建议：统一新建与挂载时的磁盘布局，分配器始终排除全部元数据块；创建入口同时校验磁盘容量。增加逼近满盘时的数据守恒测试。[文件验证输出](C:/Users/Flowsink/Documents/GitHub/System/docs/reviews/2026-10-08-probes/filesystem-output.txt)

### 3. 目录持久化静默截断

位置：[目录序列化](C:/Users/Flowsink/Documents/GitHub/System/src/fs/disk.rs:387)。

提交只写第一个 512B 目录块，写满 8 个条目后直接停止。由于 `.`、`..` 也占条目，创建 7 个根目录文件即可达到 9 项。探针中提交成功，重挂载仅剩 8 项；HashMap 顺序使丢失的条目不确定。

建议：根据条目数分配并写入多个目录块，同时扩展挂载读取路径；若暂时只支持单块目录，应在超限创建时明确报错。任何成功提交都不能静默丢条目。

### 4. 程序装载缺少回滚，代码区与数据区还会重叠

位置：[销毁旧空间](C:/Users/Flowsink/Documents/GitHub/System/src/kernel.rs:443)、[分配代码页](C:/Users/Flowsink/Documents/GitHub/System/src/kernel.rs:453)、[固定数据页](C:/Users/Flowsink/Documents/GitHub/System/src/kernel.rs:462)。

装载先销毁旧地址空间，再逐页分配新程序。4 页物理内存下装载 5 页代码会返回失败，但空闲页从 2 变为 0，旧程序已经丢失，新程序也未完成。另一个 32KiB 代码探针成功装载后，第八页被固定映射在 `0x8000` 的数据页覆盖并清零；现有代码布局最多容纳 28KiB 而不重叠。

建议：先验证各段范围，在临时地址空间中完成装载，失败完整回收，成功后再交换；数据与栈地址应根据布局安排，或者明确限制代码大小。提交时同步更新 VMA 与 CPU 状态。

### 5. 系统调用返回值污染刚切换到的任务

位置：[字节码 syscall 返回](C:/Users/Flowsink/Documents/GitHub/System/src/kernel.rs:687)。

`self.syscall` 可以切换 CPU，随后代码仍把返回值写进当前 CPU 的 RAX。父进程执行 `SYS_YIELD`，子进程原 RAX 为 123，切换后被父进程返回的 0 覆盖。

建议：返回值归属于调用者的上下文；先更新调用者 PCB，再根据当前运行者同步 CPU。覆盖 yield、sleep 和阻塞系统调用的恢复语义。

### 6. 最后一个运行 tick 内退出会触发重复清理

位置：[提交调度结果](C:/Users/Flowsink/Documents/GitHub/System/src/kernel.rs:530)、[重复退出的 expect](C:/Users/Flowsink/Documents/GitHub/System/src/kernel.rs:534)、[Halt 退出](C:/Users/Flowsink/Documents/GitHub/System/src/kernel.rs:705)。

调度器产生 burst 结束事件后仍执行最后一个 tick；该 tick 内的 Halt 已经移除 init，随后旧调度事件又要求退出一次。init 的 burst 为 1、程序只有 Halt 时，`step(1)` 会使宿主程序 panic。

建议：执行与调度统一提交一次退出结果，完成指令后重新核对任务状态，使资源清理只发生一次。

### 7. 管道读取报错却已消费数据，并可能留下阻塞写者

位置：[管道读取](C:/Users/Flowsink/Documents/GitHub/System/src/syscall/handler.rs:154)、[标准输入读取](C:/Users/Flowsink/Documents/GitHub/System/src/syscall/handler.rs:184)。

读取先移除数据，再复制到用户地址。坏地址读满 8B 的管道会返回错误，但缓冲区已经清空，等待空位的写者仍保持阻塞。标准输入同样出现 5B 数据丢失。

建议：采用“预览 → 成功复制 → 消费并唤醒”的提交顺序，或先完成完整输出范围的可写验证；明确跨页访问失败时的处理方式。已有普通文件偏移回滚测试应扩展到 Pipe/Stdin。

### 8. 热加载镜像使旧进程 FD 指向另一份文件

位置：[Shell load](C:/Users/Flowsink/Documents/GitHub/System/src/shell.rs:426)、[挂载后 FD 重新从 10 分配](C:/Users/Flowsink/Documents/GitHub/System/src/fs/disk.rs:353)。

Shell 只替换 VFS，现存 PCB 的描述符没有更新。新 VFS 复用编号后，旧 FD 可读取无关文件。探针中旧全局 FD 13 在加载镜像后读取到了新镜像的 `replacement`。

建议：将加载镜像做成 Kernel 的完整状态切换操作，明确现有任务的处理方式；至少在存在文件引用时拒绝更换，或通过实例身份使旧句柄稳定报错。文件映射引用也要参与该约束。[Shell 验证输出](C:/Users/Flowsink/Documents/GitHub/System/docs/reviews/2026-10-08-probes/shell-output.txt)

## P2：补齐边界与原生路径

| 问题 | 当前证据与建议 |
| --- | --- |
| 只读文件 mmap 得到零内容 | [kernel.rs:407](C:/Users/Flowsink/Documents/GitHub/System/src/kernel.rs:407) 对只读页执行受权限检查的写入，再忽略失败。映射 `SOURCE` 后读到全零。初始化页内容应通过受控装载路径完成，再设置最终权限；文件读取错误应传播，文件游标也应保持。 |
| 普通 fork 丢失尚未访问的按需页 | [page_table.rs:303](C:/Users/Flowsink/Documents/GitHub/System/src/mm/page_table.rs:303) 只复制 Present 页，未继承 Demand 项。父进程 mmap 后尚未触页，普通 fork 的子进程首次写入报未映射。深拷贝也必须继承按需映射及权限。 |
| SYS_PIPE 失败后残留资源 | [handler.rs:294](C:/Users/Flowsink/Documents/GitHub/System/src/syscall/handler.rs:294) 在返回用户 FD 失败前已经发布管道。坏地址调用后，管道数 1→2、FD 数 5→7。应预检返回缓冲区，并在失败路径撤销新资源。 |
| Shell exec 失败留下可运行进程 | [shell.rs:444](C:/Users/Flowsink/Documents/GitHub/System/src/shell.rs:444) 先 spawn，失败后没有撤销。执行不存在的程序后，任务数 1→2。创建与装载应作为一次可回滚操作。 |
| 鼠标队列丢字节后持续解码错位 | [interrupts.rs:41](C:/Users/Flowsink/Documents/GitHub/System/baremetal/src/interrupts.rs:41) 队列满时丢字节，[input.rs:120](C:/Users/Flowsink/Documents/GitHub/System/baremetal/src/input.rs:120) 不知发生过丢失。孤立解码验证中，丢一个包头后输入 30 个仅向右包，坐标从 (668,380) 变成 (668,140)。建议完整包入队/整包丢弃，记录溢出并恢复同步。 |
| UEFI 缺少图形模式回退 | [uefi.rs:114](C:/Users/Flowsink/Documents/GitHub/System/baremetal/src/uefi.rs:114) 只寻找 1024×768，否则沿用当前模式。mock 固件默认 640×480、可用 800×600 时仍启动失败。建议优先首选分辨率，再选择满足现有尺寸与像素格式约束的可用模式。 |
| QEMU 冒烟测试可能误判删除成功 | [smoke-qemu.js:13](C:/Users/Flowsink/Documents/GitHub/System/baremetal/scripts/smoke-qemu.js:13) 搜索整个历史日志；[删除断言](C:/Users/Flowsink/Documents/GitHub/System/baremetal/scripts/smoke-qemu.js:92) 的 tasks=2 标记在 F3 阶段已出现。删除失效仍可满足等待。建议每次动作前记录日志偏移，只验证动作之后的新记录。此项来自控制流分析。 |

## 性能优化与工程维护

1. **长期运行的日志开销。** [Kernel::step](C:/Users/Flowsink/Documents/GitHub/System/src/kernel.rs:495) 每 tick 格式化并累计 String；[Shell run](C:/Users/Flowsink/Documents/GitHub/System/src/shell.rs:226) 最终只显示首尾 10 条，但此前已经创建全部日志。新增返回结构化事件的单步入口，让日志显示端采用有界队列或流式输出，长时间模拟不再随 tick 数保留全部文本。

2. **原生桌面的全屏重绘。** [main.rs:80](C:/Users/Flowsink/Documents/GitHub/System/baremetal/src/main.rs:80) 任一鼠标包触发 dirty，[desktop.rs:217](C:/Users/Flowsink/Documents/GitHub/System/baremetal/src/desktop.rs:217) 重画全屏，[graphics.rs:35](C:/Users/Flowsink/Documents/GitHub/System/baremetal/src/graphics.rs:35) 逐像素写显存。1024×768 下仅清屏就写 3MiB/帧。优先保存静态背景，光标只更新旧/新区域，界面变化采用局部重绘；合并输入并限制刷新频率。此项为静态热点判断，没有实测 QEMU 帧率。

3. **缓存维护成本。** [BufferCache::touch_lru](C:/Users/Flowsink/Documents/GitHub/System/src/fs/buffer_cache.rs:121) 非 MRU 命中需要线性查找和中间删除；[TLB lookup](C:/Users/Flowsink/Documents/GitHub/System/src/mm/tlb.rs:49) 也线性扫描。追加 7 轮现有负载中，带缓存的宿主机耗时为直接访问的约 1.77～2.79 倍，固定 I/O 周期模型仍给出 4.71 倍收益。这说明当前内存设备上的缓存簿记成本明显；不能据此推断真实磁盘也会变慢。先比较不同容量与负载，再为块缓存评估稳定节点索引的 LRU；小容量 TLB 保留连续数组也可能更合适。

4. **基准口径与质量检查。** [sched_bench.rs:68](C:/Users/Flowsink/Documents/GitHub/System/src/benchmark/sched_bench.rs:68) 用任务提取计数和整段调度循环耗时计算“上下文切换延迟”，该负载没有经过 VirtualCpu。应改称调度计数/循环耗时，并另测 CPU 上下文加载；增加预热、多轮统计、正确性校验及容量矩阵。README 的历史数字只应作为特定环境的样例。项目未发现 CI 工作流，建议分别检查宿主机与原生项目，包含测试、Clippy、格式与 UEFI 构建；先把本次关键复现整理成正式回归测试。

建议实施顺序：先修 COW 所有权与磁盘保留区/目录序列化，再修装载事务、系统调用上下文和退出时序，随后补齐 IPC、mmap、镜像加载及原生输入，最后依据重复测量优化日志和绘制。

## 复现材料

材料保存在 [2026-10-08-probes](C:/Users/Flowsink/Documents/GitHub/System/docs/reviews/2026-10-08-probes)。`memory.rs` 和 `shell.rs` 是按正确行为编写的约束测试，当前失败用于记录缺陷；`filesystem.rs` 输出实际与预期的对照。原生输入探针引用当前源码，UEFI mock 的 `uefi_probe_module.rs` 保留本次源码快照及追加测试，后续修改后应刷新快照。

在项目根目录构建库后，可独立运行，不影响默认测试：

```powershell
cargo build --lib --offline
New-Item -ItemType Directory -Path target/review-20261008 -Force | Out-Null
$reviewLibrary = (Get-ChildItem target/debug/deps -Filter 'libmini_os_kernel*.rlib' |
    Sort-Object LastWriteTime -Descending | Select-Object -First 1).FullName
rustc --edition=2024 --test docs/reviews/2026-10-08-probes/memory.rs `
    --extern "mini_os_kernel=$reviewLibrary" -L dependency=target/debug/deps `
    -o target/review-20261008/memory.exe
if ($LASTEXITCODE -eq 0) { & ./target/review-20261008/memory.exe --test-threads=1 }
```

将 `memory.rs` 换成 `shell.rs` 可复现 Shell 问题；`filesystem.rs` 有 main，编译时去掉 `--test`。原生探针分别用 `rustc --edition=2024 docs/reviews/2026-10-08-probes/native-input.rs` 和 `rustc --edition=2024 --test docs/reviews/2026-10-08-probes/native-uefi.rs` 编译，建议同样通过 `-o` 将输出放在 target。UEFI mock 的测试返回通过代表成功观察到当前拒绝模式的行为，不代表兼容性已经修复。
