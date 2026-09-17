//! `CompileCtx` 中心状态族：单函数编译上下文与其常量池键、类字段缓冲。
//!
//! 执行流字段（insts/registers/constants/labels）平铺在 `CompileCtx` 上，标识符
//! 绑定与闭包捕获分别下沉到 `SymbolTable` / `captured_bindings`。`assemble_ir`
//! 组装产出分域组合的 `IRFunction`，由 `oxide_ir::lower` 降为 bytecode。

use std::collections::{BTreeMap, HashMap, HashSet};

use oxide_bytecode::module::{Constant, UpvalueCapture};
use oxide_ir::inst::Inst;
use oxide_ir::operand::LabelId;
use oxide_ir::IRFunction;

use crate::emit_ctx::{LabelCtx, LabelScope, LoopEntry, LoopKind, ScopeCtx};
use crate::program::{BUILTIN_GLOBALS, NON_WRITABLE_GLOBAL_BUILTINS};
use crate::symbol_table::{ScopeKind, SymbolTable};
use oxide_parser::VariableDeclarationKind;

pub(crate) struct FieldBuffer {
    pub(crate) insts: Vec<Inst>,
    pub(crate) labels: Vec<(LabelId, usize)>,
}

/// 单函数编译上下文：执行流 + 作用域 + 闭包捕获的聚合状态。
/// 指令、常量池、寄存器分配平铺于此，作用域/绑定见 `ScopeCtx`，跳转见 `LabelCtx`。
pub struct CompileCtx {
    pub(crate) insts: Vec<Inst>,
    pub(crate) constants: Vec<Constant>,
    constant_map: HashMap<ConstantKey, u16>,
    pub(crate) next_reg: u32,
    pub(crate) max_regs: u32,
    pub(crate) reserved_reg_start: u32,
    pub(crate) labels: LabelCtx,
    pub(crate) scopes: ScopeCtx,
    pub(crate) nested: Vec<IRFunction>,
    /// 外层函数上下文中持有 `this` 的寄存器。
    /// 箭头函数用它捕获词法 `this`；顶层初始化为 254（约定 this 寄存器）。
    pub(crate) enclosing_this_reg: u8,
    pub(crate) in_derived_constructor: bool,
    pub(crate) in_instance_method: bool,
    pub(crate) in_static_method: bool,
    /// 本函数是否为生成器函数体（`function*`），`assemble_ir` 回写到 IR。
    pub(crate) is_generator: bool,
    /// 本函数是否为异步函数体（`async function` / async 箭头），`assemble_ir` 回写到 IR。
    pub(crate) is_async: bool,
    /// 本函数是否为严格模式函数体：自身 directive ‖ 外层严格继承（编译入口设置 +
    /// 嵌套继承，见 compile_function_body_with_field_hooks_gen），`assemble_ir` 回写到 IR。
    pub(crate) is_strict: bool,
    /// 是否为脚本顶层模块（emit_program 的根上下文）。顶层 var/function 声明需
    /// 同步写全局对象属性（脚本环境记录的 var 可经 globalThis 反射）；函数体为 false。
    pub(crate) is_global_scope: bool,
    /// 是否为 REPL 持久模式：脚本顶层 let/const 也写全局对象属性，使跨轮次
    /// 读取（LOAD_GLOBAL）可见变量绑定。仅 `eval_repl` 设置。
    pub(crate) repl_persist: bool,
    /// 是否为 eval 脚本：脚本顶层 var/function 声明落全局对象时属性
    /// configurable:true（普通脚本顶层为 false）。仅动态脚本入口设置。
    pub(crate) is_eval_script: bool,
    /// 编译树是否源自 eval 脚本：eval 代码自身顶层 var/function 名物化 c:true
    /// 全局属性、可删，本标志随嵌套函数继承，供 delete 分类判定。不直接继承
    /// `is_eval_script`——该标志只标识当前顶层程序，且还控制顶层全局写点与块级
    /// 函数门控，整体传播语义过宽。
    pub(crate) is_eval_origin: bool,
    /// 源码是否为 `source_escape` 产物（动态编译入口）：正则字面量源文本切片
    /// 的池键形态据此选择（见 [`Emitter::source_encoded`]）。
    pub(crate) source_encoded: bool,
    pub(crate) static_block_this_reg: Option<u8>,
    pub(crate) field_buffer: Option<FieldBuffer>,
    /// 类构造器模块中 `@@field_keys` upvalue 下标（实例字段 computed key 数组）。
    /// 类定义期求值一次存入 cell，构造器经此 upvalue 读取。
    pub(crate) field_keys_uv: Option<u8>,
    /// 类定义期全部 computed key 数组寄存器（方法/静态字段阶段按 slot 读取）。
    pub(crate) class_keys_reg: Option<u32>,
    pub(crate) current_upvalue_captures: Vec<UpvalueCapture>,
    /// 本函数作用域声明的绑定名（参数 + 变量/函数声明，AST 收集，emit 前确定）。
    pub(crate) own_bindings: HashSet<String>,
    /// 本函数形参名集（含解构形参叶子与 rest，编译入口收集）：块级函数名与
    /// 形参同名时不建外层绑定、不求值写回（规范 paramNames 守卫面）。子函数
    /// ctx 为新建，不继承。
    pub(crate) param_names: HashSet<String>,
    /// 块级函数名按 sloppy 模式下与浏览器/web 实现惯例兼容的行为建外层绑定的抑制集
    /// （形参 ∪ 函数作用域树内词法声明名）：抑制集内名字退化为纯块作用域；预声明期定稿，实例化与声明点写回两侧同查。
    pub(crate) block_fn_suppressed: HashSet<String>,
    /// 块入口已物化声明登记表（按块层级压/弹）：块直接子与标签直接子体
    /// 的函数声明在块入口物化闭包后，按 名 → 声明节点 arena 指针 登记（同名
    /// 重复声明只登记首个节点，任一直接子声明点都复用块槽取入口当前值）。
    /// 声明点据此三分：同节点命中（本声明已入口物化）→ 复用块槽保闭包
    /// 同一；仅同名命中（支臂声明与直接子同名共享块槽）→ 闭包落 fresh
    /// 寄存器只写回外层 var、块槽不动；均未命中 → 执行期自物化入块槽。
    /// 节点指针解析产物 arena 地址，同一编译单元内稳定。
    pub(crate) block_fn_entry_mats: Vec<HashMap<String, *const ()>>,
    /// 本函数被嵌套函数捕获的绑定名 → cell_idx（名字排序分配，稳定跨 run）。
    /// 捕获判断（MAKE_CELL / CELL_GET / CELL_SET）与子函数 upvalue cell_idx 统一查此映射，
    /// 消除符号表时序依赖与 cell 索引错位。
    pub(crate) captured_bindings: BTreeMap<String, u8>,
    /// 正在发射的 C 风格 for 头 let/const 声明名（含解构叶）。头名不像块级
    /// let/const 那样在块入口预声明，init 表达式内创建的嵌套函数引用同头尚未
    /// declare 的前向名时，父捕获可见性过滤据此放行；init 发射完毕后移除，
    /// 嵌套 for 头取并集。
    pub(crate) pending_for_head_names: HashSet<String>,
    /// 顶层已声明 var 名与顶层函数声明名（脚本全局声明实例化提升名）：裸读走全局
    /// 对象属性（LOAD_GLOBAL）、裸写走感知全局对象属性描述符的写；值的唯一存储是
    /// 全局对象属性（顶层 var 的唯一存储），引擎侧不再保留镜像副本。
    /// 仅脚本顶层 emit_program 计算，逐层继承给嵌套函数（嵌套函数的局部同名遮蔽不属此集，由作用域索引判定）。
    pub(crate) global_tier_names: HashSet<String>,
    /// 本函数从父函数捕获的 const 绑定名：子 ctx 不继承父函数作用域符号表，
    /// 捕获 const 信息随 upvalue 收集一并快照，供 const 写检查（编译期拦截）使用。
    pub(crate) upvalue_const_flags: HashSet<String>,
    /// 未声明标识符读所分配的全局槽寄存器集合：标识符首次读未命中任何作用域时，
    /// `lookup_or_builtin` 按隐式全局登记并记录其寄存器，后续读取据此发射
    /// LOAD_GLOBAL（运行期查 global object 属性，缺失抛 ReferenceError）。
    /// 按寄存器而非名字记录：块作用域同名新绑定持不同槽位，不会被误判为隐式全局。
    pub(crate) implicit_global_reads: HashSet<u32>,
    /// 未声明标识符写所登记的全局槽寄存器集合：`lookup_or_global` 未命中任何作用域
    /// 时登记全局作用域绑定并记录其寄存器，写调用点据此（经 `is_implicit_global_reg`
    /// 与读集合取并）补全局对象属性写（sloppy）或抛 ReferenceError（strict）。
    /// 子函数 ctx 从父继承——继承绑定命中同一寄存器，补写/抛错判定跨嵌套函数一致。
    pub(crate) implicit_global_writes: HashSet<u32>,
    /// 函数 `length` 属性值：首个带默认值形参之前的形参数（rest 不计）。
    /// emit_params_prologue 前由编译入口从 param_specs 计算。
    pub(crate) function_length: u32,
    pub(crate) const_overflow: bool,
    /// with 语句作用域栈：元素为 (with 对象寄存器, 打开时的作用域深度)。
    /// 非空时 with 体内的自由标识符需动态解析（先查对象属性，回退外层）。
    pub(crate) with_stack: Vec<(u32, usize)>,
    /// 打开中（尚未 emit 对应 END）的 try handler 栈，自底向上镜像运行时
    /// try_stack 组成。每项标记是否为纯 catch handler（TRY_BEGIN，由 TRY_END
    /// 弹出）：return 逃出 try 域时据此弹出栈顶连续纯 catch，防 handler 泄漏。
    pub(crate) open_try_handlers: Vec<bool>,
    /// 循环 update 段中应走寄存器（而非 cell）的被捕获绑定名：C 风格 for 的
    /// let/const 循环变量每迭代新分配一个 cell，update 写寄存器（不污染本迭代
    /// 闭包捕获的 cell），下一迭代把这个寄存器值拷入新分配的 cell。
    pub(crate) register_update_names: Vec<String>,
    /// 正在发射的 C 风格 for 头词法绑定名（含解构叶）：这些头名在 init 中经覆盖
    /// cell 建 `MAKE_CELL`，但循环体/test/update 读的是绑定寄存器、每迭代 fresh
    /// 拷贝的源也是寄存器，故 `emit_bind_target` 对集合内名字在 cell 写之外补写
    /// 绑定寄存器。init 发射完毕后清空。
    pub(crate) for_head_store_registers: HashSet<String>,
    /// 模块编译上下文：当前模块命名空间对象寄存器（`__moduleObject` 返回值）。
    pub(crate) module_ns_reg: Option<u32>,
    /// 已求值依赖模块的命名空间对象寄存器（按 import/export source 字符串索引）。
    pub(crate) module_dep_ns_regs: HashMap<String, u32>,
    /// 自导入（import from 自身）的 source 字符串集合：绑定走别名语义，不能链接期快照。
    pub(crate) module_self_import_specs: HashSet<String>,
    /// 自导入别名：导出名 → 本地绑定槽寄存器（export 语句执行时回写绑定值）。
    /// 仅承载无法静态解析源绑定的退化占位路径（star 转发的自导入名）。
    pub(crate) module_self_aliases: HashMap<String, u32>,
    /// 自导入别名（本地名，基源绑定名）：供别名捕获后处理合并源/别名 cell。
    pub(crate) module_alias_pairs: Vec<(String, String)>,
    /// 标签模板 site 计数器：本编译树内全局唯一（子 ctx 继承父值继续递增）。
    /// 运行时与模块 flat_id 组成模板对象缓存键，保证同一编译树同 site 恒返回
    /// 同一对象、不同编译树（eval 每次编译）互不共享。
    pub(crate) next_template_site: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum ConstantKey {
    Number(u64),
    Int(i32),
    BigInt(num_bigint::BigInt),
    String(String),
    Boolean(bool),
    Null,
    Undefined,
}

impl CompileCtx {
    pub(crate) fn new() -> Self {
        Self {
            insts: Vec::new(),
            constants: Vec::new(),
            constant_map: HashMap::new(),
            next_reg: 1,
            max_regs: 1,
            reserved_reg_start: 1,
            labels: LabelCtx {
                label_pos: Vec::new(),
                loop_stack: Vec::new(),
                switch_stack: Vec::new(),
                label_scopes: Vec::new(),
                pending_loop_labels: Vec::new(),
                finally_depth: 0,
                for_of_depth: 0,
                for_in_depth: 0,
                label_counter: 0,
            },
            scopes: ScopeCtx {
                symbols: SymbolTable::new(),
                builtin_reg_map: Vec::new(),
                private_name_map: Vec::new(),
                private_element_kinds: Vec::new(),
                private_brand_id: None,
                next_private_name_id: 1,
            },
            nested: Vec::new(),
            enclosing_this_reg: 254, // conventional this register at top level
            in_derived_constructor: false,
            in_instance_method: false,
            in_static_method: false,
            is_generator: false,
            is_async: false,
            is_strict: false,
            is_global_scope: false,
            repl_persist: false,
            is_eval_script: false,
            is_eval_origin: false,
            source_encoded: false,
            static_block_this_reg: None,
            field_buffer: None,
            field_keys_uv: None,
            class_keys_reg: None,
            current_upvalue_captures: Vec::new(),
            own_bindings: HashSet::new(),
            param_names: HashSet::new(),
            block_fn_suppressed: HashSet::new(),
            block_fn_entry_mats: Vec::new(),
            captured_bindings: BTreeMap::new(),
            pending_for_head_names: HashSet::new(),
            global_tier_names: HashSet::new(),
            upvalue_const_flags: HashSet::new(),
            implicit_global_reads: HashSet::new(),
            implicit_global_writes: HashSet::new(),
            function_length: 0,
            const_overflow: false,
            with_stack: Vec::new(),
            open_try_handlers: Vec::new(),
            register_update_names: Vec::new(),
            for_head_store_registers: HashSet::new(),
            module_ns_reg: None,
            module_dep_ns_regs: HashMap::new(),
            module_self_import_specs: HashSet::new(),
            module_self_aliases: HashMap::new(),
            module_alias_pairs: Vec::new(),
            next_template_site: 0,
        }
    }

    pub(crate) fn inst(&mut self, inst: Inst) {
        self.insts.push(inst);
    }

    pub(crate) fn alloc_reg(&mut self) -> u32 {
        let r = self.next_reg;
        // vreg 化：寄存器号无上限，RegAlloc 阶段负责压缩到物理域（≤253）。
        // 254/255 是 VM 保留的 this/new.target，vreg 世界允许虚拟号越过它们，
        // 只有 RegAlloc 完成映射后 lower 的物理域检查才相关。
        self.next_reg += 1;
        if self.next_reg > self.max_regs {
            self.max_regs = self.next_reg;
        }
        r
    }

    pub(crate) fn reset_regs(&mut self) {
        self.next_reg = self.builtin_reg_floor().max(self.reserved_reg_start);
        self.labels.label_counter = 0;
    }

    pub(crate) fn reserve_reg(&mut self, reg: u32) {
        let next = reg.wrapping_add(1);
        if self.next_reg <= reg {
            self.next_reg = next;
        }
        if self.max_regs < next {
            self.max_regs = next;
        }
    }

    pub(crate) fn add_constant(&mut self, c: Constant) -> u16 {
        if let Some(key) = ConstantKey::from_constant(&c) {
            if let Some(&idx) = self.constant_map.get(&key) {
                return idx;
            }

            if self.constants.len() >= u16::MAX as usize {
                self.const_overflow = true;
                return u16::MAX;
            }
            let idx = self.constants.len() as u16;
            self.constants.push(c);
            self.constant_map.insert(key, idx);
            return idx;
        }

        let idx = self.constants.len();
        if idx >= u16::MAX as usize {
            self.const_overflow = true;
            return u16::MAX;
        }
        self.constants.push(c);
        idx as u16
    }

    pub(crate) fn push_scope(&mut self) {
        self.scopes.symbols.push_scope();
    }

    pub(crate) fn pop_scope(&mut self) {
        self.scopes.symbols.pop_scope();
    }

    pub(crate) fn declare(
        &mut self, name: &str, reg: u32, kind: VariableDeclarationKind, is_const: bool,
    ) -> Result<(), String> {
        self.scopes.symbols.declare(name, reg, kind, is_const)
    }

    pub(crate) fn declare_initialized(
        &mut self, name: &str, reg: u32, kind: VariableDeclarationKind, is_const: bool,
    ) -> Result<(), String> {
        self.scopes.symbols.declare_initialized(name, reg, kind, is_const)
    }

    pub(crate) fn declare_predeclared(
        &mut self, name: &str, reg: u32, kind: VariableDeclarationKind, is_const: bool,
    ) -> Result<(), String> {
        self.scopes.symbols.declare_predeclared(name, reg, kind, is_const)
    }

    pub(crate) fn push_scope_with_kind(&mut self, kind: ScopeKind) {
        self.scopes.symbols.push_scope_with_kind(kind);
    }

    pub(crate) fn lookup(&self, name: &str) -> Result<u32, String> {
        self.scopes.symbols.lookup(name)
    }

    pub(crate) fn lookup_or_builtin(&mut self, name: &str) -> Result<u32, String> {
        match self.scopes.symbols.lookup(name) {
            Ok(reg) => Ok(reg),
            // 未声明标识符按隐式全局登记：读取语义（读未声明应抛 ReferenceError）由
            // 发射端按 implicit_global_reads 择 LOAD_GLOBAL 实现；`typeof X` 守卫等
            // 合法 JS 在运行期判定未定义（typeof 特判走 LOAD_GLOBAL_TYPEOF）。
            Err(err) if err.contains("is not defined") => {
                let reg = self.alloc_reg();
                self.scopes.symbols.pre_register_global(name, reg);
                self.scopes.builtin_reg_map.push((name.to_string(), reg));
                // 内置名（预扫描阶段在空符号表上登记）不是用户未声明读，不标记——
                // 否则内置标识符读取全部改走 LOAD_GLOBAL（热路径退化 + 槽位被修剪）。
                if !Self::is_known_builtin(name) {
                    self.implicit_global_reads.insert(reg);
                }
                Ok(reg)
            }
            Err(err) => Err(err),
        }
    }

    pub(crate) fn lookup_or_global(&mut self, name: &str) -> u32 {
        if let Some(reg) = self.scopes.symbols.lookup_any(name) {
            return reg;
        }
        let reg = self.alloc_reg();
        // 未声明标识符写：登记全局作用域绑定，记录寄存器供写调用点补全局对象属性
        // 写（sloppy）或抛 ReferenceError（strict）。
        self.implicit_global_writes.insert(reg);
        self.scopes.symbols.lookup_or_global(name, reg)
    }

    /// 寄存器是否承载未声明标识符的全局槽（写调用点据此补全局对象属性写或抛
    /// ReferenceError）。读写两个登记集合取并：未声明名谁先引用谁登记，读侧登记
    /// （LOAD_GLOBAL 槽）与写侧登记同属一个全局槽，写都必须穿透到全局对象。
    pub(crate) fn is_implicit_global_reg(&self, reg: u32) -> bool {
        self.implicit_global_writes.contains(&reg) || self.implicit_global_reads.contains(&reg)
    }

    /// 名字在当前作用域链内是否存在真实词法绑定（排除隐式全局登记），命中时返回其寄存器。
    ///
    /// 未声明名的隐式全局登记（写侧 `lookup_or_global`、读侧 `lookup_or_builtin` 经
    /// `pre_register_global`）会在全局作用域插入占位绑定，使 `lookup_any_binding` 按名
    /// 命中；但它不是词法声明。捕获名退出作用域后若曾被登记为隐式全局，按名命中会
    /// 误读残留 cell——捕获 cell 的可见性判定统一经本入口。
    pub(crate) fn visible_binding_reg(&self, name: &str) -> Option<u32> {
        let (binding, _) = self.scopes.symbols.lookup_any_binding(name)?;
        (!self.is_implicit_global_reg(binding.reg)).then_some(binding.reg)
    }

    pub(crate) fn lookup_const_flag(&self, name: &str) -> bool {
        // upvalue 捕获的 const：子 ctx 符号表不含父函数作用域绑定，查快照标志。
        self.scopes.symbols.lookup_is_const(name) || self.upvalue_const_flags.contains(name)
    }

    pub(crate) fn init_var(&mut self, name: &str) {
        self.scopes.symbols.init_var(name);
    }

    /// 声明本地名为源绑定活引用的别名（模块自导入）：槽位复用源寄存器，
    /// TDZ/提升/活值/不可变全部委托源绑定。
    pub(crate) fn add_alias(&mut self, local: &str, source: &str) -> Result<(), String> {
        self.scopes.symbols.add_alias(local, source)
    }

    pub(crate) fn next_label_id(&mut self) -> u32 {
        let id = self.labels.label_counter;
        self.labels.label_counter += 1;
        id
    }

    pub(crate) fn push_loop(&mut self, break_label: LabelId, continue_label: LabelId, kind: LoopKind) {
        let fd = self.labels.finally_depth;
        // 深度计数先递增再快照：条目记录"打开后（含自身）"的深度，与
        // take_pending_loop_labels 的标签作用域快照一致，逃出计数才能对齐。
        if kind.is_for_of() {
            self.labels.for_of_depth += 1;
        }
        if kind.is_for_in() {
            self.labels.for_in_depth += 1;
        }
        let fod = self.labels.for_of_depth;
        let fid = self.labels.for_in_depth;
        self.labels.loop_stack.push(LoopEntry {
            break_label,
            continue_label,
            finally_depth_at_open: fd,
            for_of_depth_at_open: fod,
            for_in_depth_at_open: fid,
            kind,
        });
    }

    pub(crate) fn pop_loop(&mut self) {
        if let Some(entry) = self.labels.loop_stack.pop() {
            if entry.kind.is_for_of() {
                self.labels.for_of_depth -= 1;
            }
            if entry.kind.is_for_in() {
                self.labels.for_in_depth -= 1;
            }
        }
    }

    pub(crate) fn current_loop(&self) -> Option<&LoopEntry> {
        self.labels.loop_stack.last()
    }

    pub(crate) fn push_switch(&mut self, break_label: LabelId) {
        let fd = self.labels.finally_depth;
        let fod = self.labels.for_of_depth;
        let fid = self.labels.for_in_depth;
        self.labels.switch_stack.push((break_label, fd, fod, fid));
    }

    pub(crate) fn pop_switch(&mut self) {
        self.labels.switch_stack.pop();
    }

    pub(crate) fn current_switch(&self) -> Option<&(LabelId, usize, usize, usize)> {
        self.labels.switch_stack.last()
    }

    /// 进入/离开一个 try/finally 域：break/continue 跨越 finally 计数用。
    pub(crate) fn push_finally_domain(&mut self) {
        self.labels.finally_depth += 1;
    }

    pub(crate) fn pop_finally_domain(&mut self) {
        self.labels.finally_depth -= 1;
    }

    /// 记录一个纯 catch handler 打开（对应 emit try_begin 后的运行时 TRY_BEGIN）。
    pub(crate) fn push_open_catch_handler(&mut self) {
        self.open_try_handlers.push(true);
    }

    /// 记录一个 finally handler 打开（对应 emit try_finally_begin 后的运行时
    /// TRY_FINALLY_BEGIN）。
    pub(crate) fn push_open_finally_handler(&mut self) {
        self.open_try_handlers.push(false);
    }

    /// 弹出最近打开的 handler（对应 emit 的 TRY_END / TRY_FINALLY_END）。
    pub(crate) fn pop_open_try_handler(&mut self) {
        self.open_try_handlers.pop();
    }

    /// 栈顶连续打开的纯 catch handler 数（遇 finally handler 即停）。
    /// return 逃出本函数时，这些 handler 可由 TRY_END 直接从栈顶弹出；
    /// finally 之下的 catch 无法经 TRY_END 弹出，交给运行时统一清理。
    pub(crate) fn top_open_catch_handlers(&self) -> usize {
        self.open_try_handlers.iter().rev().take_while(|&&is_catch| is_catch).count()
    }

    pub(crate) fn push_label_scope(
        &mut self, name: &str, break_label: LabelId, continue_label: Option<LabelId>,
    ) -> Result<(), String> {
        if self.labels.label_scopes.iter().any(|s| s.name == name) {
            return Err(format!("SyntaxError: Label '{name}' has already been declared"));
        }
        let fd = self.labels.finally_depth;
        let fod = self.labels.for_of_depth;
        let fid = self.labels.for_in_depth;
        self.labels.label_scopes.push(LabelScope {
            name: name.to_string(),
            break_label,
            continue_label,
            finally_depth_at_open: fd,
            for_of_depth_at_open: fod,
            for_in_depth_at_open: fid,
        });
        Ok(())
    }

    pub(crate) fn pop_label_scope(&mut self) {
        self.labels.label_scopes.pop();
    }

    pub(crate) fn find_label(&self, name: &str) -> Option<&LabelScope> {
        self.labels.label_scopes.iter().rev().find(|s| s.name == name)
    }

    /// 登记一个待绑定标签名：该标签将作为下一个 emit 的循环（标签语句体）的
    /// continue 目标。活动集合与待绑定集合中出现重名报错。
    pub(crate) fn queue_loop_label(&mut self, name: &str) -> Result<(), String> {
        if self.labels.label_scopes.iter().any(|s| s.name == name)
            || self.labels.pending_loop_labels.iter().any(|n| n == name)
        {
            return Err(format!("SyntaxError: Label '{name}' has already been declared"));
        }
        self.labels.pending_loop_labels.push(name.to_string());
        Ok(())
    }

    /// 把待绑定标签名落地为活动标签作用域，绑定到本次循环的 break/continue 目标。
    /// 返回压入的作用域个数（供事后对称弹出）。
    pub(crate) fn take_pending_loop_labels(&mut self, break_label: LabelId, continue_label: LabelId) -> usize {
        let names = std::mem::take(&mut self.labels.pending_loop_labels);
        let count = names.len();
        let fd = self.labels.finally_depth;
        let fod = self.labels.for_of_depth;
        let fid = self.labels.for_in_depth;
        for name in names {
            self.labels.label_scopes.push(LabelScope {
                name,
                break_label,
                continue_label: Some(continue_label),
                finally_depth_at_open: fd,
                for_of_depth_at_open: fod,
                for_in_depth_at_open: fid,
            });
        }
        count
    }

    pub(crate) fn pop_label_scopes(&mut self, n: usize) {
        for _ in 0..n {
            self.labels.label_scopes.pop();
        }
    }

    pub(crate) fn is_builtin(&self, name: &str) -> bool {
        self.scopes.builtin_reg_map.iter().any(|(n, _)| n == name)
    }

    /// 内置名是否被局部声明遮蔽（`var parseInt = ...` 等）。
    /// builtin_reg_map 记录的是全局内置槽；若该名在静态作用域另有绑定，则调用应解析局部。
    pub(crate) fn is_local_shadowing_builtin(&self, name: &str) -> bool {
        if let Some(reg) = self.scopes.symbols.lookup_any(name) {
            let is_builtin_slot = self.scopes.builtin_reg_map.iter().any(|(n, r)| n == name && *r == reg);
            return !is_builtin_slot;
        }
        false
    }

    /// 名字是否解析到全局内置槽：builtin_reg_map 登记（预注册镜像槽），或绑定
    /// 落全局作用域（scope 0，顶层 var 预声明/隐式全局，含继承的全局槽）。局部
    /// var/let/参数遮蔽同名时解析到局部寄存器/局部作用域，两条件均不命中。
    fn resolves_to_global_builtin_slot(&self, name: &str, reg: u32) -> bool {
        if self.scopes.builtin_reg_map.iter().any(|(n, r)| n == name && *r == reg) {
            return true;
        }
        matches!(self.scopes.symbols.lookup_any_binding(name), Some((_, 0)))
    }

    /// 写目标是否为只读全局内置（undefined/NaN/Infinity 的全局绑定——全局对象上
    /// 数据属性 {writable:false, configurable:false}，put 永不成功）。名字在只读
    /// 名单内且解析到全局内置槽；声明臂与赋值臂共用本谓词。
    pub(crate) fn targets_readonly_builtin(&self, name: &str, reg: u32) -> bool {
        NON_WRITABLE_GLOBAL_BUILTINS.contains(&name) && self.resolves_to_global_builtin_slot(name, reg)
    }

    /// 写目标是否为可写全局内置（全局对象上数据属性 {writable:true,
    /// configurable:true}）。标识符写须双写镜像槽 + 全局对象属性，否则裸读
    /// （镜像）与 globalThis 反射（属性）失步。
    pub(crate) fn targets_writable_builtin(&self, name: &str, reg: u32) -> bool {
        Self::is_writable_builtin_global(name) && self.resolves_to_global_builtin_slot(name, reg)
    }

    /// 规范可写全局名：BUILTIN_GLOBALS 减只读三常量集——11 个 w:true 函数、
    /// 48 个构造器/命名空间、globalThis 与宿主名，描述符皆
    /// {writable:true, enumerable:false, configurable:true}。
    pub(crate) fn is_writable_builtin_global(name: &str) -> bool {
        BUILTIN_GLOBALS.contains(&name) && !NON_WRITABLE_GLOBAL_BUILTINS.contains(&name)
    }

    /// 可删除全局内置名：可写全局名再除宿主名 $262（harness 全局
    /// {configurable:false}，删除会破坏宿主面）——自只读/可写单一真源派生，
    /// 不另立名单。余名描述符皆 {writable:true, configurable:true}，delete
    /// 真删且返 true。
    pub(crate) fn is_deletable_global_builtin(name: &str) -> bool {
        Self::is_writable_builtin_global(name) && name != "$262"
    }

    /// delete 标识符是否解析到可删除的全局内置镜像槽：名可删（上谓词）且预注册
    /// 镜像槽在册时返回槽寄存器。局部 var/let/参数遮蔽同名时该名不注册镜像槽
    /// （预扫描解析到局部绑定），不命中；槽值在 run/帧入口由全局属性预载。
    pub(crate) fn global_builtin_delete_slot(&self, name: &str) -> Option<u32> {
        if !Self::is_deletable_global_builtin(name) {
            return None;
        }
        self.scopes.builtin_reg_map.iter().find(|(n, _)| n == name).map(|(_, r)| *r)
    }

    pub(crate) fn is_known_builtin(name: &str) -> bool {
        BUILTIN_GLOBALS.contains(&name)
    }

    /// 规范不可声明的全局名：只读三常量（全局对象上数据属性
    /// {writable:false, configurable:false}）——顶层块级函数名仅这一面不建
    /// 外层绑定、不求值写回（sloppy put 永不成功，跳过与执行等价）；其余
    /// 已知 builtin（parseInt 等）属性可配置，可声明（求值期覆写全局属性）。
    pub(crate) fn is_non_writable_global_builtin(name: &str) -> bool {
        NON_WRITABLE_GLOBAL_BUILTINS.contains(&name)
    }

    /// 进入 with 语句体：记录对象寄存器与当前作用域深度，供动态标识符解析。
    pub(crate) fn push_with(&mut self, obj_reg: u32) {
        let depth = self.scopes.symbols.scopes.len();
        self.with_stack.push((obj_reg, depth));
    }

    /// 离开 with 语句体。
    pub(crate) fn pop_with(&mut self) {
        self.with_stack.pop();
    }

    /// 最内层 with 对象寄存器；无 with 时为 None。
    pub(crate) fn innermost_with_obj(&self) -> Option<u32> {
        self.with_stack.last().map(|&(reg, _)| reg)
    }

    /// 指定名字是否在 with 语句体**内**（with 打开之后的块作用域）声明。
    /// with 内声明优先于对象属性；with 之前的外层声明被对象遮蔽。
    pub(crate) fn is_with_internal_binding(&self, name: &str) -> bool {
        let Some(&(_, depth)) = self.with_stack.last() else {
            return false;
        };
        matches!(self.scopes.symbols.lookup_any_binding(name), Some((_, idx)) if idx >= depth)
    }

    pub(crate) fn builtin_reg_floor(&self) -> u32 {
        self.scopes
            .builtin_reg_map
            .iter()
            .map(|(_, reg)| reg.saturating_add(1))
            .max()
            .unwrap_or(0)
    }

    /// 组装 IRFunction（两出口共用），take 走编译产物状态。
    /// `parent_ctx` 用于补全 upvalue_captures 的 enclosing_reg（父符号表在父 emit 完成后完整）。
    pub(crate) fn assemble_ir(
        &mut self, param_layout: oxide_ir::ParamLayout, parent_ctx: Option<&CompileCtx>,
    ) -> IRFunction {
        let upvalue_captures = self
            .current_upvalue_captures
            .iter()
            .map(|u| {
                let enclosing_reg = parent_ctx
                    .and_then(|p| p.scopes.symbols.lookup_any(u.name.as_str()))
                    .unwrap_or(0);
                UpvalueCapture {
                    name: u.name.clone(),
                    enclosing_reg,
                    cell_idx: u.cell_idx,
                    parent_uv_idx: u.parent_uv_idx,
                }
            })
            .collect();
        IRFunction {
            insts: std::mem::take(&mut self.insts),
            label_pos: std::mem::take(&mut self.labels.label_pos),
            label_count: self.labels.label_counter,
            constants: std::mem::take(&mut self.constants),
            param_layout,
            builtin_reg_map: std::mem::take(&mut self.scopes.builtin_reg_map),
            upvalue_captures,
            // 容量按名字集大小取：别名捕获合并会让多个名字共享同一 cell，
            // 下标可重复但不超过名字数，故 len 恒覆盖实际使用的最大下标。
            cells_needed: self.captured_bindings.len() as u8,
            n_registers: self.max_regs,
            is_arrow: false,
            is_class_constructor: false,
            is_derived_constructor: false,
            needs_home_object: false,
            is_generator: self.is_generator,
            is_async: self.is_async,
            is_strict: self.is_strict,
            captured_this_const_idx: 0,
            function_name: None,
            function_length: self.function_length,
            is_top_level: parent_ctx.is_none(),
            const_overflow: self.const_overflow,
            nested: std::mem::take(&mut self.nested),
        }
    }
}

impl ConstantKey {
    fn from_constant(value: &Constant) -> Option<Self> {
        match value {
            Constant::Number(v) => Some(Self::Number(v.to_bits())),
            Constant::Int(v) => Some(Self::Int(*v)),
            Constant::BigInt(v) => Some(Self::BigInt(v.clone())),
            Constant::String(v) => Some(Self::String(v.clone())),
            Constant::Boolean(v) => Some(Self::Boolean(*v)),
            Constant::Null => Some(Self::Null),
            Constant::Undefined => Some(Self::Undefined),
        }
    }
}
