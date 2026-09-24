//! 动态编译：`Function` 构造器与脚本动态编译入口，及子树 flat_id 重编号。

use std::sync::{Arc, OnceLock};

use oxide_bytecode::module::CompiledModule;
use oxide_bytecode::opcode::{self, OpCode};
use oxide_types::object::PropAttributes;
use oxide_types::value::JsValue;

use super::Vm;

impl Vm {
    /// 动态编译函数（`Function` 构造器路径）：把参数列表与函数体 wrap 成匿名函数
    /// 源码，走完整编译链后取匿名函数模块追加进 VM 平表，返回对应函数对象。
    ///
    /// # 源契约
    /// `params` 与 `body` 须为 `string_forge::source_escape` 产物（源码域转义
    /// 形态）：孤立 surrogate / FFFD 单元以 `\uXXXX` 转义文本承载，反斜杠
    /// 原样透传。
    ///
    /// # 步骤
    /// 1. wrap 源码 `{prefix} anonymous(p...) { body }`（prefix 按生成器 /
    ///    异步标志取 `function` / `function*` / `async function` /
    ///    `async function*`），parse + compile。
    /// 2. 取 `sub_modules[0]`（匿名函数模块），把其子树 flat_id 重编号到平表末尾
    ///    并重写子树内每条 `CREATE_CLOSURE` 的 imm16。
    /// 3. 同步扩容当前代际的常量缓存，建函数对象并设置 name/length。
    ///
    /// # 边界与前提
    /// - 编译或解析失败返回 `Err`（由 builtin 层转 SyntaxError）。
    /// - 追加的子树原 flat_id 自 1 连续（flatten 后 1=匿名体，2…=其嵌套函数）；
    ///   新 id = 平表长度 + (old - 1)。
    /// - 动态函数在函数对象存活期间跨 run 有效：扩展落当前代际平表，存活函数
    ///   对象按代际引用保活该表；full_reset 重建 session 后失效。
    ///
    /// # 副作用
    /// - 扩展当前代际平表（`tables[current_gen]`）与常量缓存。
    pub fn create_dynamic_function(
        &mut self, params: &[String], body: &str, is_generator: bool, is_async: bool,
    ) -> Result<JsValue, String> {
        // wrap 源码：body 两端换行防止以行注释结尾吞掉右花括号；形参串后补
        // 换行，使形参以 HTML 注释（`<!--`/`-->`）结尾时注释在行末终止、
        // 右括号不被吞（规范按形参串与体串各自独立解析，此处以换行等价）。
        // 函数形态按生成器 / 异步标志分流，编译标志（is_generator / is_async）
        // 由 emit 层从 AST 读回，函数对象原型与 prototype 属性面随之自动分流。
        let prefix = if is_async {
            if is_generator { "async function*" } else { "async function" }
        } else if is_generator {
            "function*"
        } else {
            "function"
        };
        let params_str = params.join(", ");
        let source = format!("{prefix} anonymous({params_str}\n) {{\n{body}\n}}");

        let allocator = oxide_parser::Allocator::default();
        let program = oxide_parser::parse(&allocator, &source)
            .map_err(|errs| errs.into_iter().map(|e| e.message).collect::<Vec<_>>().join("\n"))?;
        // 动态路径源契约：调用方以源码域转义（`string_forge::source_escape`）
        // 形态传源；正则字面量源切片经 `source_escape_to_key` 还原注入
        // marker 后入池（编码源口径，区别于静态源的 `pool_key_plain`）。
        let mut module = oxide_compiler::compiler::Compiler::new()
            .with_source_encoded(true)
            .compile(&program)?;
        let anonymous = module.sub_modules.remove(0);
        // 形参数以编译结果为准：单个实参 "a,b,c" 拼接后解析为 3 个形参
        // （ES 动态函数把非末位实参以逗号连接成参数串再解析）。
        let formal_count = anonymous.n_args as i32;

        // 子树重编号 + 追加：base = 当前代际平表长度，DFS 前序压入，push 序即新
        // flat_id。make_mut 彼时平表 Arc 强引用唯一持有者是本表，原地扩展不分叉。
        let table = self.current_table_mut();
        let base = table.modules.len() as u32;
        let mut added = Vec::new();
        rehome_subtree(&anonymous, base, &mut added);
        Arc::make_mut(&mut table.modules).extend(added);
        // 平表变长后同步扩容常量缓存，否则激活新模块常量时越界 panic。
        table.immutables.resize(table.modules.len(), OnceLock::new());

        let func_val = self.create_function_object(base, self.current_gen, false, false, false, false);
        let func_obj = unsafe { &mut *func_val.as_js_object_ptr() };
        let length_si = self.kernel_core.perm_interner().intern("length").0;
        let name_si = self.kernel_core.perm_interner().intern("name").0;
        // length/name 为不可写不可枚举可配置，且 length 先于 name（规范属性顺序）。
        let attrs = PropAttributes::new(false, false, true);
        let length_val = JsValue::int(formal_count);
        let name_val = self.new_string("anonymous");
        self.define_data_property(func_obj, length_si, length_val, attrs)?;
        self.define_data_property(func_obj, name_si, name_val, attrs)?;
        Ok(func_val)
    }

    /// 动态编译脚本（eval 脚本模式）：把源码按脚本模式编译，var/函数声明落全局对象
    /// （属性 configurable:true，区别于普通脚本顶层的 false）。
    ///
    /// # 步骤
    /// 1. parse（脚本模式）→ compile（emit_program 置 is_global_scope=true，
    ///    Compiler 置 is_eval_script=true）。
    /// 2. 整棵模块树（根 flat_id=0 + 嵌套函数）追加进平表：`rehome_subtree(&module, base+1)`，
    ///    使根落 base、子函数 old→base+old，CREATE_CLOSURE imm16 同步重写。
    /// 3. 扩容当前代际的常量缓存，建函数对象（sub_module_index = base）返回。
    ///
    /// # 边界与前提
    /// - 顶层 return 不报 SyntaxError（emit 无此检查，与 CLI 脚本路径一致）——已知偏差。
    /// - 动态模块在函数对象存活期间跨 run 有效（同 create_dynamic_function）。
    /// - 返回函数对象仅供内部同步调用，不设 name/length（用户不可见）。
    ///
    /// # 副作用
    /// - 扩展当前代际平表（`tables[current_gen]`）与常量缓存。
    pub fn create_dynamic_script(&mut self, code: &str) -> Result<JsValue, String> {
        let allocator = oxide_parser::Allocator::default();
        let program = oxide_parser::parse(&allocator, code)
            .map_err(|errs| errs.into_iter().map(|e| e.message).collect::<Vec<_>>().join("\n"))?;
        // eval 脚本：顶层 var/function 声明落全局属性 configurable:true。
        // 动态路径源契约同 `create_dynamic_function`（源码域转义形态传源）。
        let module = oxide_compiler::compiler::Compiler::new()
            .with_eval_script(true)
            .with_source_encoded(true)
            .compile(&program)?;
        // 根模块 flat_id=0 传 base+1，重编号后落 base（避开 sub_module_index()==0
        // 守卫）。make_mut 彼时平表 Arc 强引用唯一持有者是本表，原地扩展不分叉。
        let table = self.current_table_mut();
        let base = table.modules.len() as u32;
        let mut added = Vec::new();
        rehome_subtree(&module, base + 1, &mut added);
        Arc::make_mut(&mut table.modules).extend(added);
        // 平表变长后同步扩容常量缓存，否则激活新模块常量时越界 panic。
        table.immutables.resize(table.modules.len(), OnceLock::new());
        Ok(self.create_function_object(base, self.current_gen, false, false, false, false))
    }

    /// 动态编译脚本（普通脚本模式）：顶层 var/function 声明落全局对象
    /// configurable:false，与静态脚本顶层一致（区别于 [`create_dynamic_script`]
    /// 的 eval 脚本 configurable:true 面）。
    ///
    /// 供 test262 宿主 `$262.evalScript` 使用：按 test262 harness 规范它是
    /// 当前 realm 的一段普通 script，不是 eval。
    ///
    /// # 步骤
    /// 1. parse（脚本模式）→ compile（emit_program 置 is_global_scope=true，
    ///    不设 is_eval_script，与普通脚本同一编译臂）。
    /// 2. 整棵模块树追加进平表：同 [`create_dynamic_script`]
    ///    （`rehome_subtree(&module, base+1)`，根落 base、子函数 old→base+old）。
    /// 3. 扩容当前代际的常量缓存，建函数对象（sub_module_index = base）返回。
    ///
    /// # 边界与前提
    /// - 编译或解析失败返回 `Err`（由宿主层转 SyntaxError）。
    /// - 动态模块在函数对象存活期间跨 run 有效（同 create_dynamic_function）。
    /// - 返回函数对象仅供内部同步调用，不设 name/length（用户不可见）。
    ///
    /// # 副作用
    /// - 扩展当前代际平表（`tables[current_gen]`）与常量缓存。
    pub fn create_plain_dynamic_script(&mut self, code: &str) -> Result<JsValue, String> {
        let allocator = oxide_parser::Allocator::default();
        let program = oxide_parser::parse(&allocator, code)
            .map_err(|errs| errs.into_iter().map(|e| e.message).collect::<Vec<_>>().join("\n"))?;
        // 普通脚本：顶层 var/function 声明落全局属性 configurable:false，
        // let/const 落全局词法环境。动态路径源契约同 `create_dynamic_function`
        // （源码域转义形态传源）。
        let module = oxide_compiler::compiler::Compiler::new()
            .with_source_encoded(true)
            .compile(&program)?;
        // 根模块 flat_id=0 传 base+1，重编号后落 base（避开 sub_module_index()==0
        // 守卫）。make_mut 彼时平表 Arc 强引用唯一持有者是本表，原地扩展不分叉。
        let table = self.current_table_mut();
        let base = table.modules.len() as u32;
        let mut added = Vec::new();
        rehome_subtree(&module, base + 1, &mut added);
        Arc::make_mut(&mut table.modules).extend(added);
        // 平表变长后同步扩容常量缓存，否则激活新模块常量时越界 panic。
        table.immutables.resize(table.modules.len(), OnceLock::new());
        Ok(self.create_function_object(base, self.current_gen, false, false, false, false))
    }
}

/// 把 flatten 后的子模块子树重编号到平表偏移 `base`：DFS 前序拷贝进 `out`，
/// 新 flat_id = base + (old - 1)，子树内每条 `CREATE_CLOSURE` 的 imm16 同步重写。
/// 原子树 flat_id 自 1 连续，因此拷贝顺序即新 id 顺序，`out` 下标对齐平表槽位。
fn rehome_subtree(module: &CompiledModule, base: u32, out: &mut Vec<Arc<CompiledModule>>) {
    let new_id = base + module.flat_id - 1;
    let mut bytecode = module.bytecode.to_vec();
    for instr in &mut bytecode {
        if opcode::opcode(*instr) == OpCode::CREATE_CLOSURE {
            let old = opcode::imm16(*instr) as u32;
            let new_flat = base + (old - 1);
            *instr = opcode::encode(
                OpCode::CREATE_CLOSURE,
                opcode::rd(*instr),
                (new_flat & 0xFF) as u8,
                ((new_flat >> 8) & 0xFF) as u8,
            );
        }
    }
    let mut rehomed = module.clone();
    rehomed.bytecode = Arc::from(bytecode);
    rehomed.flat_id = new_id;
    out.push(Arc::new(rehomed));
    for sub in &module.sub_modules {
        rehome_subtree(sub, base, out);
    }
}
