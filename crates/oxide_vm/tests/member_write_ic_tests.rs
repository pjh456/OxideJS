//! member 复合写（obj.x += 1 / obj.x++ 等）接入 IC 写命中的行为测试。
//!
//! 覆盖三组语义：①写侧命中（depth==0 槽直写，读写对称）；②写新属性走
//! CreateDataProperty 快路径（shape 转换，命中无法覆盖）；③继承属性 shadow /
//! 原型 setter / 只读继承属性（写侧 depth==0 限制与原型链检查兜底，防原型污染）。
//!
//! 计数断言基于单次 run 内 miss/hit 对比（run() 每次冷启动，IC 学习不跨 run）。

use oxide_compiler::compiler::Compiler;
use oxide_parser::Allocator;
use oxide_vm::vm::Vm;

/// 编译并单次运行，返回 (结果, hits, misses)。
fn run_once(source: &str) -> (String, u64, u64) {
    let allocator = Allocator::default();
    let program = oxide_parser::parse(&allocator, source).expect("parse");
    let module = Compiler::new().compile(&program).expect("compile");
    let mut vm = Vm::new();
    let result = vm.run(&module).expect("run");
    // 字符串结果经 lookup_str 提取内容（JsValue Display 只打印 {string}）。
    let r = vm.lookup_str(result).unwrap_or_else(|| format!("{result}"));
    (r, vm.ic_hit_count(), vm.ic_miss_count())
}

/// 单态 member 复合写：写侧命中直写（读侧命中不计数，pre-existing 口径——
/// 计数仅来自写侧与独立 IC_GET/IC_SET 分发站点）。
#[test]
fn member_compound_monomorphic_write_hits() {
    let (r, hits, misses) = run_once(
        r#"var t = { x: 0 };
           for (var i = 0; i < 10000; i++) { t.x += 1; }
           t.x"#,
    );
    assert_eq!(r, "10000");
    assert_eq!(misses, 1, "仅末尾 `t.x` IC_GET 站点学习 miss，循环写侧零 miss");
    assert_eq!(hits, 10000, "循环 10000 次写侧全命中（实际 {hits}）");
}

/// 4 shape 轮换 member 复合写：写侧复用读侧学习的 4 槽条目，循环内零写 miss。
#[test]
fn member_compound_poly4_read_write_hits() {
    let (r, hits, misses) = run_once(
        r#"var objs = [{x:0}, {x:0,y:1}, {x:0,z:1}, {x:0,y:1,z:2}];
           for (var i = 0; i < 10000; i++) {
             var t = objs[i % 4];
             t.x += 1;
           }
           objs[0].x + objs[1].x + objs[2].x + objs[3].x"#,
    );
    assert_eq!(r, "10000", "四对象 x 各增 2500 次");
    assert_eq!(misses, 4, "末尾 4 个 `objs[i].x` IC_GET 站点各学习 1 次");
    assert_eq!(hits, 10000, "循环写侧 10000 全命中（实际 {hits}）");
}

/// 继承属性 shadow（核心正确性）：写侧只接受 depth==0 槽，原型不被污染。
#[test]
fn inherited_prop_shadows_receiver_not_proto() {
    let (r, _hits, _misses) = run_once(
        r#"var p = { x: 1 };
           var o = Object.create(p);
           o.x += 1;
           o.x + ":" + p.x"#,
    );
    assert_eq!(r, "2:1", "o 获得 own x=2，p.x 保持 1（未直写原型）");
}

/// IC_SET 独立赋值路径的继承 shadow：`Object.create(p).x = 5` 同样不写原型。
#[test]
fn ic_set_inherited_shadow_receiver_only() {
    let (r, _hits, _misses) = run_once(
        r#"var p = { x: 1 };
           var o = Object.create(p);
           o.x = 5;
           p.x"#,
    );
    assert_eq!(r, "1", "p.x 保持 1，o 得到 own x=5");
}

/// 原型 accessor setter：IC_SET 与 member 复合写都触发 setter，接收者不建新槽。
#[test]
fn proto_setter_called_both_write_paths() {
    let (r, _hits, _misses) = run_once(
        r#"var calls = 0;
           var p = { set x(v) { calls++; } };
           var o = Object.create(p);
           o.x = 5;
           o.x += 2;
           calls + ":" + Object.keys(o).length"#,
    );
    assert_eq!(r, "2:0", "setter 被调 2 次（IC_SET 与 member 各 1），o 无 own 属性");
}

/// 严格模式原型只读 data：两条写路径都按 ordinary_set 抛 TypeError。
#[test]
fn readonly_inherited_data_raises_in_strict() {
    let (r, _hits, _misses) = run_once(
        r#"var p = {};
           Object.defineProperty(p, 'x', { value: 1, writable: false });
           var o = Object.create(p);
           var msg = '';
           (function() { 'use strict';
             try { o.x = 5; } catch (e) { msg = e.message; }
             try { o.x += 1; } catch (e) { msg += '|' + e.message; }
           })();
           msg"#,
    );
    assert_eq!(
        r, "cannot assign to read-only property|cannot assign to read-only property",
        "两条路径都拒绝写只读继承属性"
    );
}

/// sloppy 原型只读 data：两条写路径都静默 no-op（值不变、不抛）。
#[test]
fn readonly_inherited_data_silently_fails_in_sloppy() {
    let (r, _hits, _misses) = run_once(
        r#"var p = {};
           Object.defineProperty(p, 'x', { value: 1, writable: false });
           var o = Object.create(p);
           var threw = false;
           try { o.x = 5; } catch (e) { threw = true; }
           try { o.x += 1; } catch (e) { threw = true; }
           threw + ':' + o.x + ':' + o.hasOwnProperty('x')"#,
    );
    assert_eq!(r, "false:1:false", "sloppy 只读继承属性写静默失败，值保持 1，不创建 own 属性");
}

/// 写新属性（shape 转换）：快路径建 shape + 写回，循环内同 shape 写全命中。
#[test]
fn write_new_prop_then_hit() {
    let (r, hits, misses) = run_once(
        r#"var o = {};
           o.y = 0;
           for (var i = 0; i < 10000; i++) { o.y += 1; }
           o.y"#,
    );
    assert_eq!(r, "10000");
    assert_eq!(misses, 2, "`o.y = 0` 站点 1 次 + 末尾 `o.y` IC_GET 站点 1 次");
    assert_eq!(hits, 10000, "compound 写侧 10000 全命中（实际 {hits}）");
}

/// ic_cache.js 同款模式（批量建对象 + 分批写 y/z 新属性）：快路径建 shape 正确。
#[test]
fn ic_cache_style_new_prop_writes() {
    let (r, _hits, _misses) = run_once(
        r#"var objs = [];
           for (var i = 0; i < 100; i++) {
             objs.push({ x: i });
             if (i % 3 === 0) objs[i].y = i;
             if (i % 5 === 0) objs[i].z = i;
           }
           var yok = 0, zok = 0;
           for (var i = 0; i < 100; i++) {
             if (i % 3 === 0 && objs[i].y === i) yok++;
             if (i % 5 === 0 && objs[i].z === i) zok++;
           }
           yok + ":" + zok"#,
    );
    assert_eq!(r, "34:20", "y/z 全部写入成功（含 i%15==0 的 y+z 同对象场景）");
}

/// has_prop_meta 对象 member 写走 ordinary_set（不缓存，accessor 语义保留）。
#[test]
fn meta_object_member_write_uses_ordinary_set() {
    let (r, _hits, _misses) = run_once(
        r#"var store = 1;
           var o = { get a() { return store; }, set a(v) { store = v; } };
           o.a += 5;
           store"#,
    );
    assert_eq!(r, "6", "accessor 对象走 setter，store 更新为 6");
}

/// 命中率口径：member 写 hit/miss 计入后 ic_hit_rate 反映写侧全量命中。
#[test]
fn member_write_counts_in_ic_hit_rate() {
    let allocator = Allocator::default();
    let program = oxide_parser::parse(
        &allocator,
        r#"var t = { x: 0 };
           for (var i = 0; i < 1000; i++) { t.x += 1; }
           t.x"#,
    )
    .expect("parse");
    let module = Compiler::new().compile(&program).expect("compile");
    let mut vm = Vm::new();
    let result = vm.run(&module).expect("run");
    let r = vm.lookup_str(result).unwrap_or_else(|| format!("{result}"));
    assert_eq!(r, "1000");
    // 循环写侧 1000 命中 + 末尾 IC_GET 1 miss → 命中率 ~0.999（写侧计入后趋近真实）。
    assert!(vm.ic_hit_rate() > 0.99, "member 写侧命中计入命中率（实际 {:.4}）", vm.ic_hit_rate());
}

/// `__proto__` 赋值 + 继承成员复合写：原型链建立正常，shadow 语义不变（回归锚定）。
#[test]
fn proto_assignment_then_compound_shadow() {
    let (r, _hits, _misses) = run_once(
        r#"var p = { x: 1 };
           var o = {};
           o.__proto__ = p;
           o.x += 1;
           o.x + ":" + p.x"#,
    );
    assert_eq!(r, "2:1", "__proto__ 赋值建链后复合写 shadow 到 o，p 不被污染");
}

/// 数组 length 复合写分流 ordinary_set（ArraySetLength）：`arr.length += 1` 更新逻辑
/// 长度而非建影子槽，后续 push / 索引写读正常（P1-1 回归锚定）。
#[test]
fn array_length_compound_write_uses_array_set_length() {
    let (r, _hits, _misses) = run_once(
        r#"var arr = [1, 2, 3];
           arr.length += 1;
           var l1 = arr.length;
           arr.push(5);
           arr[1] = 9;
           l1 + ":" + arr.length + ":" + arr.join(",")"#,
    );
    assert_eq!(r, "4:5:1,9,3,,5", "length 复合写后 l1=4，push 后 length=5，元素区正确");
}

/// 数组 length 复合写后再直接赋值 `arr.length = N`：两写路径均走 ArraySetLength，
/// 元素区按新长度伸缩，不残留影子槽（P1-1 双路径对称回归锚定）。
#[test]
fn array_length_compound_then_direct_assign() {
    let (r, _hits, _misses) = run_once(
        r#"var arr = [1, 2, 3];
           arr.length += 1;
           arr.length = 5;
           var l = arr.length;
           arr[4] = 7;
           l + ":" + arr.join(",")"#,
    );
    assert_eq!(r, "5:1,2,3,,7", "length=5 后逻辑长度与元素区同步伸缩");
}

/// 数组写新命名属性（IC_SET 快路径建 shape 槽）：命名槽在元素区之后，元素写入不错位。
#[test]
fn array_named_prop_create_then_element_write() {
    let (r, _hits, _misses) = run_once(
        r#"var arr = [1];
           arr.foo = 1;
           arr[0] = 9;
           arr.foo"#,
    );
    assert_eq!(r, "1", "元素区写入后命名属性 foo 不错位（槽在元素区之后）");
}

/// 数组命名属性 member 复合写命中 shadow 槽 + 元素增长后读回正确（P2-1）。
#[test]
fn array_named_prop_compound_hit_after_element_growth() {
    let (r, _hits, _misses) = run_once(
        r#"var arr = [1];
           arr.foo = 2;
           arr.foo += 3;
           arr[0] = 9;
           arr.foo"#,
    );
    assert_eq!(r, "5", "member 复合写命中 shadow 槽（2+3=5），元素增长后仍不错位");
}
