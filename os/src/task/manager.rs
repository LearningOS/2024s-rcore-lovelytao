//!Implementation of [`TaskManager`]
use super::TaskControlBlock;
use crate::sync::UPSafeCell;
// use alloc::collections::VecDeque;
use alloc::sync::Arc;
use alloc::vec::Vec;
use lazy_static::*;

pub const BIG_STRIDE: usize = 16 * 16;

///A array of `TaskControlBlock` that is thread-safe
pub struct TaskManager {
    ready_queue: Vec<Arc<TaskControlBlock>>,
    // ready_queue: VecDeque<Arc<TaskControlBlock>>,
}

/// A simple FIFO scheduler.
impl TaskManager {
    ///Creat an empty TaskManager
    pub fn new() -> Self {
        Self {
            // 双端队列方便任务轮流执行，每次执行新任务从队头获取。每次重新排队时都放入队尾
            // ready_queue: VecDeque::new(),
            ready_queue: Vec::new(),
        }
    }
    /// Add process back to ready queue
    pub fn add(&mut self, task: Arc<TaskControlBlock>) {
        // 修改这里

        // self.ready_queue.push_back(task);
        self.ready_queue.push(task);
    }
    /// Take a process out of the ready queue
    pub fn fetch(&mut self) -> Option<Arc<TaskControlBlock>> {
        // 修改这里
        // self.ready_queue.pop_front()

        let mut index = 0;
        let l = self.ready_queue.len();
        if l == 0 {
            return None;
        }
        for i in 0..l {
            let index_task_stride = self.ready_queue[index].inner_exclusive_access().stride;
            let i_task_stride = self.ready_queue[i].inner_exclusive_access().stride;
            if i_task_stride < index_task_stride {
                index = i;
            }
        }
        let priority = self.ready_queue[index].inner_exclusive_access().priority;
        self.ready_queue[index].inner_exclusive_access().stride += BIG_STRIDE / priority as usize;
        Some(self.ready_queue.remove(index))

        // self.ready_queue.
    }
}

lazy_static! {
    /// TASK_MANAGER instance through lazy_static!
    pub static ref TASK_MANAGER: UPSafeCell<TaskManager> =
        unsafe { UPSafeCell::new(TaskManager::new()) };
}

/// Add process to ready queue
pub fn add_task(task: Arc<TaskControlBlock>) {
    //trace!("kernel: TaskManager::add_task");
    TASK_MANAGER.exclusive_access().add(task);
}

/// Take a process out of the ready queue
pub fn fetch_task() -> Option<Arc<TaskControlBlock>> {
    //trace!("kernel: TaskManager::fetch_task");
    TASK_MANAGER.exclusive_access().fetch()
}
