use core::ops::DerefMut;

use crate::{
    mm::kernel_token,
    task::{add_task, current_task, TaskControlBlock},
    trap::{trap_handler, TrapContext},
};
use alloc::{sync::Arc, vec};
/// thread create syscall
pub fn sys_thread_create(entry: usize, arg: usize) -> isize {
    trace!(
        "kernel:pid[{}] tid[{}] sys_thread_create",
        current_task().unwrap().process.upgrade().unwrap().getpid(),
        current_task()
            .unwrap()
            .inner_exclusive_access()
            .res
            .as_ref()
            .unwrap()
            .tid
    );
    // 找到当前正在执行的线程task和此线程所属的进程process
    let task = current_task().unwrap();
    let process = task.process.upgrade().unwrap();

    // create a new thread
    // 在创建过程中，建立与进程process的所属关系，分配了线程资源组TaskUserRes和其他资源
    let new_task = Arc::new(TaskControlBlock::new(
        Arc::clone(&process),
        task.inner_exclusive_access()
            .res
            .as_ref()
            .unwrap()
            .ustack_base,
        true,
    ));
    // add new task to scheduler
    add_task(Arc::clone(&new_task));

    let new_task_inner = new_task.inner_exclusive_access();
    let new_task_res = new_task_inner.res.as_ref().unwrap();
    let new_task_tid = new_task_res.tid;
    let mut process_inner = process.inner_exclusive_access();
    // add new thread to current process
    let tasks = &mut process_inner.tasks;
    while tasks.len() < new_task_tid + 1 {
        tasks.push(None);

        // 这个时候需要新增一行
    }
    // 把线程接入到所属进程的线程列表tasks中
    tasks[new_task_tid] = Some(Arc::clone(&new_task));
    // drop(task);
    // 新增一行
    let mutex_len = process_inner.mutex_list.len();
    let semaphore_len = process_inner.semaphore_list.len();
    process_inner
        .deref_mut()
        .mutex_allocation
        .push(vec![0; mutex_len]);
    process_inner
        .deref_mut()
        .mutex_need
        .push(vec![0; mutex_len]);
    process_inner
        .deref_mut()
        .semaphore_allocation
        .push(vec![0; semaphore_len]);
    process_inner
        .deref_mut()
        .semaphore_need
        .push(vec![0; semaphore_len]);
    // println!(
    //     "semaphore_need.len = {}",
    //     process_inner.semaphore_need.len()
    // );

    let new_task_trap_cx = new_task_inner.get_trap_cx();
    // 初始化位于该线程在用户态地址空间中的 Trap 上下文
    *new_task_trap_cx = TrapContext::app_init_context(
        entry,                     // 初始化位于该线程在用户态地址空间中的 Trap 上下文
        new_task_res.ustack_top(), // 栈内部从上往下存放数据，所以存放栈顶
        kernel_token(),
        new_task.kstack.get_top(),
        trap_handler as usize,
    );

    (*new_task_trap_cx).x[10] = arg;
    new_task_tid as isize
}
/// get current thread id syscall
pub fn sys_gettid() -> isize {
    trace!(
        "kernel:pid[{}] tid[{}] sys_gettid",
        current_task().unwrap().process.upgrade().unwrap().getpid(),
        current_task()
            .unwrap()
            .inner_exclusive_access()
            .res
            .as_ref()
            .unwrap()
            .tid
    );
    current_task()
        .unwrap()
        .inner_exclusive_access()
        .res
        .as_ref()
        .unwrap()
        .tid as isize
}

/// wait for a thread to exit syscall
///
/// thread does not exist, return -1
/// thread has not exited yet, return -2
/// otherwise, return thread's exit code
pub fn sys_waittid(tid: usize) -> i32 {
    trace!(
        "kernel:pid[{}] tid[{}] sys_waittid",
        current_task().unwrap().process.upgrade().unwrap().getpid(),
        current_task()
            .unwrap()
            .inner_exclusive_access()
            .res
            .as_ref()
            .unwrap()
            .tid
    );
    let task = current_task().unwrap();
    let process = task.process.upgrade().unwrap();
    let task_inner = task.inner_exclusive_access();
    let mut process_inner = process.inner_exclusive_access();
    // a thread cannot wait for itself
    if task_inner.res.as_ref().unwrap().tid == tid {
        // 如果是线程等自己，返回错误
        return -1;
    }
    let mut exit_code: Option<i32> = None;
    let waited_task = process_inner.tasks[tid].as_ref();
    // 如果找到tid对应的退出线程，则收集该退出线程的退出码 exit_tid
    if let Some(waited_task) = waited_task {
        if let Some(waited_exit_code) = waited_task.inner_exclusive_access().exit_code {
            exit_code = Some(waited_exit_code);
        }
    } else {
        // waited thread does not exist
        return -1;
    }
    if let Some(exit_code) = exit_code {
        // dealloc the exited thread
        // 如果退出码存在，则从进程的线程向量中将被等待的线程删除。
        // 这意味着该函数返回之后，被等待线程的 TCB 的引用计数将被归零从而相关资源被完全回收
        process_inner.tasks[tid] = None;
        exit_code
    } else {
        // waited thread has not exited
        -2
    }
}
