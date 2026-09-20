//! 顶层程序发射：`emit_program`（全局声明实例化与 sub-pass 调度）与全局写入族。
//!
//! 关键不变式：
//! - `BUILTIN_GLOBALS` / `NON_WRITABLE_GLOBAL_BUILTINS` 是全 crate 的内置名集
//!   单一真值，`compile_ctx` 各编译期谓词经此二表引用；
//! - GDI 序言名集 = 顶层 var 名 ∪ 块级函数泄漏名；顶层函数声明名不进序言
//!   （其 A 侧值由首 sub-pass 的声明写以真闭包建立）；
//! - `Emitter` 结构体留 `emit.rs`，本文件以跨文件 `impl` 块扩展之。

use std::collections::HashSet;

use oxide_bytecode::module::Constant;
use oxide_bytecode::opcode::OpCode;
use oxide_ir::inst::Inst;
use oxide_ir::operand::Operand;
use oxide_ir::IRFunction;
use oxide_parser::Statement;

use crate::capture::{
    collect_captured_bindings, collect_direct_lexical_names, collect_own_binding_names,
    collect_top_level_function_names, collect_top_level_function_names_ordered, collect_var_binding_names,
};
use crate::compile_ctx::CompileCtx;
use crate::Emitter;

pub(crate) const BUILTIN_GLOBALS: &[&str] = &[
    "NaN",
    "undefined",
    "Infinity",
    "globalThis",
    "Object",
    "Array",
    "String",
    "Number",
    "Boolean",
    "Function",
    "Error",
    "TypeError",
    "ReferenceError",
    "RangeError",
    "SyntaxError",
    "URIError",
    "EvalError",
    "eval",
    "SuppressedError",
    "DisposableStack",
    "AsyncDisposableStack",
    "Math",
    "JSON",
    "Promise",
    "AggregateError",
    "Date",
    "Set",
    "Map",
    "RegExp",
    "Symbol",
    "parseInt",
    "parseFloat",
    "isNaN",
    "isFinite",
    "Proxy",
    "WeakMap",
    "WeakSet",
    "WeakRef",
    "FinalizationRegistry",
    "Atomics",
    "SharedArrayBuffer",
    "ArrayBuffer",
    "DataView",
    "Iterator",
    "BigInt",
    "TypedArray",
    "Int8Array",
    "Uint8Array",
    "Uint8ClampedArray",
    "Int16Array",
    "Uint16Array",
    "Int32Array",
    "Uint32Array",
    "Float32Array",
    "Float64Array",
    "BigInt64Array",
    "BigUint64Array",
    "Reflect",
    "escape",
    "unescape",
    "encodeURI",
    "decodeURI",
    "encodeURIComponent",
    "decodeURIComponent",
    // test262 宿主对象：编译期按已知全局解析，运行期由 VM 绑定（详见 bind_test262_host）。
    "$262",
];

/// 只读全局内置名：全局对象上的数据属性为 {writable:false, configurable:false}，
/// 对它们的 put 永不成功。写路径命中这些名字的全局绑定（非局部遮蔽）时编译期
/// 拦截：sloppy 静默丢弃、strict 抛 TypeError；BUILTIN_GLOBALS 内其余名属性可写
/// （writable:true），标识符写须双写全局属性（见 targets_writable_builtin）。
pub(crate) const NON_WRITABLE_GLOBAL_BUILTINS: &[&str] = &["undefined", "NaN", "Infinity"];

impl Emitter {
    /// 隐式全局写（sloppy 未声明标识符写）：把值寄存器写到全局对象可写/可枚举/
    /// 可配置数据属性（未解析引用上的 PutValue 语义）。全局对象由 VM 运行期从
    /// session 解析，不依赖 this。
    ///
    /// # 边界与前提
    /// - 仅读写登记集并集命中的寄存器调用（未声明标识符写）：未声明名谁先引用谁
    ///   登记全局槽，读侧与写侧登记同属一个全局槽，写一律须穿透到全局对象。
    ///
    /// # 副作用
    /// - 定义全局对象数据属性（可写/可枚举/可配置），属性缺失时新建。
    pub(crate) fn emit_implicit_global_write(&self, name: &str, val_reg: u32, ctx: &mut CompileCtx) {
        let idx = ctx.add_constant(Constant::String(name.to_string()));
        ctx.inst(Inst::define_global_prop_c(Operand::Reg(val_reg), idx));
    }

    /// 严格模式未声明写：发射 ReferenceError 抛错指令序列（未解析引用不可 put，
    /// 值无关，编译期拦截）。错误消息格式与读侧 LOAD_GLOBAL 运行期消息一致。
    ///
    /// # 副作用
    /// - 发射 THROW 指令序列，其后控制流不可达，dummy 值保持寄存器良定义。
    pub(crate) fn emit_strict_undeclared_write(&self, name: &str, ctx: &mut CompileCtx) -> Result<u32, String> {
        self.emit_throw_error("ReferenceError", &format!("{name} is not defined"), ctx)
    }

    /// 把脚本顶层 var/function 绑定的当前值同步写入全局对象属性，使顶层声明
    /// 可经 `globalThis` 反射（脚本环境记录的 var 绑定全局对象属性）。
    ///
    /// # 边界与前提
    /// - 仅顶层模块上下文调用。
    /// - let/const/class 不落全局对象，不得调用本函数。
    ///
    /// # 副作用
    /// - 普通脚本：定义可写/可枚举/不可配置数据属性（经顶层 `this` = 全局对象）。
    /// - eval 脚本（`is_eval_script`）：属性可配置（configurable:true），全局对象
    ///   经 session 解析——eval var 声明允许后续 redefine/delete。
    pub(crate) fn emit_global_prop_write(&self, name: &str, val_reg: u32, ctx: &mut CompileCtx) {
        let idx = ctx.add_constant(Constant::String(name.to_string()));
        if ctx.is_eval_script {
            ctx.inst(Inst::define_global_prop_c(Operand::Reg(val_reg), idx));
        } else {
            let key_reg = ctx.alloc_reg();
            ctx.inst(Inst::load_const(Operand::Reg(key_reg), idx));
            ctx.inst(Inst::define_global_prop(Operand::This, Operand::Reg(val_reg), Operand::Reg(key_reg)));
        }
    }

    /// 顶层函数声明的全局对象属性同步写：eval 脚本沿用
    /// [`emit_global_prop_write`] 的 `DEFINE_GLOBAL_PROP_C` 分支（属性可配置，
    /// 创建检查不适用于 eval 动态路径）；普通脚本经顶层 This 走
    /// `DEFINE_GLOBAL_FUNC_BIND`（规范 CreateGlobalFunctionBinding 三臂，声明检查
    /// 由 GlobalDeclarationInstantiation（脚本顶层声明实例化）序言的
    /// `CAN_DECLARE_GLOBAL_FUNC` 段预先完成）。
    pub(crate) fn emit_global_func_bind_write(&self, name: &str, val_reg: u32, ctx: &mut CompileCtx) {
        if ctx.is_eval_script {
            self.emit_global_prop_write(name, val_reg, ctx);
            return;
        }
        let idx = ctx.add_constant(Constant::String(name.to_string()));
        let key_reg = ctx.alloc_reg();
        ctx.inst(Inst::load_const(Operand::Reg(key_reg), idx));
        ctx.inst(Inst::define_global_func_bind(Operand::This, Operand::Reg(val_reg), Operand::Reg(key_reg)));
    }

    /// 顶层 var 声明带初始化的全局对象属性同步：PutValue 语义——既有不可写数据
    /// 属性 strict 抛 TypeError / sloppy 静默 no-op；可写照原描述符仅更值。脚本与
    /// eval 均经 session 解析全局对象，不依赖顶层 this。
    ///
    /// # 边界与前提
    /// - 仅顶层模块上下文调用；builtin 名不走本写点（该名由
    ///   GlobalDeclarationInstantiation（脚本顶层声明实例化）序言写点处理：属性
    ///   已存在时不改动值，仅缺失时新建）。
    ///
    /// # 副作用
    /// - 更新全局对象数据属性，失败面抛 TypeError（strict 不可写）。
    pub(crate) fn emit_global_put_write(&self, name: &str, val_reg: u32, ctx: &mut CompileCtx) {
        let idx = ctx.add_constant(Constant::String(name.to_string()));
        ctx.inst(Inst::define_global_prop_c(Operand::Reg(val_reg), idx));
    }

    /// 判定名字的裸写是否落全局对象属性。
    ///
    /// 判定条件：名字在顶层 var/函数声明名集内，且当前作用域解析落在全局作用域
    /// （scope 0）。嵌套函数内的同名局部绑定遮蔽顶层绑定时，解析命中更高作用域，
    /// 判定为局部绑定，写走既有 cell/寄存器路径，不触全局对象。
    pub(crate) fn is_global_tier_name(&self, ctx: &CompileCtx, name: &str) -> bool {
        ctx.global_tier_names.contains(name) && matches!(ctx.scopes.symbols.lookup_any_binding(name), Some((_, 0)))
    }

    /// 顶层已声明 var 的裸写落到全局对象属性（顶层 var 的唯一存储）：顶层普通
    /// 脚本经 This=全局对象走 `DEFINE_GLOBAL_PROP`；顶层 eval 与嵌套函数
    /// This≠全局对象，经 session 解析走 `DEFINE_GLOBAL_PROP_C`（configurable:true；
    /// 既有 configurable:false 描述符不升级，仅更新值）。
    pub(crate) fn emit_tier_global_write(&self, name: &str, val_reg: u32, ctx: &mut CompileCtx) {
        let idx = ctx.add_constant(Constant::String(name.to_string()));
        if ctx.is_global_scope && !ctx.is_eval_script {
            let key_reg = ctx.alloc_reg();
            ctx.inst(Inst::load_const(Operand::Reg(key_reg), idx));
            ctx.inst(Inst::define_global_prop(Operand::This, Operand::Reg(val_reg), Operand::Reg(key_reg)));
        } else {
            ctx.inst(Inst::define_global_prop_c(Operand::Reg(val_reg), idx));
        }
    }

    /// GlobalDeclarationInstantiation（脚本顶层声明实例化）序言：顶层 var 全局
    /// 属性 define-if-absent。既有属性（数据或 accessor）不改动值——
    /// CreateGlobalVarBinding 对既有数据描述符零修改（不更值）；仅属性缺失时新建。
    /// 顶层普通脚本经 This 走 `DEFINE_GLOBAL_PROP_IF_ABSENT`；eval 经
    /// KernelSession 解析全局对象走 `DEFINE_GLOBAL_PROP_C_IF_ABSENT`。
    pub(crate) fn emit_global_prop_write_if_absent(&self, name: &str, val_reg: u32, ctx: &mut CompileCtx) {
        let idx = ctx.add_constant(Constant::String(name.to_string()));
        if ctx.is_eval_script {
            ctx.inst(Inst::define_global_prop_c_if_absent(Operand::Reg(val_reg), idx));
        } else {
            let key_reg = ctx.alloc_reg();
            ctx.inst(Inst::load_const(Operand::Reg(key_reg), idx));
            ctx.inst(Inst::define_global_prop_if_absent(
                Operand::This,
                Operand::Reg(val_reg),
                Operand::Reg(key_reg),
            ));
        }
    }

    /// 规范 GlobalDeclarationInstantiation step 9（函数臂）的检查阶段：顶层函数
    /// 声明名按声明逆序逐名去重，每名发一条 `CAN_DECLARE_GLOBAL_FUNC`
    /// （CanDeclareGlobalFunction 运行期判定，撞既有不可配置非可写数据属性或
    /// 不可扩展上的缺失名抛 TypeError）。发射位必须位于 var 序言之先——
    /// 检查失败时任何绑定（含 var 序言新建属性）不得实例化。
    ///
    /// # 边界与前提
    /// - 仅普通脚本调用（`is_eval_script` 面由调用点按条件判定排除）；eval 动态
    ///   路径与 eval 三常量编译期条件开关零触碰。
    /// - 仅遍历语句列表直接子级具名函数声明（生成器/异步声明同节点类型，天然
    ///   覆盖）；块内函数声明与 `export default function` 不入全局检查面。
    pub(crate) fn emit_gdi_func_decl_checks(&self, stmts: &[Statement], ctx: &mut CompileCtx) {
        let names = collect_top_level_function_names_ordered(stmts);
        let mut seen = HashSet::new();
        for name in names.iter().rev() {
            // 逆序首见即源序最后声明者（规范去重口径），重名只查一次。
            if !seen.insert(name.as_str()) {
                continue;
            }
            let idx = ctx.add_constant(Constant::String(name.clone()));
            let key_reg = ctx.alloc_reg();
            ctx.inst(Inst::load_const(Operand::Reg(key_reg), idx));
            ctx.inst(Inst::can_declare_global_func(Operand::This, Operand::Reg(key_reg)));
        }
    }

    /// 把完整程序编译为顶层模块体的 IRFunction。
    ///
    /// 调用方为 `oxide_compiler::Compiler::compile`：本函数完成 emit 半程，
    /// 随后由 `oxide_ir::lower::lower` 降为字节码。
    /// `repl_persist` 为 true 时脚本顶层 let/const 也写全局对象（REPL 跨轮次持久）；
    /// `is_eval_script` 为 true 时顶层 var/function 声明落全局属性 configurable:true。
    /// 顶层 var 的全局对象属性在求值开始前统一创建（值 undefined），声明语句
    /// 保持赋值语义——声明语句出现之前的读取（typeof/反射/自引用）即见绑定。
    pub fn emit_program(
        &self, program: &oxide_parser::Program, repl_persist: bool, is_eval_script: bool,
    ) -> Result<IRFunction, String> {
        crate::emit_debug!("emit_program: {} stmts", program.body.len());
        let mut ctx = CompileCtx::new();
        // 脚本顶层：var/function 声明需落到全局对象，let/const/class 不进全局。
        ctx.is_global_scope = true;
        ctx.repl_persist = repl_persist;
        ctx.is_eval_script = is_eval_script;
        // eval 起源随嵌套函数继承，供 delete 分类判定 eval 顶层可删名；与
        // `is_eval_script` 区分——后者只标识当前顶层程序，传播会误改全局写点。
        ctx.is_eval_origin = is_eval_script;
        ctx.source_encoded = self.source_encoded;
        // 脚本顶层严格模式由源码 "use strict" directive 决定（嵌套函数经父 ctx 继承）。
        ctx.is_strict = program.has_use_strict_directive();

        // eval 代码顶层函数声明撞不可写全局内置（三常量的全局绑定在任何符合规范的
        // 实现中均不可配置，声明实例化无法建立全局绑定）：规范在建立全局 var 绑定
        // 之前抛 TypeError（step 8 的 abrupt 先于绑定实例化），故 throw 发为程序首
        // 指令，其后预声明/序言/声明发射均不可达（寄存器保持良定义，运行期零开销）。
        // 严格 eval 代码函数声明绑定 eval 自身 lexical 环境、不触全局、不抛，门禁
        // 随 !is_strict 关闭，其写点抑制在声明发射处。
        if ctx.is_eval_script && !ctx.is_strict {
            for name in collect_top_level_function_names_ordered(&program.body) {
                if NON_WRITABLE_GLOBAL_BUILTINS.contains(&name.as_str()) {
                    let _ = self.emit_throw_error(
                        "TypeError",
                        &format!("Cannot declare function '{name}': global property is not configurable"),
                        &mut ctx,
                    )?;
                    break;
                }
            }
        }

        // GDI step 9 函数臂检查阶段（运行期，先于任何绑定实例化）：普通脚本
        // 顶层函数声明撞既有不可配置全局属性抛 TypeError，sloppy/strict 同形。
        // eval 面零触碰：静态三常量面由上方 eval 三常量编译期门禁拦，动态臂另归口。
        if !ctx.is_eval_script {
            self.emit_gdi_func_decl_checks(&program.body, &mut ctx);
        }

        self.predeclare_function_declarations(&program.body, &mut ctx);

        // 预注册 builtin 引用（先于任何临时寄存器），builtin 槽不进入临时寄存器池。
        // 顶层词法声明名（let/const/class）排除登记：词法绑定遮蔽同名全局属性，
        // 镜像占位先建会与词法预声明撞 scope 0 致合法遮蔽形被编译期误拒。
        let lexical_excluded = collect_direct_lexical_names(&program.body);
        self.pre_register_builtin_references(&program.body, &lexical_excluded, &mut ctx);

        // 预声明顶层 `var` 名，使首个 sub-pass 中提升的函数声明能解析外层 var。
        self.predeclare_var_declarations(&program.body, &mut ctx);

        // 预声明顶层 `let`/`const`/`class`（未初始化 TDZ 占位）。受限全局名检查
        // 仅对脚本代码启用：脚本声明实例化查全局对象受限自有属性名，eval 代码
        // 声明实例化不查——门控随 is_eval_script 而非作用域标志（嵌套块内
        // is_global_scope 仍为 true，不能作门控）。
        let global_lexical = !ctx.is_eval_script;
        self.predeclare_lexical_declarations(&program.body, &mut ctx, global_lexical)?;

        // 顶层块级函数名按 sloppy 模式下与浏览器/web 实现惯例兼容的行为处理
        // （块级函数声明建外层 var 绑定并求值写回；sloppy eval 代码同走
        // Annex B.3.3.3，其名并入 eval 的 var 环境）：实例化 var 绑定
        // （新建 var 槽；顶层只读三常量名不可声明不建）、并入顶层 var 名集
        // （裸读/写路由全局对象属性）、GlobalDeclarationInstantiation 序言建属性
        // （define-if-absent，既有属性不改动值）。strict 代码不建。
        let mut block_fn_names: Vec<String> = Vec::new();
        if !ctx.is_strict {
            ctx.block_fn_suppressed = self.collect_block_fn_suppressed_names(&program.body, &ctx.param_names);
            block_fn_names = self.collect_block_function_names(&program.body);
            for name in &block_fn_names {
                if !ctx.block_fn_suppressed.contains(name) && !CompileCtx::is_non_writable_global_builtin(name) {
                    self.predeclare_var_name(name, &mut ctx);
                }
            }
        }

        // 闭包捕获分析（AST 级，emit 前确定）
        ctx.own_bindings = collect_own_binding_names(&[], &program.body);
        // 顶层已声明名（全局对象属性为唯一存储）：裸读走全局对象属性、裸写走感知
        // 全局对象属性描述符的写，不落引擎侧镜像副本——从捕获集剔除，使嵌套函数经
        // 继承 scope 0 直连全局。
        // 集 = 顶层 var 名 ∪ 顶层函数声明名：函数值是编译闭包，编译期不可得，
        // 其全局对象属性值由首个 sub-pass（`emit_program` 顶层的内部阶段划分：先
        // 声明实例化、后用户代码发射）以真闭包建立，先于任何用户代码。
        // 仅在此顶层调用点过滤：嵌套函数的局部同名遮蔽是独立绑定，其调用点不过滤。
        let var_names = collect_var_binding_names(&program.body);
        let mut tier_names = var_names.clone();
        tier_names.extend(collect_top_level_function_names(&program.body));
        // 块级函数泄漏名并入：求值期写回与块入口头写同走全局对象属性（全局对象
        // 属性为唯一存储）。
        for name in &block_fn_names {
            if !ctx.block_fn_suppressed.contains(name) && !CompileCtx::is_non_writable_global_builtin(name) {
                tier_names.insert(name.clone());
            }
        }
        ctx.global_tier_names = tier_names;
        ctx.captured_bindings = collect_captured_bindings(&program.body, &[], &ctx.own_bindings);
        ctx.captured_bindings.retain(|n, _| !ctx.global_tier_names.contains(n));

        // 顶层 var 入口实例化：被捕获的 var 名统一 MAKE_CELL(undefined)，使 var
        // 声明语句执行前创建的闭包读取到 undefined（脚本 GlobalDeclarationInstantiation
        // 语义），而非占位 cell 的 TDZ 误报。声明语句的 MAKE_CELL 覆盖此初值。
        // 名集保持 var-only：函数声明名从不入捕获集（tier 剔除），无需入口 cell。
        let mut var_names: Vec<String> = var_names
            .into_iter()
            .filter(|n| ctx.captured_bindings.contains_key(n))
            .collect();
        // HashSet 迭代序带随机种子，排序后入口 MAKE_CELL 发射序跨进程稳定。
        var_names.sort();
        if !var_names.is_empty() {
            let undef_reg = self.emit_undefined(&mut ctx);
            for name in var_names {
                if let Some(&cell_idx) = ctx.captured_bindings.get(&name) {
                    ctx.inst(Inst::new(
                        OpCode::MAKE_CELL,
                        Operand::Reg(undef_reg),
                        Operand::Imm(cell_idx as u16),
                        Operand::None,
                    ));
                }
            }
        }

        // 被捕获顶层词法（let/const/class）入口 TDZ 占位 cell：声明语句执行前
        // 创建的闭包指向此未初始化 cell，声明前读写经运行时抛真 TDZ；声明语句
        // 的 MAKE_CELL 按占位更新语义原位翻转为已初始化。名集取捕获集交集
        // （直接子级词法名，块级名不提升到顶层），排序保证发射序跨进程稳定。
        let mut lex_names: Vec<String> = collect_direct_lexical_names(&program.body)
            .into_iter()
            .filter(|n| ctx.captured_bindings.contains_key(n))
            .collect();
        lex_names.sort();
        if !lex_names.is_empty() {
            let undef_reg = self.emit_undefined(&mut ctx);
            for name in lex_names {
                if let Some(&cell_idx) = ctx.captured_bindings.get(&name) {
                    // 未初始化标志折入 16 位立即数高字节（0x0100），dispatch 侧
                    // 按字节拆回两字段。
                    ctx.inst(Inst::new(
                        OpCode::MAKE_CELL,
                        Operand::Reg(undef_reg),
                        Operand::Imm(cell_idx as u16 | 0x0100),
                        Operand::None,
                    ));
                }
            }
        }

        // 全局声明实例化序言：脚本求值前为顶层 var 名创建全局对象属性（值 undefined），
        // 使声明语句执行前的读取（typeof、反射、自引用）可经全局对象见绑定。
        // 顶层函数声明名不进序言：函数值是编译闭包，编译期不可得，其全局对象属性值
        // 由首个 sub-pass 的声明写以真闭包建立，先于任何用户代码（首个 sub-pass 仅发
        // 函数声明），无 undefined 读窗口。
        // CreateGlobalVarBinding 对既有属性不改动值：define-if-absent 只在属性缺失时
        // 新建，可写/不可写/可配置既有属性（含值）一律保留。内置名的全局属性
        // 运行期预存（session 绑定）：缺失分支写入值取内置名镜像槽（本轮求值开始前
        // 把全局属性值预载进固定寄存器槽），既有属性不改动值，值幂等保留。
        // 序言名集 = 顶层 var 名 ∪ 顶层块级函数泄漏名（sloppy 模式浏览器兼容行为的
        // 外层绑定同样在求值前实例化，同走 define-if-absent）。
        let mut gdi_var_names: Vec<String> = collect_var_binding_names(&program.body).into_iter().collect();
        gdi_var_names.extend(
            block_fn_names
                .iter()
                .filter(|n| !ctx.block_fn_suppressed.contains(*n) && !CompileCtx::is_non_writable_global_builtin(n))
                .cloned(),
        );
        // HashSet 迭代序带随机种子，排序后 GDI 序言与内置镜像槽分配序跨进程稳定。
        gdi_var_names.sort();
        if !gdi_var_names.is_empty() {
            let undef_reg = self.emit_undefined(&mut ctx);
            for name in &gdi_var_names {
                let value_reg = if CompileCtx::is_known_builtin(name) {
                    // 内置名镜像槽未预登记时（解构 pattern 名等）就地登记，本轮求值
                    // 开始前预载全局属性值。
                    ctx.lookup_or_builtin(name).unwrap_or(undef_reg)
                } else {
                    undef_reg
                };
                self.emit_global_prop_write_if_absent(name, value_reg, &mut ctx);
            }
        }

        // 首个 sub-pass：发函数声明（hoisting），保证任何代码运行前函数对象已就绪。
        for stmt in &program.body {
            if matches!(stmt, Statement::FunctionDeclaration(_)) {
                self.emit_statement(stmt, &mut ctx)?;
            }
        }

        // 第二个 sub-pass：发其余所有语句。
        let mut last_result: Option<u32> = None;
        // directive 序言是 AST 独立字段（顶格字符串字面量，不在 body）：作为语句
        // 序列的头前缀按源序先于 body 发射。完成值即字符串值本身，参与"最后非空
        // 完成值"收敛；严格模式标志由 has_use_strict_directive 独立处理，此处不涉。
        for dir in &program.directives {
            last_result = Some(self.emit_string_literal_expression(&dir.expression, &mut ctx)?);
        }
        for stmt in &program.body {
            if matches!(stmt, Statement::FunctionDeclaration(_)) {
                continue; // Already emitted above
            }
            // 空完成值语句（变量/函数等声明）不覆写此前结果：保留最后一个非空
            // 完成值，与函数体 emit_body_stmts 的收敛口径一致。
            if let Some(r) = self.emit_statement(stmt, &mut ctx)? {
                last_result = Some(r);
            }
        }
        if let Some(r) = last_result {
            ctx.inst(Inst::new(OpCode::LOAD_VAR, Operand::None, Operand::Reg(r), Operand::None));
        } else {
            let undef_idx = ctx.add_constant(Constant::Undefined);
            ctx.inst(Inst::load_const(Operand::None, undef_idx));
        }
        ctx.inst(Inst::new(OpCode::HALT, Operand::None, Operand::None, Operand::None));

        let ir = ctx.assemble_ir(
            oxide_ir::ParamLayout {
                // 顶层模块无父函数：base 恒 0。base 若误含父函数槽偏移，会把模块自身
                // 低号虚拟寄存器误判为父槽，寄存器分配的恒等约束（identity 约束）
                // 随之收缩可分配色集并增加 spill。
                base: 0,
                count: 0,
            },
            None,
        );
        Ok(ir)
    }
}

#[cfg(test)]
mod tests {
    use super::{BUILTIN_GLOBALS, NON_WRITABLE_GLOBAL_BUILTINS};
    use crate::prepass::RESTRICTED_GLOBAL_LEXICAL_NAMES;

    /// 漂移守卫：受限全局名集与 put 写拦截名单交叠名恒同步（两名单语义独立、
    /// 不互相派生，靠本断言防止改名/删名时单边漂移）。
    #[test]
    fn restricted_lexical_names_within_builtin_globals() {
        for name in RESTRICTED_GLOBAL_LEXICAL_NAMES {
            assert!(BUILTIN_GLOBALS.contains(name), "受限全局名缺失于 builtin 名单：{name}");
        }
    }

    /// 漂移守卫：只读名单是 builtin 母集子集，且只读/可写两谓词对母集构成
    /// 划分（不重不漏）——防三常量集或母集改名/删名时拦截面与双写面单边漂移。
    #[test]
    fn readonly_and_writable_partition_builtin_globals() {
        for name in NON_WRITABLE_GLOBAL_BUILTINS {
            assert!(BUILTIN_GLOBALS.contains(name), "只读全局名缺失于 builtin 名单：{name}");
        }
        for name in BUILTIN_GLOBALS {
            let readonly = NON_WRITABLE_GLOBAL_BUILTINS.contains(name);
            let writable = crate::CompileCtx::is_writable_builtin_global(name);
            assert!(readonly ^ writable, "builtin 名只读/可写归属漂移：{name}");
        }
    }

    /// 漂移守卫：可删名集自可写划分派生（可删 ⊆ 可写、只读三常量不入可删），
    /// 且宿主名 $262 恒不可删——防派生口径改动时 delete 面单边漂移。
    #[test]
    fn deletable_global_builtins_derive_from_writable_partition() {
        for name in BUILTIN_GLOBALS {
            let deletable = crate::CompileCtx::is_deletable_global_builtin(name);
            let writable = crate::CompileCtx::is_writable_builtin_global(name);
            assert!(!deletable || writable, "可删名缺失于可写划分：{name}");
            if NON_WRITABLE_GLOBAL_BUILTINS.contains(name) {
                assert!(!deletable, "只读三常量名误入可删集：{name}");
            }
        }
        assert!(!crate::CompileCtx::is_deletable_global_builtin("$262"), "不可删名误入可删集：$262");
    }
}
