//! Types related to task management
use super::TaskContext;
use crate::config::TRAP_CONTEXT_BASE;
use crate::mm::{
    frame_alloc, kernel_stack_position, MapPermission, MemorySet, PTEFlags, PhysPageNum, VPNRange,
    VirtAddr, KERNEL_SPACE,
};
use crate::trap::{trap_handler, TrapContext};

/// The task control block (TCB) of a task.
pub struct TaskControlBlock {
    /// Save task context
    pub task_cx: TaskContext,

    /// Maintain the execution status of the current process
    pub task_status: TaskStatus,

    /// Application address space
    pub memory_set: MemorySet,

    /// The phys page number of trap context
    pub trap_cx_ppn: PhysPageNum,

    /// The size(top addr) of program which is loaded from elf file
    pub base_size: usize,

    /// Heap bottom
    pub heap_bottom: usize,

    /// Program break
    pub program_brk: usize,
}

impl TaskControlBlock {
    /// get the trap context
    pub fn get_trap_cx(&self) -> &'static mut TrapContext {
        self.trap_cx_ppn.get_mut()
    }
    /// get the user token
    pub fn get_user_token(&self) -> usize {
        self.memory_set.token()
    }
    /// Based on the elf info in program, build the contents of task in a new address space
    pub fn new(elf_data: &[u8], app_id: usize) -> Self {
        // memory_set with elf program headers/trampoline/trap context/user stack
        let (memory_set, user_sp, entry_point) = MemorySet::from_elf(elf_data);
        let trap_cx_ppn = memory_set
            .translate(VirtAddr::from(TRAP_CONTEXT_BASE).into())
            .unwrap()
            .ppn();
        let task_status = TaskStatus::Ready;
        // map a kernel-stack in kernel space
        let (kernel_stack_bottom, kernel_stack_top) = kernel_stack_position(app_id);
        KERNEL_SPACE.exclusive_access().insert_framed_area(
            kernel_stack_bottom.into(),
            kernel_stack_top.into(),
            MapPermission::R | MapPermission::W,
        );
        let task_control_block = Self {
            task_status,
            task_cx: TaskContext::goto_trap_return(kernel_stack_top),
            memory_set,
            trap_cx_ppn,
            base_size: user_sp,
            heap_bottom: user_sp,
            program_brk: user_sp,
        };
        // prepare TrapContext in user space
        let trap_cx = task_control_block.get_trap_cx();
        *trap_cx = TrapContext::app_init_context(
            entry_point,
            user_sp,
            KERNEL_SPACE.exclusive_access().token(),
            kernel_stack_top,
            trap_handler as usize,
        );
        task_control_block
    }
    /// change the location of the program break. return None if failed.
    pub fn change_program_brk(&mut self, size: i32) -> Option<usize> {
        let old_break = self.program_brk;
        let new_brk = self.program_brk as isize + size as isize;
        if new_brk < self.heap_bottom as isize {
            return None;
        }
        let result = if size < 0 {
            self.memory_set
                .shrink_to(VirtAddr(self.heap_bottom), VirtAddr(new_brk as usize))
        } else {
            self.memory_set
                .append_to(VirtAddr(self.heap_bottom), VirtAddr(new_brk as usize))
        };
        if result {
            self.program_brk = new_brk as usize;
            Some(old_break)
        } else {
            None
        }
    }
    /// 根据传入的起始虚拟地址以及长度和标识来请求新的映射
    pub fn map_new_area_by_address(
        &mut self,
        start: usize,
        len: usize,
        port: usize,
        // index: usize,
    ) -> isize {
        // 是否应该判断一下会不会溢出？
        // println!("port{}", port);
        // println!("port as u8{}", port as u8);
        // let s_start: usize = VirtAddr(start).floor().into();
        // let e_start: usize = VirtAddr(start + len -1).floor().into();
        // println!("start: {:#x}", s_start);
        // println!("end: {:#x}", e_start);
        // println!("pear: {:#b}",MapPermission::from_bits((port << 1) as u8).unwrap() | MapPermission::U);
        // if port == 0 || (port & !0x7 != 0) {
        //     return -1;
        // }
        // if let Some(_) = self.memory_set.translate(VirtAddr(start).floor()) {
        //     // 表示已经存在了
        //     return -1;
        // }
        // if let Some(_) = self
        //     .memory_set
        //     .translate(VirtAddr(start + len - 1).floor())
        // {
        //     println!("aaaaaaaaaaaaaaaaaaaaaaa");
        //     // 表示已经存在了
        //     return -1;
        // }
        // self.memory_set.insert_framed_area(
        //     VirtAddr(start).floor().into(),
        //     VirtAddr(start + len - 1).floor().into(),
        //     // port xwr 210 而MapPermission里是 uxwr 421 ，因此需要将port先左移一位
        //     MapPermission::from_bits((port << 1) as u8).unwrap() | MapPermission::U,
        // );
        if port == 0 || (port & !0x7 != 0) {
            return -1;
        }
        // if VirtAddr(start) != VirtAddr(start).floor().into() { // 表示地址没对齐PAGE_SIZE
        if start % 4096 != 0 {
            // 表示地址没对齐PAGE_SIZE
            // println!("{} map  x : VPN:{:#x}", index, start);
            return -1;
        }
        // VPNRange 好像左闭右开
        for vpn in VPNRange::new(VirtAddr(start).floor(), VirtAddr(start + len).ceil()) {
            // let tmp: usize = vpn.into();
            if let Some(pte) = self.memory_set.page_table.find_pte(vpn) {
                // 表示已经存在了
                if pte.is_valid() {
                    // println!("{} map  xxxx : VPN:{:#x}", index, tmp);
                    return -1;
                }
                
            }

            // println!("{} map : VPN:{:#x}", index, tmp);
            // let pte = self.memory_set.page_table.find_pte(vpn).unwrap();
            // if pte.is_valid() {
            //     return -1;
            // }
            let frame = frame_alloc().unwrap();
            let ppn = frame.ppn;
            self.memory_set.page_table.map(
                vpn,
                ppn,
                PTEFlags::from_bits((port << 1) as u8).unwrap() | PTEFlags::U,
            );
            self.memory_set.page_table.frames.push(frame);
        }
        // println!("map finish.........");

        0
    }

    /// 删除映射
    pub fn unmap_area_by_address(&mut self, start: usize, len: usize) -> isize {
        // if let None = self.memory_set.translate(VirtAddr(start).floor().into()) {
        //     // 表示不存在
        //     return -1;
        // };

        // if let None = self
        //     .memory_set
        //     .translate(VirtAddr(start + len).floor().into())
        // {
        //     // 表示已经存在了
        //     return -1;
        // }
        // self.memory_set
        // .unmap_framed_area(VirtAddr(start), VirtAddr(start + len))

        for vpn in VPNRange::new(VirtAddr(start).floor(), VirtAddr(start + len).ceil()) {
            // let tmp: usize = vpn.into();
            // println!("VPN:{:#x}", tmp);
            let pte = self.memory_set.page_table.find_pte(vpn).unwrap();
            if !pte.is_valid() {
                return -1;
            }
            if let None = self.memory_set.page_table.find_pte(vpn) {
                // 表示该页面未映射， 返回错误
                return -1;
            }
            self.memory_set.page_table.unmap(vpn);
        }
        // println!("map finish.........");

        0
    }
}

#[derive(Copy, Clone, PartialEq)]
/// task status: UnInit, Ready, Running, Exited
pub enum TaskStatus {
    /// uninitialized
    UnInit,
    /// ready to run
    Ready,
    /// running
    Running,
    /// exited
    Exited,
}
