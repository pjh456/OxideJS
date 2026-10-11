//! test262 harness 内嵌注册表与前缀拼接/缓存：`HarnessSources`（24 处 `include_str!`）、`HARNESS`、黑名单、
//! 前缀拼接与缓存。全部 `include_str!` 路径相对上 3 级到仓库根，依赖本文件平铺 `src/` 一级，禁子目录。

use crate::meta::TestMeta;
use oxide_bytecode::CompiledModule;
use oxide_compiler::compiler::Compiler;
use oxide_engine::Engine;
use std::collections::HashMap;
use std::sync::{Arc, OnceLock, RwLock};

/// 内嵌的 test262 harness 辅助脚本注册表（编译期 include_str! 打包）。
pub(crate) struct HarnessSources {
    sources: HashMap<&'static str, &'static str>,
}

/// harness 前缀缓存键：由测试 `includes` 列表唯一确定。
pub(crate) type HarnessPrefixCache = HashMap<Vec<String>, String>;

/// harness 前缀编译产物缓存：键为 includes 组合（含 onlyStrict / async 旗标），
/// 值为编译后的前缀模块。命中时直接复用，免每测试重解析重编译前缀。
pub(crate) type HarnessModuleCache = HashMap<Vec<String>, Arc<CompiledModule>>;

impl HarnessSources {
    /// 构建 harness 源注册表（键为文件名，值为编译期内嵌源码）。
    pub(crate) fn new() -> Self {
        let mut sources = HashMap::new();
        sources.insert("sta.js", include_str!("../../../tests/test262/harness/sta.js"));
        sources.insert("assert.js", include_str!("../../../tests/test262/harness/assert.js"));
        sources.insert("propertyHelper.js", include_str!("../../../tests/test262/harness/propertyHelper.js"));
        sources.insert("compareArray.js", include_str!("../../../tests/test262/harness/compareArray.js"));
        sources.insert("fnGlobalObject.js", include_str!("../../../tests/test262/harness/fnGlobalObject.js"));
        sources.insert("nans.js", include_str!("../../../tests/test262/harness/nans.js"));
        sources.insert("dateConstants.js", include_str!("../../../tests/test262/harness/dateConstants.js"));
        sources.insert(
            "decimalToHexString.js",
            include_str!("../../../tests/test262/harness/decimalToHexString.js"),
        );
        sources.insert("isConstructor.js", include_str!("../../../tests/test262/harness/isConstructor.js"));
        sources.insert("nativeErrors.js", include_str!("../../../tests/test262/harness/nativeErrors.js"));
        sources.insert(
            "nativeFunctionMatcher.js",
            include_str!("../../../tests/test262/harness/nativeFunctionMatcher.js"),
        );
        sources.insert("regExpUtils.js", include_str!("../../../tests/test262/harness/regExpUtils.js"));
        sources.insert(
            "assertRelativeDateMs.js",
            include_str!("../../../tests/test262/harness/assertRelativeDateMs.js"),
        );
        sources.insert(
            "wellKnownIntrinsicObjects.js",
            include_str!("../../../tests/test262/harness/wellKnownIntrinsicObjects.js"),
        );
        sources.insert("typeCoercion.js", include_str!("../../../tests/test262/harness/typeCoercion.js"));
        sources.insert("deepEqual.js", include_str!("../../../tests/test262/harness/deepEqual.js"));
        sources.insert("testTypedArray.js", include_str!("../../../tests/test262/harness/testTypedArray.js"));
        sources.insert("testAtomics.js", include_str!("../../../tests/test262/harness/testAtomics.js"));
        sources.insert("temporalHelpers.js", include_str!("../../../tests/test262/harness/temporalHelpers.js"));
        sources.insert("asyncHelpers.js", include_str!("../../../tests/test262/harness/asyncHelpers.js"));
        sources.insert("doneprintHandle.js", include_str!("../../../tests/test262/harness/doneprintHandle.js"));
        sources.insert("promiseHelper.js", include_str!("../../../tests/test262/harness/promiseHelper.js"));
        sources.insert(
            "detachArrayBuffer.js",
            include_str!("../../../tests/test262/harness/detachArrayBuffer.js"),
        );
        sources.insert(
            "resizableArrayBufferUtils.js",
            include_str!("../../../tests/test262/harness/resizableArrayBufferUtils.js"),
        );
        Self { sources }
    }

    /// 按文件名取 harness 源码。
    pub(crate) fn get(&self, name: &str) -> Option<&'static str> {
        self.sources.get(name).copied()
    }
}

pub(crate) static HARNESS: OnceLock<HarnessSources> = OnceLock::new();

/// 判断 harness 文件是否在支持范围之外（依赖 Proxy/Intl/async 等未实现特性）。
fn is_blacklisted_harness(name: &str) -> bool {
    matches!(
        name,
        "testIntl.js"
            | "atomicsHelper.js"
            | "proxyTrapsHelper.js"
            | "tcoHelper.js"
            | "byteConversionValues.js"
            | "compareIterator.js"
            | "iteratorZipUtils.js"
    )
}

/// 生成 `Test262Error` 的 JS prelude（供测试脚本 `assert` 失败时抛出）。
/// 向拼接源码追加一段带注释标记的 harness 代码块。
pub(crate) fn append_source_chunk(out: &mut String, name: &str, source: &str) {
    out.push_str("\n// ---- test262 harness: ");
    out.push_str(name);
    out.push_str(" ----\n");
    out.push_str(source);
    out.push('\n');
}

/// 取测试元数据的 harness 缓存键（`includes` 列表 + onlyStrict / async 旗标）。
/// 旗标入键是因为它们改变前缀内容（use strict 指令 / async 捕获注入）。
fn harness_key(meta: &TestMeta) -> Vec<String> {
    let mut key = meta.includes.clone();
    if meta.flags.iter().any(|f| f == "onlyStrict") {
        key.push("__onlyStrict__".into());
    }
    if meta.flags.iter().any(|f| f == "async") {
        key.push("__async__".into());
    }
    key
}

/// 把拼接源码中顶层（行首）let/const 声明改写为 var，使其落全局对象、跨 run 可见。
///
/// 引擎的全局词法环境不跨 run 持久化：顶层 let/const 只活在当前 run 的词法环境，
/// 下一个 run（测试体脚本）看不到。改写为 var 后落全局对象，跨 run 共享。
/// 仅作用于行首声明：harness 文件内函数体代码均缩进，不受影响。
fn demote_top_level_lexical(source: &str) -> String {
    source
        .lines()
        .map(|line| {
            if let Some(rest) = line.strip_prefix("let ") {
                format!("var {rest}")
            } else if let Some(rest) = line.strip_prefix("const ") {
                format!("var {rest}")
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// 按测试元数据拼接完整的 harness 前缀源码（prelude + sta/assert + 各 include）。
///
/// `demote` 为真时把顶层 let/const 降级为 var（供两模块方案跨 run 共享），
/// 为假时保留原样（供单模块方案，同模块内 let/const 本可见）。
fn build_harness_source(
    meta: &TestMeta, harness: &HarnessSources, demote: bool,
) -> Result<String, String> {
    let mut source = String::new();
    // onlyStrict 测试在严格模式下运行：指令必须位于脚本最前（harness 之前）。
    if meta.flags.iter().any(|f| f == "onlyStrict") {
        append_source_chunk(&mut source, "use strict", "\"use strict\";");
    }
    // Test262Error 由 sta.js 提供；此处不再重复定义（重复函数声明会引发
    // 引擎 prototype 语义错乱，导致 assert.throws 的 constructor 比对失败）。
    append_source_chunk(
        &mut source,
        "sta.js",
        harness.get("sta.js").ok_or_else(|| String::from("unknown harness: sta.js"))?,
    );
    append_source_chunk(
        &mut source,
        "assert.js",
        harness
            .get("assert.js")
            .ok_or_else(|| String::from("unknown harness: assert.js"))?,
    );
    for include in &meta.includes {
        if is_blacklisted_harness(include) {
            return Err(format!("out-of-scope harness: {include}"));
        }
        let include_source = harness.get(include).ok_or_else(|| format!("unknown harness: {include}"))?;
        append_source_chunk(&mut source, include, include_source);
    }
    // 异步测试：注入 print 捕获 + $DONE（asyncTests 用 $DONE 报告结果）。
    // 结果写入 globalThis 全局字符串，run() 结束后由 runner 读取并按 marker 判定。
    if meta.flags.iter().any(|f| f == "async") {
        append_source_chunk(
            &mut source,
            "async capture",
            "globalThis.$__test262_async_result = \"\";\n\
             function print(msg) { globalThis.$__test262_async_result += String(msg) + \"\\n\"; }",
        );
        if let Some(done_src) = harness.get("doneprintHandle.js") {
            append_source_chunk(&mut source, "doneprintHandle.js", done_src);
        }
    }
    if demote {
        Ok(demote_top_level_lexical(&source))
    } else {
        Ok(source)
    }
}

/// 取得（并缓存）测试所需的 harness 前缀源码；缓存键为 includes 组合。
pub(crate) fn get_harness_prefix(
    meta: &TestMeta, harness: &HarnessSources, cache: &Arc<RwLock<HarnessPrefixCache>>,
) -> Result<String, String> {
    let key = harness_key(meta);
    {
        let guard = cache.read().unwrap();
        if let Some(prefix) = guard.get(&key) {
            return Ok(prefix.clone());
        }
    }

    let source = build_harness_source(meta, harness, false)?;
    cache.write().unwrap().insert(key, source.clone());
    Ok(source)
}

/// 取得（并缓存）测试所需的 harness 前缀编译产物；缓存键为 includes 组合
/// （含 onlyStrict / async 旗标）。命中直接返回缓存模块，未命中编译后存入。
///
/// 前缀经 `demote` 改写（顶层 let/const → var），使其落全局对象、跨 run 共享，
/// 供两模块方案（前缀模块 + 测试体模块顺序执行）复用。
pub(crate) fn get_harness_module(
    meta: &TestMeta, harness: &HarnessSources, engine: &Engine, compiler: &Compiler,
    cache: &Arc<RwLock<HarnessModuleCache>>,
) -> Result<Arc<CompiledModule>, String> {
    let key = harness_key(meta);
    {
        let guard = cache.read().unwrap();
        if let Some(m) = guard.get(&key) {
            return Ok(Arc::clone(m));
        }
    }
    let source = build_harness_source(meta, harness, true)?;
    let module = engine.compile_with(&source, compiler)?;
    cache.write().unwrap().insert(key, Arc::clone(&module));
    Ok(module)
}
