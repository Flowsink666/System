//! 系统调用分发与特权级边界检查 (Syscall Dispatch & Privilege Boundary)
//!
//! 高可靠特性：
//! 1. 用户指针与缓冲区经由受检查的地址空间转换 (read_virtual_checked / write_virtual_checked)
//! 2. 基于进程专属描述符表 (PCB fd_table) 解析文件句柄，严防句柄越权
//! 3. 接入 waitpid、sleep、fork 及 IPC 管道完整流程

use super::types::*;
use crate::kernel::Kernel;
use crate::sched::FileDescriptorEntry;

pub struct SyscallDispatcher;

impl SyscallDispatcher {
    /// 执行系统调用处理
    pub(crate) fn dispatch(args: SyscallArgs, kernel: &mut Kernel) -> Result<isize, &'static str> {
        let curr_pid = kernel.pm.current_pid.ok_or("No current process context")?;
        match args.num {
            SYS_EXIT => {
                kernel.terminate_process(curr_pid, args.arg0 as i32)?;
                return Ok(0);
            }
            SYS_FORK => return kernel.fork_process(curr_pid).map(|pid| pid as isize),
            SYS_YIELD => {
                kernel.yield_current()?;
                return Ok(0);
            }
            SYS_SLEEP => {
                kernel.sleep_current(args.arg0 as u64)?;
                return Ok(0);
            }
            _ => {}
        }

        let Kernel { mm, pm, vfs, .. } = kernel;
        match args.num {
            SYS_GETPID => Ok(curr_pid as isize),

            SYS_WAITPID => {
                let child_pid = args.arg0;
                match pm.waitpid(curr_pid, child_pid)? {
                    Some(exit_code) => Ok(exit_code as isize),
                    None => Ok(-1), // 子进程仍在运行，父进程已转入阻塞
                }
            }

            SYS_KMALLOC => {
                let size = args.arg0;
                let handle = mm
                    .slab
                    .kmalloc(&mut mm.buddy, size)
                    .ok_or("Failed to allocate slab memory")?;
                Ok(handle as isize)
            }

            SYS_KFREE => {
                let handle = args.arg0 as u64;
                mm.slab.kfree(&mut mm.buddy, handle)?;
                Ok(0)
            }

            SYS_OPEN => {
                // arg0: 用户态路径字符串虚拟地址, arg1: 字符串长度, arg2: 打开标志
                let path_va = args.arg0;
                let path_len = args.arg1;
                let flags = args.arg2 as u32;

                let path_str = if path_va != 0 && path_len > 0 {
                    if path_len > 256 {
                        return Err("Path too long (exceeds 256 bytes)");
                    }
                    let mut path_buf = vec![0u8; path_len];
                    let proc = pm.processes.get_mut(&curr_pid).unwrap();
                    mm.read_virtual_checked(&mut proc.address_space, path_va, &mut path_buf, true)?;
                    String::from_utf8(path_buf).map_err(|_| "Invalid UTF-8 in path")?
                } else {
                    "/tmp/default.txt".to_string()
                };

                let vfs_fd = vfs.open(&path_str, flags)?;

                // 挂载到当前进程专属的 fd_table
                let proc = pm.processes.get_mut(&curr_pid).unwrap();
                let user_fd = proc.alloc_fd(FileDescriptorEntry { vfs_fd, flags });
                Ok(user_fd as isize)
            }

            SYS_READ => {
                // arg0: user_fd, arg1: 用户缓冲区虚拟地址, arg2: 读取长度
                let user_fd = args.arg0;
                let buf_va = args.arg1;
                let len = args.arg2;

                if len == 0 {
                    return Ok(0);
                }

                if buf_va == 0 {
                    return Err("Bad buffer address: NULL pointer");
                }

                let safe_len = len.min(70 * 1024);

                let vfs_fd = {
                    let proc = pm.processes.get(&curr_pid).unwrap();
                    let fd_entry = proc.fd_table.get(&user_fd).ok_or("Bad file descriptor")?;
                    fd_entry.vfs_fd
                };

                let old_offset = vfs
                    .open_files
                    .get(&vfs_fd)
                    .ok_or("Invalid file descriptor")?
                    .offset;

                let mut kbuf = vec![0u8; safe_len];
                let n = vfs.read(vfs_fd, &mut kbuf)?;

                if n > 0 {
                    let proc = pm.processes.get_mut(&curr_pid).unwrap();
                    if let Err(e) =
                        mm.write_virtual_checked(&mut proc.address_space, buf_va, &kbuf[..n], true)
                    {
                        let _ = vfs.seek(vfs_fd, old_offset);
                        return Err(e);
                    }
                }

                Ok(n as isize)
            }

            SYS_WRITE => {
                // arg0: user_fd, arg1: 用户数据虚拟地址, arg2: 写入长度
                let user_fd = args.arg0;
                let buf_va = args.arg1;
                let len = args.arg2;

                if len == 0 {
                    return Ok(0);
                }

                if buf_va == 0 {
                    return Err("Bad buffer address: NULL pointer");
                }

                let safe_len = len.min(70 * 1024);

                let vfs_fd = {
                    let proc = pm.processes.get(&curr_pid).unwrap();
                    let fd_entry = proc.fd_table.get(&user_fd).ok_or("Bad file descriptor")?;
                    fd_entry.vfs_fd
                };

                let mut tmp = vec![0u8; safe_len];
                let proc = pm.processes.get_mut(&curr_pid).unwrap();
                mm.read_virtual_checked(&mut proc.address_space, buf_va, &mut tmp, true)?;

                let n = vfs.write(vfs_fd, &tmp)?;
                Ok(n as isize)
            }

            SYS_CLOSE => {
                let user_fd = args.arg0;
                let proc = pm.processes.get_mut(&curr_pid).unwrap();
                let fd_entry = proc.close_fd(user_fd).ok_or("Bad file descriptor")?;
                vfs.close(fd_entry.vfs_fd)?;
                Ok(0)
            }

            SYS_PIPE => {
                let capacity = if args.arg0 > 0 { args.arg0 } else { 4096 };
                let pipe_id = pm.create_pipe(capacity);
                Ok(pipe_id as isize)
            }

            SYS_MKDIR => {
                let path_va = args.arg0;
                let path_len = args.arg1;
                if path_va == 0 || path_len == 0 {
                    return Err("Invalid path argument for mkdir");
                }
                if path_len > 256 {
                    return Err("Path too long (exceeds 256 bytes)");
                }
                let mut path_buf = vec![0u8; path_len];
                let proc = pm.processes.get_mut(&curr_pid).unwrap();
                mm.read_virtual_checked(&mut proc.address_space, path_va, &mut path_buf, true)?;
                let path_str = String::from_utf8(path_buf).map_err(|_| "Invalid UTF-8 in path")?;
                let inode_id = vfs.mkdir(&path_str)?;
                Ok(inode_id as isize)
            }

            SYS_STAT => {
                let path_va = args.arg0;
                let path_len = args.arg1;
                if path_va == 0 || path_len == 0 {
                    return Err("Invalid path argument for stat");
                }
                if path_len > 256 {
                    return Err("Path too long (exceeds 256 bytes)");
                }
                let mut path_buf = vec![0u8; path_len];
                let proc = pm.processes.get_mut(&curr_pid).unwrap();
                mm.read_virtual_checked(&mut proc.address_space, path_va, &mut path_buf, true)?;
                let path_str = String::from_utf8(path_buf).map_err(|_| "Invalid UTF-8 in path")?;
                let stat = vfs.stat(&path_str)?;
                Ok(stat.size as isize)
            }

            _ => Err("Invalid or unimplemented syscall number"),
        }
    }
}
