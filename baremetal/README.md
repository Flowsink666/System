# Mini-OS 原生内核

这是一个可启动的 x86_64 小内核，不是宿主机窗口程序或 Web UI。UEFI 将 `BOOTX64.EFI` 装载到虚拟机内存，内核获取显存和内存描述后调用 `ExitBootServices`，随后自己处理中断、输入、物理页和界面绘制。

## 构建与启动

需要 Rust stable、Node.js（仅用于生成磁盘镜像）、QEMU 和 x86_64 UEFI 固件。内核本身不使用 Node.js、Windows、浏览器、Rust 标准库或第三方 crate。

Windows 可以直接双击项目根目录的 `Run-MiniOS.cmd`，自动构建并打开 QEMU 窗口。

```powershell
./baremetal/build.ps1
./baremetal/run-qemu.ps1 -NoBuild
```

构建输出：

- `target/baremetal/x86_64-unknown-uefi/release/mini_os_native.efi`：PE/COFF EFI 启动程序。
- `target/baremetal/mini-os.img`：64MiB MBR 磁盘镜像，含 FAT32 EFI System Partition 和 `/EFI/BOOT/BOOTX64.EFI`。
- `target/baremetal/serial.log`：QEMU COM1 启动与硬件事件日志。

脚本自动寻找 PATH、`C:/Program Files/qemu` 或项目下 `target/tools/qemu` 的 QEMU，并在其目录查找 `edk2-x86_64-code.fd`。也可以显式指定：

```powershell
./baremetal/run-qemu.ps1 -QemuPath 'C:/path/qemu-system-x86_64.exe' -FirmwarePath 'C:/path/OVMF_CODE.fd' -VarsPath 'C:/path/OVMF_VARS.fd'
```

默认使用 Q35、256MiB 内存、单 CPU、TCG、标准 VGA、PS/2 键鼠和串口，不创建网络设备。不需要 Windows 虚拟化特性或管理员权限才能运行 QEMU。
脚本使用两块 pflash 加载固件代码和变量存储；变量模板复制到 `target/baremetal/uefi-vars.fd`，不会修改 QEMU 安装目录的模板。自动识别 QEMU 自带的 `edk2-i386-vars.fd` 或相邻的 OVMF VARS 文件。

## 操作

点击侧栏，或使用功能键：

| 功能键 | 应用 | 操作 |
| --- | --- | --- |
| F1 | Overview | 原生内核和硬件状态 |
| F2 | Memory | A 分配一个 4KiB 物理页；F 释放最近分配的页；也可点击按钮 |
| F3 | Tasks | N 新建协作式内核任务；K 移除最后一个任务；Space 暂停/恢复任务执行 |
| F4 | Terminal | 输入命令并按 Enter；Backspace 编辑；Esc 回到总览 |
| F5 | Files | 查看 RAM 中的 readme.txt、note.txt |

终端命令：

```text
help
mem
alloc
free
spawn
tasks
kill
ls
cat readme.txt
write Hello from my operating system
cat note.txt
clear
```

当前字体和键盘输入支持 ASCII。文件是内核 RAM 文件，重启后清空。鼠标按 QEMU 默认 PS/2 三字节协议处理；没有设置 USB tablet。

## 当前内核边界

- 在 x86_64 Ring 0 中执行，建立自己的 IDT，重新配置 8259 PIC，使用 PIT IRQ0 提供约 100Hz 时钟。
- PS/2 IRQ1、IRQ12 将输入写入有界队列，界面与应用在中断之外运行。鼠标先组装完整三字节包，再按整包入队；队列满时丢弃整包并记录计数，防止丢字节造成持续错位。键盘溢出后清理残留队列并重置修饰键／扩展前缀状态，避免丢失释放事件后 Shift 持续生效。
- 显示优先使用 1024×768；固件未提供时，保留兼容的当前模式或选择最小的兼容模式（至少 800×600、32 位 RGB/BGR）。
- 鼠标移动只保存、恢复光标覆盖的 9×17 像素区域。输入合并后最多每 30ms 呈现一次，内容变化和每 250ms 的统计更新才重绘整个桌面。
- 物理页来自固件内存图的 ConventionalMemory，分配后实际清零；不重新分配固件和启动映像所在页。
- 保留 UEFI 建立的初始恒等映射及代码段；尚未替换为自有页表、GDT/TSS 或实现 Ring 3。
- Tasks 执行内核内置工作函数，采用协作调度；尚未实现用户 ELF、真实进程地址空间或抢占式任务切换。
- 原 `src/mm`、`src/sched`、`src/fs` 仍是参考模拟器，未将其宿主机元数据和性能数字冒充硬件实现。后续应先建立自有页表和堆，再移植这些算法及真实磁盘驱动。

## 验证

宿主机上验证不依赖硬件的内核组件：

```powershell
cargo test --manifest-path baremetal/Cargo.toml --lib --target-dir target/native-tests --offline
node --test baremetal/scripts/test-serial-markers.js
```

硬件路径必须在 QEMU 中验证。串口需要先出现 `ExitBootServices succeeded`、`native desktop ready`，随后持续出现 `TICK`；键鼠操作需出现相应物理页、命令和鼠标事件。
测试客户端在每次操作前记录日志位置，只匹配该操作之后的新记录，避免旧状态让回归检查假通过。

可以重跑实际虚拟机的交互测试：先在一个终端运行 QEMU，再在另一个终端运行测试客户端。

```powershell
./baremetal/run-qemu.ps1 -NoBuild -Headless -QmpPort 4444
# 另一个终端
node ./baremetal/scripts/smoke-qemu.js
```

2026-10-07 已在 Windows 上用 QEMU 11.1.0、Q35/TCG、256MiB、EDK2 验证通过：UEFI 退出、持续 PIT 中断、功能键、键盘与鼠标分配/释放实际物理页、协作任务增删、终端写入并读取 RAM 文件。测试将真实显存截图保存到 `target/baremetal/{desktop,terminal,files}.ppm`。独立组件的 4 项单元测试和 UEFI 目标严格 Clippy 检查也已通过。

2026-10-08 优化后重新通过 11 项原生组件测试、2 项日志断言回归、UEFI release 构建及严格 Clippy；无窗口 QEMU 的完整键鼠、任务增删、物理页和 RAM 文件 smoke 通过，三张显存截图已检查。新增组件测试覆盖队列溢出后的包边界、键盘溢出状态重置、双 Shift 状态、光标重叠移动和边缘背景恢复，以及兼容显示模式选择。

参考：[Rust 的 UEFI 目标](https://doc.rust-lang.org/rustc/platform-support/unknown-uefi.html)、[UEFI Boot Services 与退出规范](https://uefi.org/specs/UEFI/2.10/07_Services_Boot_Services.html)、[QEMU x86 系统仿真](https://www.qemu.org/docs/master/system/target-i386.html)。
