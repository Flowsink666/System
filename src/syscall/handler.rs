//! 系统调用分发与特权级边界检查 (Syscall Dispatch & Privilege Boundary)
//!
//! 高可靠特性：
//! 1. 用户指针与缓冲区经由受检查的地址空间转换 (read_virtual_checked / write_virtual_checked)
//! 2. 基于进程专属描述符表 (PCB fd_table) 解析文件句柄，严防句柄越权
//! 3. 接入 waitpid、sleep、fork 及 IPC 管道完整流程

use super::types::*;
use crate::kernel::Kernel;
use crate::sched::{BlockedReason, FileDescriptorEntry, FileDescriptorType};

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
            SYS_FORK => {
                let pid = if args.arg0 == 1 {
                    kernel.fork_process_cow(curr_pid)?
                } else {
                    kernel.fork_process(curr_pid)?
                };
                return Ok(pid as isize);
            }
            SYS_FORK_COW => return kernel.fork_process_cow(curr_pid).map(|pid| pid as isize),
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

        match args.num {
            SYS_GETPID => Ok(curr_pid as isize),

            SYS_WAITPID => {
                let child_pid = args.arg0;
                match kernel.pm.waitpid(curr_pid, child_pid)? {
                    Some(exit_code) => Ok(exit_code as isize),
                    None => Ok(-1), // 子进程仍在运行，父进程已转入阻塞
                }
            }

            SYS_KMALLOC => {
                let size = args.arg0;
                let handle = kernel
                    .mm
                    .slab
                    .kmalloc(&mut kernel.mm.buddy, size)
                    .ok_or("Failed to allocate slab memory")?;
                Ok(handle as isize)
            }

            SYS_KFREE => {
                let handle = args.arg0 as u64;
                kernel.mm.slab.kfree(&mut kernel.mm.buddy, handle)?;
                Ok(0)
            }

            SYS_OPEN => {
                // arg0: 用户态路径字符串虚拟地址, arg1: 字符串长度, arg2: 打开标志
                let path_va = args.arg0;
                let path_len = args.arg1;
                let supported_flags = (crate::fs::O_WRONLY
                    | crate::fs::O_RDWR
                    | crate::fs::O_CREAT
                    | crate::fs::O_TRUNC
                    | crate::fs::O_APPEND) as usize;
                if args.arg2 & !supported_flags != 0 {
                    return Err("Unsupported open flags");
                }
                let flags = args.arg2 as u32;

                let path_str = if path_va != 0 && path_len > 0 {
                    if path_len > 256 {
                        return Err("Path too long (exceeds 256 bytes)");
                    }
                    let mut path_buf = vec![0u8; path_len];
                    let proc = kernel.pm.processes.get_mut(&curr_pid).unwrap();
                    kernel.mm.read_virtual_checked(
                        &mut proc.address_space,
                        path_va,
                        &mut path_buf,
                        true,
                    )?;
                    String::from_utf8(path_buf).map_err(|_| "Invalid UTF-8 in path")?
                } else {
                    "/tmp/default.txt".to_string()
                };

                let vfs_fd = kernel.vfs.open(&path_str, flags)?;

                // 挂载到当前进程专属的 fd_table
                let proc = kernel.pm.processes.get_mut(&curr_pid).unwrap();
                let user_fd = proc.alloc_fd(FileDescriptorEntry::new_vfs(vfs_fd, flags));
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

                let proc = kernel
                    .pm
                    .processes
                    .get(&curr_pid)
                    .ok_or("Process not found")?;
                let fd_entry = proc
                    .fd_table
                    .get(&user_fd)
                    .cloned()
                    .ok_or("Bad file descriptor")?;

                match fd_entry.descriptor_type() {
                    FileDescriptorType::VfsFile(vfs_fd) => {
                        let safe_len = len.min(70 * 1024);
                        let old_offset = kernel
                            .vfs
                            .open_files
                            .get(&vfs_fd)
                            .ok_or("Invalid file descriptor")?
                            .offset;

                        let mut kbuf = vec![0u8; safe_len];
                        let n = kernel.vfs.read(vfs_fd, &mut kbuf)?;

                        if n > 0 {
                            let proc = kernel.pm.processes.get_mut(&curr_pid).unwrap();
                            if let Err(e) = kernel.mm.write_virtual_checked(
                                &mut proc.address_space,
                                buf_va,
                                &kbuf[..n],
                                true,
                            ) {
                                let _ = kernel.vfs.seek(vfs_fd, old_offset);
                                return Err(e);
                            }
                        }

                        Ok(n as isize)
                    }
                    FileDescriptorType::PipeRead(pipe_id) => {
                        let safe_len = len.min(64 * 1024);
                        let (read_bytes, should_block) = {
                            let pipe = kernel.pm.pipes.get(&pipe_id).ok_or("Pipe not found")?;
                            if pipe.buffer.is_empty() {
                                if pipe.is_eof() { (0, false) } else { (0, true) }
                            } else {
                                let to_read = safe_len.min(pipe.buffer.len());
                                let mut kbuf = vec![0u8; to_read];
                                let n = pipe.peek(&mut kbuf);
                                let proc = kernel.pm.processes.get_mut(&curr_pid).unwrap();
                                kernel.mm.write_virtual_checked(
                                    &mut proc.address_space,
                                    buf_va,
                                    &kbuf[..n],
                                    true,
                                )?;
                                kernel.pm.pipes.get_mut(&pipe_id).unwrap().consume(n);
                                (n, false)
                            }
                        };

                        if should_block {
                            kernel
                                .pm
                                .block(curr_pid, BlockedReason::WaitingPipeRead { pipe_id });
                            Ok(-1)
                        } else {
                            if read_bytes > 0 {
                                kernel.pm.wake_pipe_writers(pipe_id);
                            }
                            Ok(read_bytes as isize)
                        }
                    }
                    FileDescriptorType::PipeWrite(_) => Err("Cannot read from write-end of pipe"),
                    FileDescriptorType::Stdin => {
                        let safe_len = len.min(64 * 1024);
                        let kbuf: Vec<u8> = kernel
                            .console_stdin
                            .iter()
                            .take(safe_len)
                            .copied()
                            .collect();
                        let n = kbuf.len();
                        if n > 0 {
                            let proc = kernel.pm.processes.get_mut(&curr_pid).unwrap();
                            kernel.mm.write_virtual_checked(
                                &mut proc.address_space,
                                buf_va,
                                &kbuf[..n],
                                true,
                            )?;
                            drop(kernel.console_stdin.drain(..n));
                        }
                        Ok(n as isize)
                    }
                    FileDescriptorType::Stdout | FileDescriptorType::Stderr => {
                        Err("Cannot read from stdout or stderr")
                    }
                }
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

                let proc = kernel
                    .pm
                    .processes
                    .get(&curr_pid)
                    .ok_or("Process not found")?;
                let fd_entry = proc
                    .fd_table
                    .get(&user_fd)
                    .cloned()
                    .ok_or("Bad file descriptor")?;

                match fd_entry.descriptor_type() {
                    FileDescriptorType::VfsFile(vfs_fd) => {
                        let safe_len = len.min(70 * 1024);
                        let mut tmp = vec![0u8; safe_len];
                        let proc = kernel.pm.processes.get_mut(&curr_pid).unwrap();
                        kernel.mm.read_virtual_checked(
                            &mut proc.address_space,
                            buf_va,
                            &mut tmp,
                            true,
                        )?;

                        let n = kernel.vfs.write(vfs_fd, &tmp)?;
                        Ok(n as isize)
                    }
                    FileDescriptorType::PipeWrite(pipe_id) => {
                        let safe_len = len.min(64 * 1024);
                        let mut tmp = vec![0u8; safe_len];
                        let proc = kernel.pm.processes.get_mut(&curr_pid).unwrap();
                        kernel.mm.read_virtual_checked(
                            &mut proc.address_space,
                            buf_va,
                            &mut tmp,
                            true,
                        )?;

                        let (written, should_block) = {
                            let pipe = kernel.pm.pipes.get_mut(&pipe_id).ok_or("Pipe not found")?;
                            if pipe.is_broken() {
                                return Err("Broken pipe: reader closed");
                            }
                            if pipe.available_write_space() == 0 {
                                (0, true)
                            } else {
                                let n = pipe.write(&tmp)?;
                                (n, false)
                            }
                        };

                        if should_block {
                            kernel
                                .pm
                                .block(curr_pid, BlockedReason::WaitingPipeWrite { pipe_id });
                            Ok(-1)
                        } else {
                            if written > 0 {
                                kernel.pm.wake_pipe_readers(pipe_id);
                            }
                            Ok(written as isize)
                        }
                    }
                    FileDescriptorType::PipeRead(_) => Err("Cannot write to read-end of pipe"),
                    FileDescriptorType::Stdout | FileDescriptorType::Stderr => {
                        let safe_len = len.min(64 * 1024);
                        let mut tmp = vec![0u8; safe_len];
                        let proc = kernel.pm.processes.get_mut(&curr_pid).unwrap();
                        kernel.mm.read_virtual_checked(
                            &mut proc.address_space,
                            buf_va,
                            &mut tmp,
                            true,
                        )?;
                        kernel.console_write(&tmp);
                        Ok(tmp.len() as isize)
                    }
                    FileDescriptorType::Stdin => Err("Cannot write to stdin"),
                }
            }

            SYS_CLOSE => {
                let user_fd = args.arg0;
                let proc = kernel.pm.processes.get_mut(&curr_pid).unwrap();
                let fd_entry = proc.close_fd(user_fd).ok_or("Bad file descriptor")?;
                kernel.close_fd_entry(fd_entry)?;
                Ok(0)
            }

            SYS_PIPE => {
                // arg0: pipefd_va (用户态缓冲区地址，若非 0 则写入 [read_fd as u32, write_fd as u32])
                // arg1: capacity (管道容量，若为 0 则默认 4096)
                let pipefd_va = args.arg0;
                let capacity = if args.arg1 > 0 { args.arg1 } else { 4096 };
                if capacity > 64 * 1024 {
                    return Err("Pipe capacity exceeds 64 KiB");
                }

                let pipe_id = kernel.pm.create_pipe(capacity);
                let read_entry = FileDescriptorEntry::new_pipe_read(pipe_id);
                let write_entry = FileDescriptorEntry::new_pipe_write(pipe_id);

                let proc = kernel.pm.processes.get_mut(&curr_pid).unwrap();
                let old_next_fd = proc.next_fd;
                let read_fd = proc.alloc_fd(read_entry);
                let write_fd = proc.alloc_fd(write_entry);

                if pipefd_va != 0 {
                    let mut bytes = [0u8; 8];
                    bytes[..4].copy_from_slice(&(read_fd as u32).to_ne_bytes());
                    bytes[4..].copy_from_slice(&(write_fd as u32).to_ne_bytes());
                    if let Err(err) = kernel.mm.write_virtual_checked(
                        &mut proc.address_space,
                        pipefd_va,
                        &bytes,
                        true,
                    ) {
                        proc.close_fd(read_fd);
                        proc.close_fd(write_fd);
                        proc.next_fd = old_next_fd;
                        kernel.pm.pipes.remove(&pipe_id);
                        return Err(err);
                    }
                    Ok(0)
                } else {
                    Ok(((write_fd as isize) << 16) | (read_fd as isize))
                }
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
                let proc = kernel.pm.processes.get_mut(&curr_pid).unwrap();
                kernel.mm.read_virtual_checked(
                    &mut proc.address_space,
                    path_va,
                    &mut path_buf,
                    true,
                )?;
                let path_str = String::from_utf8(path_buf).map_err(|_| "Invalid UTF-8 in path")?;
                let inode_id = kernel.vfs.mkdir(&path_str)?;
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
                let proc = kernel.pm.processes.get_mut(&curr_pid).unwrap();
                kernel.mm.read_virtual_checked(
                    &mut proc.address_space,
                    path_va,
                    &mut path_buf,
                    true,
                )?;
                let path_str = String::from_utf8(path_buf).map_err(|_| "Invalid UTF-8 in path")?;
                let stat = kernel.vfs.stat(&path_str)?;
                Ok(stat.size as isize)
            }

            SYS_EXEC => {
                let path_va = args.arg0;
                let path_len = args.arg1;
                if path_va == 0 || path_len == 0 {
                    return Err("Invalid path argument for exec");
                }
                if path_len > 256 {
                    return Err("Path too long (exceeds 256 bytes)");
                }
                let mut path_buf = vec![0u8; path_len];
                let proc = kernel.pm.processes.get_mut(&curr_pid).unwrap();
                kernel.mm.read_virtual_checked(
                    &mut proc.address_space,
                    path_va,
                    &mut path_buf,
                    true,
                )?;
                let path_str = String::from_utf8(path_buf).map_err(|_| "Invalid UTF-8 in path")?;
                kernel.exec_process(curr_pid, &path_str)?;
                Ok(0)
            }

            SYS_MMAP => {
                let hint_addr = args.arg0;
                let length = args.arg1;
                let prot = args.arg2 as u8;
                let flags = args.arg3 as u32;
                let addr = kernel.mmap(curr_pid, hint_addr, length, prot, flags, None, 0)?;
                Ok(addr as isize)
            }

            SYS_MUNMAP => {
                let addr = args.arg0;
                let length = args.arg1;
                kernel.munmap(curr_pid, addr, length)?;
                Ok(0)
            }

            _ => Err("Invalid or unimplemented syscall number"),
        }
    }
}
