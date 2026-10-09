//! `CompileCtx` 中心状态族：单函数编译上下文与其常量池键、类字段缓冲。
//!
//! 执行流字段（insts/registers/constants/labels）平铺在 `CompileCtx` 上，标识符
//! 绑定与闭包捕获分别下沉到 `SymbolTable` / `captured_bindings`。`assemble_ir`
//! 组装产出分域组合的 `IRFunction`，由 `oxide_ir::lower` 降为 bytecode。

use std::collections::{BTreeMap, HashMap, HashSet};

use oxide_bytecode::module::{Constant, UpvalueCapture};
use oxide_bytecode::opcode::OpCode;
use oxide_ir::inst::Inst;
use oxide_ir::operand::{LabelId, Operand};
use oxide_ir::IRFunction;

use crate::emit_ctx::{
    CompletionCarry, CompletionFrame, LabelCtx, LabelScope, LoopEntry, LoopKind, ScopeCtx, SwitchEntry,
};
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
    /// 下一可用 cell 索引（计数器）：捕获分析后初始化为名字集大小（函数级索引
    /// 0..n-1 占满），块级遮蔽绑定预声明追加、catch 参数就地声明与合成 cell 经
    /// `alloc_cell_idx` 递增取号，保证与函数级名字排序索引及历次追加互不碰撞。
    pub(crate) next_cell_idx: u8,
    /// 词法循环头（C-for / for-in / for-of）名集：头名索引由 ForHeadEnv 覆盖
    /// 捕获映射分配（TDZ 占位与体区 fresh cell），声明点据此走映射回退而非
    /// 就地追加新索引。begin 时并入、restore 时移除，嵌套循环成对。
    pub(crate) for_head_env_names: HashSet<String>,
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
    /// 本函数从父函数捕获的函数名不可写绑定名：子 ctx 不继承父函数作用域符号表，
    /// 捕获不可写信息随 upvalue 收集一并快照，供写路径 non_writable 守卫（编译期拦截）使用。
    pub(crate) upvalue_non_writable_flags: HashSet<String>,
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
    /// 形参列表是否 simple（全部无初始值标识符且无 rest）：arguments 对象
    /// callee 形态（数据属性 vs 受限访问器）按 strict ‖ !simple 分流。
    pub(crate) has_simple_params: bool,
    pub(crate) const_overflow: bool,
    /// with 语句作用域栈：元素为 (with 对象寄存器, 打开时的作用域深度)。
    /// 非空时 with 体内的自由标识符需动态解析（先查对象属性，回退外层）。
    pub(crate) with_stack: Vec<(u32, usize)>,
    /// 打开中（尚未 emit 对应 END）的 try handler 栈，自底向上镜像运行时
    /// try_stack 组成。每项标记是否为纯 catch handler（TRY_BEGIN，由 TRY_END
    /// 弹出）：return 逃出 try 域时据此弹出栈顶连续纯 catch，防 handler 泄漏。
    pub(crate) open_try_handlers: Vec<bool>,
    /// 完成值帧栈（见 `CompletionFrame`）：每个活动语句列表独立累积、if 支臂 /
    /// with 体压边界帧、循环 / switch / 非迭代标签压出口目标帧；break/continue
    /// 发射跳转前据此解析携值。压弹须在所有非 Err 路径配对（Err 直接终止本次
    /// 编译、ctx 丢弃，不做 RAII）。
    pub(crate) completion_frames: Vec<CompletionFrame>,
    /// 循环 update 段中应走寄存器（而非 cell）的被捕获绑定名：C 风格 for 的
    /// let/const 循环变量每迭代新分配一个 cell，update 写寄存器（不污染本迭代
    /// 闭包捕获的 cell），下一迭代把这个寄存器值拷入新分配的 cell。
    pub(crate) register_update_names: Vec<String>,
    /// 正在发射的 C 风格 for 头词法绑定名（含解构叶）：这些头名在 init 中经覆盖
    /// cell 建 `MAKE_CELL`，但循环体/test/update 读的是绑定寄存器、每迭代 fresh
    /// 拷贝的源也是寄存器，故 `emit_bind_target` 对集合内名字在 cell 写之外补写
    /// 绑定寄存器。init 发射完毕后清空。
    pub(crate) for_head_store_registers: HashSet<String>,
    /// 词法循环头保留集：C-for 填被闭包引用或与顶层 var/函数同名的头名，
    /// for-in/for-of 填全头名（无撤出覆盖）。`emit_bind_target` 头名回填据此
    /// 门控，非保留头名不置位绑定字段（否则体区读命中陈旧 cell 致死循环）。
    /// C-for 在 init 发射完毕后清空；for-in/for-of 经 `restore_for_head_env`
    /// 逐头名移除。
    pub(crate) for_head_keep: HashSet<String>,
    /// 词法头声明所在作用域深度（`push_scope` 后的 `scopes.len()`）：头名回填
    /// 仅在该深度（for 语句自身作用域）的声明点执行；for 体内嵌套作用域的同名
    /// 遮蔽声明与 catch 参数不在此深度，不触发回填，防遮蔽绑定的追加索引被头名
    /// 索引覆写、声明写落入头绑定 cell。嵌套循环由 `ForHeadEnv` 保存前值并恢复。
    pub(crate) for_head_depth: Option<usize>,
    /// 模块编译上下文：当前模块命名空间对象寄存器（`__moduleObject` 返回值）。
    pub(crate) module_ns_reg: Option<u32>,
    /// 写穿反演表：本模块源绑定槽寄存器 → 引用它的导出名集合。仅自导入 ns 的
    /// live 模块填充；顶层赋值按解析后的绑定槽命中此表时同步命名空间条目。
    /// 以槽位而非名字为键，块级/catch 同名遮蔽解析到不同槽位，天然不误触发。
    /// 嵌套函数 ctx 为新建，不继承本字段，写穿只作用于模块顶层。
    pub(crate) module_local_export_regs: HashMap<u32, Vec<String>>,
    /// 已求值依赖模块的命名空间对象寄存器（按 import/export source 字符串索引）。
    pub(crate) module_dep_ns_regs: HashMap<String, u32>,
    /// defer 依赖的 deferred namespace 对象寄存器（按 import source 字符串索引）：
    /// `import defer * as ns` 绑定此对象（`__moduleDeferObject` 产物），eager 命名空间
    /// 绑定仍走 `module_dep_ns_regs`。仅含 defer 导入的 spec 登记。
    pub(crate) module_defer_ns_regs: HashMap<String, u32>,
    /// 依赖模块规范路径（按 import/export source 字符串索引）：再导出来源身份用。
    pub(crate) module_dep_paths: HashMap<String, String>,
    /// 非自导入的导入局部名 → (依赖模块规范路径, 导入名)。命名空间导入的导入名为
    /// `oxide_types::MODULE_NAMESPACE_BINDING`；再导出据此重分类为间接导出。
    pub(crate) module_import_origins: HashMap<String, (String, String)>,
    /// 依赖 source 字符串 → 依赖模块是否含可重赋导出（`compile_js_dep` 回传；
    /// Json/Text 数据模块恒 false）。导入方据此把命名/默认导入登记为活读。
    pub(crate) module_dep_reassignable: HashMap<String, bool>,
    /// 导入本地名 → (依赖命名空间对象寄存器, 导出名)：可重赋依赖的命名/默认导入
    /// 活读映射。顶层读点据此改发 `__moduleGet` 活读；仅模块顶层 ctx 填充，嵌套
    /// 函数 ctx 不继承，闭包内读退化为链接期快照。
    pub(crate) module_live_imports: HashMap<String, (u32, String)>,
    /// 本模块是否含可重赋导出。仅在依赖模块（`top_level == false`）且捕获/写穿
    /// 消费者需要时置真；入口模块恒假，保证纯导出入口 IR 零变化。
    pub(crate) module_live_dep: bool,
    /// live 命名空间激活：自导入 ns 或本模块含可重赋导出时为真。写穿反演与
    /// 导出预注册据此发射；非 live 模块编译产物逐字节不变。
    pub(crate) module_live_ns_active: bool,
    /// 自导入（import from 自身）的 source 字符串集合：绑定走别名语义，不能链接期快照。
    pub(crate) module_self_import_specs: HashSet<String>,
    /// 自导入别名：导出名 → 本地绑定槽寄存器（export 语句执行时回写绑定值）。
    /// 仅承载无法静态解析源绑定的退化占位路径（star 转发的自导入名）；同名多绑定
    /// （如 default 双绑定）经 Vec 并列回写。
    pub(crate) module_self_aliases: HashMap<String, Vec<u32>>,
    /// 自导入别名（本地名，基源绑定名）：供别名捕获后处理合并源/别名 cell。
    pub(crate) module_alias_pairs: Vec<(String, String)>,
    /// 标签模板 site 计数器：本编译树内全局唯一（子 ctx 继承父值继续递增）。
    /// 运行时与模块 flat_id 组成模板对象缓存键，保证同一编译树同 site 恒返回
    /// 同一对象、不同编译树（eval 每次编译）互不共享。
    pub(crate) next_template_site: u32,
    /// 本函数登记的函数名（非抑制面）：`assemble_ir` 回写到 IR 供调试与
    /// 帧槽写入判定。抑制面（形参/var 同名）不登记名绑定，此字段仍记名
    /// 供 `instantiate_var_bindings` 排除入口 undefined 写。
    pub(crate) function_name: Option<String>,
    /// 函数名不可写绑定的寄存器（帧槽写入目标）：压帧时把 callee 函数对象
    /// 写入该槽。非抑制面在名登记时分配；抑制面（var 同名）在
    /// `instantiate_var_bindings` 回填为 var 槽寄存器。
    pub(crate) function_name_reg: Option<u32>,
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
                dispose_depth: 0,
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
            next_cell_idx: 0,
            for_head_env_names: HashSet::new(),
            pending_for_head_names: HashSet::new(),
            global_tier_names: HashSet::new(),
            upvalue_const_flags: HashSet::new(),
            upvalue_non_writable_flags: HashSet::new(),
            implicit_global_reads: HashSet::new(),
            implicit_global_writes: HashSet::new(),
            function_length: 0,
            has_simple_params: false,
            const_overflow: false,
            with_stack: Vec::new(),
            open_try_handlers: Vec::new(),
            completion_frames: Vec::new(),
            register_update_names: Vec::new(),
            for_head_store_registers: HashSet::new(),
            for_head_keep: HashSet::new(),
            for_head_depth: None,
            module_ns_reg: None,
            module_local_export_regs: HashMap::new(),
            module_dep_ns_regs: HashMap::new(),
            module_defer_ns_regs: HashMap::new(),
            module_dep_paths: HashMap::new(),
            module_import_origins: HashMap::new(),
            module_dep_reassignable: HashMap::new(),
            module_live_imports: HashMap::new(),
            module_live_dep: false,
            module_live_ns_active: false,
            module_self_import_specs: HashSet::new(),
            module_self_aliases: HashMap::new(),
            module_alias_pairs: Vec::new(),
            next_template_site: 0,
            function_name: None,
            function_name_reg: None,
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

    /// 解析点 cell 索引与绑定寄存器：名字当前可见（真实词法绑定、非隐式全局
    /// 登记）时返回 (cell_idx, binding_reg)。索引优先取绑定自身字段（函数级
    /// 回填与块级追加的真源），绑定未分配索引时回退捕获映射（for 头覆盖 /
    /// catch 参数等容器面，后续子任务逐面收编）。块级遮蔽绑定在块退出后不可
    /// 解析，返回 None（落全局解析），不得按名误读已失效 cell。tier 绑定
    /// （scope 0 顶层 var/函数）的全局对象属性是唯一存储，不建 cell；映射中
    /// 被同名块级绑定保留的 entry 不属该 tier 绑定，回退不得命中——for 头
    /// 前向名除外（头绑定未 declare，映射值是 ForHeadEnv 的 TDZ 索引）。
    pub(crate) fn visible_cell(&self, name: &str) -> Option<(u8, u32)> {
        let (binding, scope_idx) = self.scopes.symbols.lookup_any_binding(name)?;
        if self.is_implicit_global_reg(binding.reg) {
            return None;
        }
        // for 头前向名（C 风格 for 头 pending / for-in/for-of env）读 TDZ cell：
        // 头绑定未 declare，按名回查命中外层同名 tier 绑定，映射回退须放行以命中
        // ForHeadEnv 覆盖的 TDZ 索引。
        let is_for_head = self.pending_for_head_names.contains(name) || self.for_head_env_names.contains(name);
        let is_tier_binding = scope_idx == 0 && self.global_tier_names.contains(name);
        let idx = binding.cell_idx.or_else(|| {
            if is_tier_binding && !is_for_head {
                None
            } else {
                self.captured_bindings.get(name).copied()
            }
        })?;
        Some((idx, binding.reg))
    }

    /// 设置捕获集并同步 cell 索引计数器：映射值为 0..n-1 的名字排序索引，
    /// 计数器自 n 起保证后续追加（块级预声明 / 合成 cell）不与既有索引碰撞。
    pub(crate) fn set_captured_bindings(&mut self, map: BTreeMap<String, u8>) {
        self.next_cell_idx = map.len() as u8;
        self.captured_bindings = map;
    }

    /// 分配一个新 cell 索引：取当前计数器值并递增。块级遮蔽绑定预声明追加、
    /// catch 参数就地声明与合成 cell 共用，分配序由发射序决定，跨 run 稳定。
    pub(crate) fn alloc_cell_idx(&mut self) -> u8 {
        let idx = self.next_cell_idx;
        self.next_cell_idx = self.next_cell_idx.saturating_add(1);
        idx
    }

    /// 声明点 cell 索引解析：索引按绑定实例取（绑定自身字段是真源）。绑定未分配
    /// 索引时，词法循环头名（ForHeadEnv 覆盖捕获映射）走映射回退；最内层为块
    /// 作用域的就地声明（catch 参数）现场追加新索引并写回绑定，与函数级同名
    /// 绑定的名字排序索引区分。头名判定按作用域深度门控：仅 for 语句自身作用域
    /// 的声明点是头绑定本身（走映射回退）；体内嵌套作用域的同名就地声明（catch
    /// 参数）不属头绑定，走块级追加臂取独立索引，不共用头 cell。名字不可见（隐式
    /// 全局登记）或不在捕获集时返回 None，调用方落 with/全局臂。顶层 tier 名
    /// （顶层 var/函数）的全局对象属性是唯一存储，不建 cell，返回 None。
    ///
    /// # 副作用
    /// - 就地追加臂写回绑定 cell 索引并递增计数器；指令流不改动。
    pub(crate) fn resolve_bind_cell_idx(&mut self, name: &str) -> Option<u8> {
        let (existing, is_implicit, block_scope, for_head, is_tier) = {
            let symbols = &self.scopes.symbols;
            let (binding, scope_idx) = symbols.lookup_any_binding(name)?;
            (
                binding.cell_idx,
                self.is_implicit_global_reg(binding.reg),
                symbols.scopes.last().is_some_and(|s| s.kind == ScopeKind::BlockScope),
                self.for_head_env_names.contains(name) && self.for_head_depth == Some(symbols.scopes.len()),
                self.is_global_scope && self.global_tier_names.contains(name) && scope_idx == 0,
            )
        };
        if is_implicit {
            return None;
        }
        if let Some(idx) = existing {
            return Some(idx);
        }
        if is_tier {
            return None;
        }
        let base = *self.captured_bindings.get(name)?;
        if block_scope && !for_head {
            let fresh = self.alloc_cell_idx();
            self.set_binding_cell_idx(name, fresh);
            Some(fresh)
        } else {
            Some(base)
        }
    }

    /// 给最内层同名可见绑定写入 cell 索引（委托符号表）。
    pub(crate) fn set_binding_cell_idx(&mut self, name: &str, idx: u8) {
        self.scopes.symbols.set_binding_cell_idx(name, idx);
    }

    /// 函数级绑定 cell 索引回填：捕获集（名字排序分配）的索引写入对应绑定
    /// 实例，使解析点按绑定身份取索引。预声明完成后调用，函数级绑定（参数 /
    /// var / 直接子级词法 / 块级函数外层 var / arguments）此时已齐。顶层 tier
    /// 名（顶层 var/函数）的全局对象属性是唯一存储，不回填 cell 索引。
    pub(crate) fn backfill_captured_cell_idxs(&mut self) {
        let entries: Vec<(String, u8)> = self
            .captured_bindings
            .iter()
            .filter(|(name, _)| !(self.is_global_scope && self.global_tier_names.contains(*name)))
            .map(|(name, &idx)| (name.clone(), idx))
            .collect();
        for (name, idx) in entries {
            self.set_binding_cell_idx(&name, idx);
        }
    }

    pub(crate) fn lookup_const_flag(&self, name: &str) -> bool {
        // upvalue 捕获的 const：子 ctx 符号表不含父函数作用域绑定，查快照标志。
        self.scopes.symbols.lookup_is_const(name) || self.upvalue_const_flags.contains(name)
    }

    /// 写路径 non_writable 守卫查询：名字是否为函数名不可写绑定（本作用域或 upvalue 捕获）。
    pub(crate) fn lookup_non_writable_flag(&self, name: &str) -> bool {
        // upvalue 捕获的函数名：子 ctx 符号表不含父函数作用域绑定，查快照标志。
        self.scopes.symbols.lookup_non_writable(name) || self.upvalue_non_writable_flags.contains(name)
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

    /// 打开一个循环并返回其出口结果寄存器：就地分配并初始化为 undefined
    /// （覆盖零迭代路径，缺初始化则 `for(;;false;)` 形读垃圾泄漏前值）。
    /// 调用点须先于循环头标签落位：初始化指令随调用点发射，落在回边内会每
    /// 迭代重置出口值。
    pub(crate) fn push_loop(&mut self, break_label: LabelId, continue_label: LabelId, kind: LoopKind) -> u32 {
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
        let dd = self.labels.dispose_depth;
        let v_reg = self.alloc_reg();
        let undef_idx = self.add_constant(Constant::Undefined);
        self.inst(Inst::load_const(Operand::Reg(v_reg), undef_idx));
        self.labels.loop_stack.push(LoopEntry {
            break_label,
            continue_label,
            finally_depth_at_open: fd,
            for_of_depth_at_open: fod,
            for_in_depth_at_open: fid,
            dispose_depth_at_open: dd,
            kind,
            v_reg,
        });
        v_reg
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

    pub(crate) fn push_switch(&mut self, break_label: LabelId, result_reg: u32) {
        let fd = self.labels.finally_depth;
        let fod = self.labels.for_of_depth;
        let fid = self.labels.for_in_depth;
        let dd = self.labels.dispose_depth;
        self.labels.switch_stack.push(SwitchEntry {
            break_label,
            finally_depth_at_open: fd,
            for_of_depth_at_open: fod,
            for_in_depth_at_open: fid,
            dispose_depth_at_open: dd,
            result_reg,
        });
    }

    pub(crate) fn pop_switch(&mut self) {
        self.labels.switch_stack.pop();
    }

    pub(crate) fn current_switch(&self) -> Option<&SwitchEntry> {
        self.labels.switch_stack.last()
    }

    /// 压一个语句列表累积帧：列表内每条非空语句经 `set_completion_last` 记
    /// 最后非空值寄存器（空语句不覆写，规范 `UpdateEmpty` 的「非空才覆写」）。
    pub(crate) fn push_completion_list(&mut self) {
        self.completion_frames.push(CompletionFrame::List { last: None });
    }

    pub(crate) fn pop_completion_list(&mut self) {
        debug_assert!(matches!(self.completion_frames.last(), Some(CompletionFrame::List { .. })));
        self.completion_frames.pop();
    }

    /// 记栈顶列表帧的最后非空语句寄存器（调用点须刚压过列表帧）。
    pub(crate) fn set_completion_last(&mut self, reg: u32) {
        if let CompletionFrame::List { last } = self.completion_frames.last_mut().expect("完成值帧栈顶为语句列表帧")
        {
            *last = Some(reg);
        }
    }

    /// 压一个边界帧（`UpdateEmpty(_, undefined)` 站点：if 支臂 / with 体）：
    /// break/continue 携值解析撞它物化 undefined。
    pub(crate) fn push_completion_boundary(&mut self) {
        self.completion_frames.push(CompletionFrame::Boundary);
    }

    pub(crate) fn pop_completion_boundary(&mut self) {
        debug_assert!(matches!(self.completion_frames.last(), Some(CompletionFrame::Boundary)));
        self.completion_frames.pop();
    }

    /// 压一个出口目标帧：break/continue 携值写入该寄存器，空携值保持其原值。
    pub(crate) fn push_completion_target(&mut self, v_reg: u32) {
        self.completion_frames.push(CompletionFrame::Target { v_reg });
    }

    pub(crate) fn pop_completion_target(&mut self) {
        debug_assert!(matches!(self.completion_frames.last(), Some(CompletionFrame::Target { .. })));
        self.completion_frames.pop();
    }

    /// 解析 break/continue 的携值：自栈顶向目标帧走，首个非空列表帧取
    /// `Value(r)`，撞边界帧取 `Undefined`，首个目标帧——与 `target_v_reg`
    /// 相等（本跳转自身目标）取 `Empty`（不写，目标寄存器保持既有累积值，
    /// 规范 `UpdateEmpty(break, iterationResult)` 等义），为内层非目标帧
    /// （labeled break 穿内层循环）取 `Value(该帧寄存器)`。
    pub(crate) fn resolve_completion_carry(&self, target_v_reg: u32) -> CompletionCarry {
        for frame in self.completion_frames.iter().rev() {
            match frame {
                CompletionFrame::List { last: Some(r) } => return CompletionCarry::Value(*r),
                CompletionFrame::List { last: None } => {}
                CompletionFrame::Boundary => return CompletionCarry::Undefined,
                CompletionFrame::Target { v_reg } => {
                    return if *v_reg == target_v_reg {
                        CompletionCarry::Empty
                    } else {
                        CompletionCarry::Value(*v_reg)
                    };
                }
            }
        }
        CompletionCarry::Empty
    }

    /// 进入/离开一个 try/finally 域：break/continue 跨越 finally 计数用。
    pub(crate) fn push_finally_domain(&mut self) {
        self.labels.finally_depth += 1;
    }

    pub(crate) fn pop_finally_domain(&mut self) {
        self.labels.finally_depth -= 1;
    }

    /// 发 DISPOSE_MARK：作用域入口登记释放栈水位，dispose_depth 递增。
    ///
    /// # 边界与前提
    /// - 与 `emit_dispose_pop` 成对；编译期不变式是函数体收尾 dispose_depth 归零
    ///   （`assemble_ir` 前 debug_assert 校验）。
    pub(crate) fn emit_dispose_mark(&mut self) {
        self.inst(Inst::new(OpCode::DISPOSE_MARK, Operand::None, Operand::None, Operand::None));
        self.labels.dispose_depth += 1;
    }

    /// 发 DISPOSE_POP：作用域出口逆序释放水位以上资源，dispose_depth 递减。
    pub(crate) fn emit_dispose_pop(&mut self) {
        self.inst(Inst::new(OpCode::DISPOSE_POP, Operand::None, Operand::None, Operand::None));
        self.labels.dispose_depth -= 1;
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
        &mut self, name: &str, break_label: LabelId, continue_label: Option<LabelId>, completion_reg: Option<u32>,
    ) -> Result<(), String> {
        if self.labels.label_scopes.iter().any(|s| s.name == name) {
            return Err(format!("SyntaxError: Label '{name}' has already been declared"));
        }
        let fd = self.labels.finally_depth;
        let fod = self.labels.for_of_depth;
        let fid = self.labels.for_in_depth;
        let dd = self.labels.dispose_depth;
        self.labels.label_scopes.push(LabelScope {
            name: name.to_string(),
            break_label,
            continue_label,
            finally_depth_at_open: fd,
            for_of_depth_at_open: fod,
            for_in_depth_at_open: fid,
            dispose_depth_at_open: dd,
            completion_reg,
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

    /// 把待绑定标签名落地为活动标签作用域，绑定到本次循环的 break/continue 目标
    /// 与出口结果寄存器。返回压入的作用域个数（供事后对称弹出）。
    pub(crate) fn take_pending_loop_labels(
        &mut self, break_label: LabelId, continue_label: LabelId, completion_reg: u32,
    ) -> usize {
        let names = std::mem::take(&mut self.labels.pending_loop_labels);
        let count = names.len();
        let fd = self.labels.finally_depth;
        let fod = self.labels.for_of_depth;
        let fid = self.labels.for_in_depth;
        let dd = self.labels.dispose_depth;
        for name in names {
            self.labels.label_scopes.push(LabelScope {
                name,
                break_label,
                continue_label: Some(continue_label),
                finally_depth_at_open: fd,
                for_of_depth_at_open: fod,
                for_in_depth_at_open: fid,
                dispose_depth_at_open: dd,
                completion_reg: Some(completion_reg),
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
    /// scope 0 臂拒词法绑定：顶层 let/const/class/导入绑定遮蔽同名全局属性，
    /// 值存储是其自身槽位，写不落全局对象。
    fn resolves_to_global_builtin_slot(&self, name: &str, reg: u32) -> bool {
        if self.scopes.builtin_reg_map.iter().any(|(n, r)| n == name && *r == reg) {
            return true;
        }
        matches!(self.scopes.symbols.lookup_any_binding(name), Some((binding, 0)) if !binding.lexical)
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
        // 编译期不变式：每个 DISPOSE_MARK 都有配对的 DISPOSE_POP，函数体收尾
        // 释放作用域全部关闭（早 return 跳过 POP 归穿越面，不在此列）。
        debug_assert!(
            self.labels.dispose_depth == 0,
            "函数体收尾释放作用域深度须归零（mark/pop 未配对）"
        );
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
        // 容量覆盖全部已分配 cell 索引：函数级名字排序索引（映射值 0..n-1）
        // 与追加索引（计数器，块级预声明 / 合成 cell）取较大者加一；别名
        // 捕获合并让多个名字共享同一 cell，下标可重复。
        let cells_needed = self
            .captured_bindings
            .values()
            .copied()
            .max()
            .map_or(0, |m| m.saturating_add(1))
            .max(self.next_cell_idx);
        // own cell 绑定名表：下标 = cell 下标，值 = 绑定名；合成 cell 与块级
        // 预声明追加无源名，留空串。
        let mut cell_names = vec![String::new(); cells_needed as usize];
        for (name, idx) in &self.captured_bindings {
            cell_names[*idx as usize] = name.clone();
        }
        IRFunction {
            insts: std::mem::take(&mut self.insts),
            label_pos: std::mem::take(&mut self.labels.label_pos),
            label_count: self.labels.label_counter,
            constants: std::mem::take(&mut self.constants),
            param_layout,
            builtin_reg_map: std::mem::take(&mut self.scopes.builtin_reg_map),
            upvalue_captures,
            cells_needed,
            cell_names,
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
            function_name_reg: self.function_name_reg,
            function_length: self.function_length,
            has_simple_params: self.has_simple_params,
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
