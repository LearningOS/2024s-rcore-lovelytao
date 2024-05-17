//! Implementation of process [`ProcessControlBlock`] and task(thread) [`TaskControlBlock`] management mechanism
//!
//! Here is the entry for task scheduling required by other modules
//! (such as syscall or clock interrupt).
//! By suspending or exiting the current task, you can
//! modify the task state, manage the task queue through TASK_MANAGER (in task/manager.rs) ,
//! and switch the control flow through PROCESSOR (in task/processor.rs) .
//!
//! Be careful when you see [`__switch`]. Control flow around this function
//! might not be what you expect.

mod context;
mod id;
mod manager;
mod process;
mod processor;
mod signal;
mod switch;
#[allow(clippy::module_inception)]
mod task;

use self::id::TaskUserRes;
use crate::fs::{open_file, OpenFlags};
use crate::task::manager::add_stopping_task;
use crate::timer::remove_timer;
use alloc::vec;
use alloc::{sync::Arc, vec::Vec};
use lazy_static::*;
use manager::fetch_task;
use process::ProcessControlBlock;
use switch::__switch;

pub use context::TaskContext;
pub use id::{kstack_alloc, pid_alloc, KernelStack, PidHandle, IDLE_PID};
pub use manager::{add_task, pid2process, remove_from_pid2process, remove_task, wakeup_task};
pub use processor::{
    current_kstack_top, current_process, current_task, current_trap_cx, current_trap_cx_user_va,
    current_user_token, run_tasks, schedule, take_current_task,
};
pub use signal::SignalFlags;
pub use task::{TaskControlBlock, TaskStatus};

/// Make current task suspended and switch to the next task
pub fn suspend_current_and_run_next() {
    // There must be an application running.
    let task = take_current_task().unwrap();

    // ---- access current TCB exclusively
    let mut task_inner = task.inner_exclusive_access();
    let task_cx_ptr = &mut task_inner.task_cx as *mut TaskContext;
    // Change status to Ready
    task_inner.task_status = TaskStatus::Ready;
    drop(task_inner);
    // ---- release current TCB

    // push back to ready queue.
    add_task(task);
    // jump to scheduling cycle
    schedule(task_cx_ptr);
}

/// Make current task blocked and switch to the next task.
pub fn block_current_and_run_next() {
    let task = take_current_task().unwrap();
    let mut task_inner = task.inner_exclusive_access();
    let task_cx_ptr = &mut task_inner.task_cx as *mut TaskContext;
    task_inner.task_status = TaskStatus::Blocked;
    drop(task_inner);
    schedule(task_cx_ptr);
}

use crate::board::QEMUExit;

/// Exit the current 'Running' task and run the next task in task list.
pub fn exit_current_and_run_next(exit_code: i32) {
    trace!(
        "kernel: pid[{}] exit_current_and_run_next",
        current_task().unwrap().process.upgrade().unwrap().getpid()
    );
    // take from Processor
    // 将当前线程出PROCESSOR中拿出来
    let task = take_current_task().unwrap();
    let mut task_inner = task.inner_exclusive_access();
    let process = task.process.upgrade().unwrap();
    let tid = task_inner.res.as_ref().unwrap().tid;
    // record exit code
    task_inner.exit_code = Some(exit_code);
    // 回收当前线程的线程资源组 u_stak  u_trap
    task_inner.res = None;
    // here we do not remove the thread since we are still using the kstack
    // it will be deallocated when sys_waittid is called
    drop(task_inner);

    let mut process_inner = process.inner_exclusive_access();

    // 将 tid这一列全部置为0
    process_inner.mutex_allocation[tid]
        .iter_mut()
        .for_each(|x| *x = 0);
    process_inner.mutex_need[tid]
        .iter_mut()
        .for_each(|x| *x = 0);
    process_inner.semaphore_allocation[tid]
        .iter_mut()
        .for_each(|x| *x = 0);
    process_inner.mutex_need[tid]
        .iter_mut()
        .for_each(|x| *x = 0);

    drop(process_inner);
    // Move the task to stop-wait status, to avoid kernel stack from being freed
    // 如果是主线程
    if tid == 0 {
        add_stopping_task(task);
    } else {
        // 不是主线程退出的话，完成上述操作后，直接丢掉即可
        drop(task);
    }
    // however, if this is the main thread of current process
    // the process should terminate at once
    if tid == 0 {
        let pid = process.getpid();
        if pid == IDLE_PID {
            println!(
                "[kernel] Idle process exit with exit_code {} ...",
                exit_code
            );
            if exit_code != 0 {
                //crate::sbi::shutdown(255); //255 == -1 for err hint
                crate::board::QEMU_EXIT_HANDLE.exit_failure();
            } else {
                //crate::sbi::shutdown(0); //0 for success hint
                crate::board::QEMU_EXIT_HANDLE.exit_success();
            }
        }
        // 根性PID-进程控制块映射
        remove_from_pid2process(pid);
        let mut process_inner = process.inner_exclusive_access();
        // mark this process as a zombie process
        process_inner.is_zombie = true;
        // record exit code of main process
        process_inner.exit_code = exit_code;

        {
            // move all child processes under init process
            // 把当前进程下的子进程都移到 INITPROC下面
            let mut initproc_inner = INITPROC.inner_exclusive_access();
            for child in process_inner.children.iter() {
                child.inner_exclusive_access().parent = Some(Arc::downgrade(&INITPROC));
                initproc_inner.children.push(child.clone());
            }
        }

        // deallocate user res (including tid/trap_cx/ustack) of all threads
        // it has to be done before we dealloc the whole memory_set
        // otherwise they will be deallocated twice
        let mut recycle_res = Vec::<TaskUserRes>::new();
        // 回收进程中的每一个线程的TaskUserRes，最后一起释放
        for task in process_inner.tasks.iter().filter(|t| t.is_some()) {
            let task = task.as_ref().unwrap();
            // if other tasks are Ready in TaskManager or waiting for a timer to be
            // expired, we should remove them.
            //
            // Mention that we do not need to consider Mutex/Semaphore since they
            // are limited in a single process. Therefore, the blocked tasks are
            // removed when the PCB is deallocated.
            trace!("kernel: exit_current_and_run_next .. remove_inactive_task");
            // 主线程退出的时候可能有一些线程处于就绪状态等在任务管理器 TASK_MANAGER 的队列中，
            // 我们需要及时调用 remove_inactive_task 函数将它们从队列中移除
            remove_inactive_task(Arc::clone(&task));
            let mut task_inner = task.inner_exclusive_access();
            if let Some(res) = task_inner.res.take() {
                recycle_res.push(res);
            }
        }
        // dealloc_tid and dealloc_user_res require access to PCB inner, so we
        // need to collect those user res first, then release process_inner
        // for now to avoid deadlock/double borrow problem.
        drop(process_inner);
        recycle_res.clear();

        let mut process_inner = process.inner_exclusive_access();
        process_inner.children.clear();
        // deallocate other data in user space i.e. program code/data section
        process_inner.memory_set.recycle_data_pages();
        // drop file descriptors
        process_inner.fd_table.clear();
        // remove all tasks
        process_inner.tasks.clear();
    }
    drop(process);
    // we do not have to save task context
    let mut _unused = TaskContext::zero_init();
    schedule(&mut _unused as *mut _);
}

lazy_static! {
    /// Creation of initial process
    ///
    /// the name "initproc" may be changed to any other app name like "usertests",
    /// but we have user_shell, so we don't need to change it.
    pub static ref INITPROC: Arc<ProcessControlBlock> = {
        let inode = open_file("ch8b_initproc", OpenFlags::RDONLY).unwrap();
        let v = inode.read_all();
        ProcessControlBlock::new(v.as_slice())
    };
}

///Add init process to the manager
pub fn add_initproc() {
    let _initproc = INITPROC.clone();
}

/// Check if the current task has any signal to handle
pub fn check_signals_of_current() -> Option<(i32, &'static str)> {
    let process = current_process();
    let process_inner = process.inner_exclusive_access();
    process_inner.signals.check_error()
}

/// Add signal to the current task
pub fn current_add_signal(signal: SignalFlags) {
    let process = current_process();
    let mut process_inner = process.inner_exclusive_access();
    process_inner.signals |= signal;
}

/// the inactive(blocked) tasks are removed when the PCB is deallocated.(called by exit_current_and_run_next)
pub fn remove_inactive_task(task: Arc<TaskControlBlock>) {
    remove_task(Arc::clone(&task));
    trace!("kernel: remove_inactive_task .. remove_timer");
    remove_timer(Arc::clone(&task));
}

/// 检查系统是否处于安全状态
/// f: 查看类型， res：表示要哪个资源, num 表示请求多少个  输出：0表示等待，1表示正确，2表示死锁
pub fn check_system_state(t: usize, _res: usize, _num: isize) -> bool {
    let process = current_process();
    let process_inner = process.inner_exclusive_access();
    // let tid = current_task()
    //     .unwrap()
    //     .inner_exclusive_access()
    //     .res
    //     .as_ref()
    //     .unwrap()
    //     .tid;

    let (available, need, allocation);
    if t == 0 {
        // 0 表示mutex
        available = process_inner.mutex_available.clone();
        need = process_inner.mutex_need.clone();
        allocation = process_inner.mutex_allocation.clone();
    } else {
        // 0 表示mutex
        available = process_inner.semaphore_available.clone();
        need = process_inner.semaphore_need.clone();
        allocation = process_inner.semaphore_allocation.clone();
    }

    drop(process_inner);
    // if available[res] < num {
    //     // 不够， 先去等待资源
    //     return true;
    // }

    // 先尝试分配。分配完查看是否满足
    // available[res] -= num;
    // allocation[tid][res] += num;
    // need[tid][res] -= num;

    let mut work = available.clone();
    let mut finish = vec![false; need.len()];
    let mut i = 0;
    // println!("tid :{}    finish.len:{}    ", tid, finish.len());
    while i < finish.len() {
        // println!("tid :{}   need[{}][{}] = {}, wrok[{}] = {}", tid, i, res, need[i][res],res, work[res]);
        // if finish[i] == false && need[i][res] <= work[res] {
        //     // 假设分给他后，能够顺利执行，并且完成释放出分配给它的资源
        //     work[res] += allocation[i][res];
        //     println!("add i = {}   work[{}] = {}", i, res, work[res]);
        //     finish[i] = true;

        //     i = 0;
        // } else {
        //     i += 1;
        // }

        if finish[i] == false {
            // 懂了！！！！  你每次得比较所有的资源，不能只比较一个
            // 因为可能当前某一个线程依赖多个资源，因此需要满足所有需求才能进行释放
            if (0..work.len()).all(|r| need[i][r] <= work[r]) {
                // 假设分给他后，能够顺利执行，并且完成释放出分配给它的资源
                for r in 0..work.len() {
                    work[r] += allocation[i][r];
                }
                // println!("add i = {}   work = {:?}", i, work);
                finish[i] = true;
                i = 0;
            } else {
                i += 1;
            }
        } else {
            i += 1;
        }
    }

    for i in 0..finish.len() {
        if !finish[i] {
            return false;
        }
    }

    return true;
}
