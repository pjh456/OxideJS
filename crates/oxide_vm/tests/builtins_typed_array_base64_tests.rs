use std::sync::Arc;

use oxide_compiler::compiler::Compiler;
use oxide_types::value::JsValue;
use oxide_vm::vm::Vm;

fn eval(vm: &mut Vm, source: &str) -> Result<JsValue, String> {
    let allocator = oxide_parser::Allocator::default();
    let program = oxide_parser::parse(&allocator, source).map_err(|e| format!("Parse error: {:?}", e))?;
    let module = Compiler::new().compile(&program).map_err(|e| format!("Compile error: {}", e))?;
    vm.run(&Arc::new(module))
}

fn to_str(vm: &Vm, val: JsValue) -> String {
    vm.lookup_str(val).unwrap_or_default()
}

#[test]
fn to_base64_rfc4648_vectors() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        r#"(function() {
            var vectors = [
                [new Uint8Array([]), ""],
                [new Uint8Array([102]), "Zg=="],
                [new Uint8Array([102, 111]), "Zm8="],
                [new Uint8Array([102, 111, 111]), "Zm9v"],
                [new Uint8Array([102, 111, 111, 98]), "Zm9vYg=="],
                [new Uint8Array([102, 111, 111, 98, 97]), "Zm9vYmE="],
                [new Uint8Array([102, 111, 111, 98, 97, 114]), "Zm9vYmFy"]
            ];
            for (var i = 0; i < vectors.length; i++) {
                if (vectors[i][0].toBase64() !== vectors[i][1]) return "vector " + i;
            }
            return true;
        })()"#,
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn to_base64_alphabet_and_omit_padding() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        r#"(function() {
            var a = new Uint8Array([199, 239, 242]);
            if (a.toBase64() !== "x+/y") return "std";
            if (a.toBase64({ alphabet: "base64url" }) !== "x-_y") return "url";
            var b = new Uint8Array([199, 239]);
            if (b.toBase64() !== "x+8=") return "pad";
            if (b.toBase64({ omitPadding: true }) !== "x+8") return "omit";
            if (b.toBase64({ omitPadding: 1 }) !== "x+8") return "tobool";
            if (b.toBase64({ omitPadding: 0 }) !== "x+8=") return "tobool0";
            if (new Uint8Array([255]).toBase64({ alphabet: "base64url", omitPadding: true }) !== "_w") return "urlomit";
            return true;
        })()"#,
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn to_base64_option_coercion_and_receiver() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        r#"(function() {
            // 装箱串与抛错 toString 对象均 TypeError（不做 ToPrimitive）。
            var t1 = (function() { try { new Uint8Array(2).toBase64({ alphabet: Object("base64") }); return false; } catch (e) { return e instanceof TypeError; } })();
            var t2 = (function() { try { new Uint8Array(2).toBase64({ alphabet: { toString: function() { throw 1; } } }); return false; } catch (e) { return e instanceof TypeError; } })();
            var t3 = (function() { try { new Uint8Array(2).toBase64({ alphabet: "other" }); return false; } catch (e) { return e instanceof TypeError; } })();
            // 选项 getter 触发恰一次，且其副作用在数据读取前生效。
            var accesses = 0;
            var array = new Uint8Array([0]);
            var options = {};
            Object.defineProperty(options, "alphabet", {
                get: function() { accesses += 1; array[0] = 255; return "base64"; }
            });
            var result = array.toBase64(options);
            return t1 && t2 && t3 && accesses === 1 && result === "/w==" && array[0] === 255;
        })()"#,
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn to_hex_and_receiver_validation() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        r#"(function() {
            if (new Uint8Array([]).toHex() !== "") return "empty";
            if (new Uint8Array([102, 111, 111, 98, 97, 114]).toHex() !== "666f6f626172") return "vector";
            // 非 Uint8 接收者抛 TypeError（kind 校验先于任何副作用）。
            var t1 = (function() { try { Uint8Array.prototype.toHex.call(new Uint8ClampedArray(2), {}); return false; } catch (e) { return e instanceof TypeError; } })();
            var t2 = (function() { try { Uint8Array.prototype.toBase64.call(new Int8Array(2)); return false; } catch (e) { return e instanceof TypeError; } })();
            // 非对象接收者同样抛。
            var t3 = (function() { try { Uint8Array.prototype.toBase64.call([]); return false; } catch (e) { return e instanceof TypeError; } })();
            return t1 && t2 && t3;
        })()"#,
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn set_from_base64_results_and_target_size() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        r#"(function() {
            // 标准向量：read/written 与内容。
            var pairs = [
                ["", 0, []],
                ["Zg==", 4, [102]],
                ["Zm8=", 4, [102, 111]],
                ["Zm9v", 4, [102, 111, 111]],
                ["Zm9vYg==", 8, [102, 111, 111, 98]]
            ];
            for (var i = 0; i < pairs.length; i++) {
                var target = new Uint8Array(4);
                var r = target.setFromBase64(pairs[i][0]);
                if (r.read !== pairs[i][1] || r.written !== pairs[i][2].length) return "rw " + i;
                for (var k = 0; k < 4; k++) {
                    var expect = k < pairs[i][2].length ? pairs[i][2][k] : 0;
                    if (target[k] !== expect) return "body " + i;
                }
            }
            // 小目标：块前停（第 2 块不消费）、恰好补齐。
            var t1 = new Uint8Array([255, 255, 255, 255, 255]);
            var r1 = t1.setFromBase64("Zm9vYmFy");
            if (r1.read !== 4 || r1.written !== 3 || t1[3] !== 255) return "small";
            var t2 = new Uint8Array([255, 255, 255, 255, 255]);
            var r2 = t2.setFromBase64("Zm9vYmE=");
            if (r2.read !== 8 || r2.written !== 5) return "exact";
            // 空白跳过且计入 read。
            var t3 = new Uint8Array(3);
            var r3 = t3.setFromBase64("Z g==");
            if (r3.read !== 5 || r3.written !== 1 || t3[0] !== 102) return "ws";
            return true;
        })()"#,
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn set_from_base64_last_chunk_handling() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        r#"(function() {
            function ok(input, handling, read, written) {
                var t = new Uint8Array(6);
                var r = t.setFromBase64(input, handling === undefined ? undefined : { lastChunkHandling: handling });
                return r.read === read && r.written === written;
            }
            function throws(input, handling) {
                try {
                    new Uint8Array(6).setFromBase64(input, handling === undefined ? undefined : { lastChunkHandling: handling });
                    return false;
                } catch (e) { return e instanceof SyntaxError; }
            }
            if (!ok("ZXhhZg==", undefined, 8, 4)) return "pad default";
            if (!ok("ZXhhZg==", "strict", 8, 4)) return "pad strict";
            if (!ok("ZXhhZg", "loose", 6, 4)) return "nopad loose";
            if (!ok("ZXhhZg", "stop-before-partial", 4, 3)) return "nopad sbp";
            if (!throws("ZXhhZg", "strict")) return "nopad strict";
            if (!ok("ZXhhZh==", undefined, 8, 4)) return "bits default";
            if (!throws("ZXhhZh==", "strict")) return "bits strict";
            if (!throws("ZXhhZg=")) return "partialpad default";
            if (!ok("ZXhhZg=", "stop-before-partial", 4, 3)) return "partialpad sbp";
            if (!throws("ZXhhZg=", "strict")) return "partialpad strict";
            if (!throws("ZXhhZg===")) return "excesspad default";
            if (!throws("ZXhhZg===", "stop-before-partial")) return "excesspad sbp";
            // 非法字符（含非 ASCII 空格）。
            if (!throws("Zm.9v")) return "illegal";
            if (!throws("Zg\u{00A0}==")) return "nbsp";
            return true;
        })()"#,
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn set_from_base64_subarray_and_option_order() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        r#"(function() {
            // subarray 偏移生效。
            var base = new Uint8Array([255, 255, 255, 255, 255, 255, 255]);
            var sub = base.subarray(2, 5);
            var r = sub.setFromBase64("Zm9vYmFy");
            if (r.read !== 4 || r.written !== 3) return "sub rw";
            if (base[2] !== 102 || base[3] !== 111 || base[4] !== 111 || base[5] !== 255) return "sub body";
            // 接收者校验先于选项副作用：抛错 getter 零触发。
            var touched = 0;
            var options = {};
            Object.defineProperty(options, "alphabet", {
                get: function() { touched += 1; throw new TypeError("no"); }
            });
            try {
                Uint8Array.prototype.setFromBase64.call(new Int8Array(2), "Zg==", options);
                return false;
            } catch (e) {
                if (!(e instanceof TypeError)) return "type";
            }
            if (touched !== 0) return "touched";
            // string 非原始串抛 TypeError，选项零触发。
            var optTouched = 0;
            var touchy = {};
            Object.defineProperty(touchy, "alphabet", {
                get: function() { optTouched += 1; throw 1; }
            });
            try {
                new Uint8Array(2).setFromBase64({ toString: function() { throw 1; } }, touchy);
                return false;
            } catch (e) {
                if (!(e instanceof TypeError)) return "str type";
            }
            return optTouched === 0;
        })()"#,
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn set_from_base64_writes_up_to_error() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        r#"(function() {
            var t = new Uint8Array([255, 255, 255, 255, 255]);
            var threw = false;
            try { t.setFromBase64("MjYyZm.9v"); } catch (e) { threw = e instanceof SyntaxError; }
            if (!threw) return "throw";
            return t[0] === 50 && t[1] === 54 && t[2] === 50 && t[3] === 255 && t[4] === 255;
        })()"#,
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn set_from_hex_results_and_odd_length() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        r#"(function() {
            var t = new Uint8Array(3);
            var r = t.setFromHex("aabbcc");
            if (r.read !== 6 || r.written !== 3) return "rw";
            if (t[0] !== 170 || t[1] !== 187 || t[2] !== 204) return "body";
            // 大小写宽容。
            t.fill(0);
            var r2 = t.setFromHex("AABB");
            if (r2.read !== 4 || t[0] !== 170 || t[1] !== 187) return "case";
            // 奇数长度最前判定：零写入抛 SyntaxError（含零长目标）。
            var threw1 = false;
            try { new Uint8Array(0).setFromHex("1"); } catch (e) { threw1 = e instanceof SyntaxError; }
            if (!threw1) return "odd zero";
            var t3 = new Uint8Array(3);
            var threw2 = false;
            try { t3.setFromHex("aaa"); } catch (e) { threw2 = e instanceof SyntaxError; }
            if (!threw2 || t3[0] !== 0) return "odd write";
            // 坏字符保留前字节。
            var t4 = new Uint8Array(3);
            var threw3 = false;
            try { t4.setFromHex("aaag"); } catch (e) { threw3 = e instanceof SyntaxError; }
            if (!threw3 || t4[0] !== 170 || t4[1] !== 0) return "uptoerr";
            return true;
        })()"#,
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn from_base64_static() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        r#"(function() {
            var vectors = [
                ["", 0], ["Zg==", 1], ["Zm8=", 2], ["Zm9v", 3],
                ["Zm9vYg==", 4], ["Zm9vYmE=", 5], ["Zm9vYmFy", 6]
            ];
            for (var i = 0; i < vectors.length; i++) {
                var arr = Uint8Array.fromBase64(vectors[i][0]);
                if (arr.length !== vectors[i][1]) return "len " + i;
                if (arr.buffer.byteLength !== vectors[i][1]) return "buf " + i;
                if (Object.getPrototypeOf(arr) !== Uint8Array.prototype) return "proto " + i;
            }
            // 不读 this：子类构造器不触发，结果 proto 恒 %Uint8Array%.prototype。
            var Subclass = new Function("Uint8Array", "class S extends Uint8Array { constructor() { throw new Error('ctor'); } } return S;")(Uint8Array);
            var fromSubclass = Subclass.fromBase64("Zg==");
            if (Object.getPrototypeOf(fromSubclass) !== Uint8Array.prototype) return "sub proto";
            // 字符串检查先于选项。
            var threw1 = false;
            try { Uint8Array.fromBase64(42, { alphabet: Object("base64") }); } catch (e) { threw1 = e instanceof TypeError; }
            if (!threw1) return "strcheck";
            // 尾块矩阵（fromBase64 无 maxLength 面）。
            if (!throws("A")) return "A";
            if (Uint8Array.fromBase64("A", { lastChunkHandling: "stop-before-partial" }).length !== 0) return "A sbp";
            if (Uint8Array.fromBase64("ABCDA", { lastChunkHandling: "stop-before-partial" }).length !== 3) return "ABCDA sbp";
            var t5 = Uint8Array.fromBase64("AA=", { lastChunkHandling: "stop-before-partial" });
            if (t5.length !== 0) return "AA= sbp";
            var t6 = Uint8Array.fromBase64("ZXhhZg", { lastChunkHandling: "stop-before-partial" });
            if (t6.length !== 3) return "ZXhhZg sbp";
            var threw2 = false;
            try { Uint8Array.fromBase64("ZXhhZg", { lastChunkHandling: "strict" }); } catch (e) { threw2 = e instanceof SyntaxError; }
            if (!threw2) return "strict";
            return true;
            function throws(s) {
                try { Uint8Array.fromBase64(s); return false; } catch (e) { return e instanceof SyntaxError; }
            }
        })()"#,
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn from_hex_static() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        r#"(function() {
            var arr = Uint8Array.fromHex("666f6f626172");
            if (arr.length !== 6 || arr.buffer.byteLength !== 6) return "len";
            if (Object.getPrototypeOf(arr) !== Uint8Array.prototype) return "proto";
            if (arr[0] !== 102 || arr[5] !== 114) return "body";
            var threw = false;
            try { Uint8Array.fromHex("a"); } catch (e) { threw = e instanceof SyntaxError; }
            if (!threw) return "odd";
            var threw2 = false;
            try { Uint8Array.fromHex({ toString: function() { throw 1; } }); } catch (e) { threw2 = e instanceof TypeError; }
            if (!threw2) return "type";
            return true;
        })()"#,
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn typed_array_prototype_to_string_chain() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        r#"(function() {
            // own toString 与 Array.prototype.toString 同一函数对象。
            if (TypedArray.prototype.toString !== Array.prototype.toString) return "identity";
            var d = Object.getOwnPropertyDescriptor(TypedArray.prototype, "toString");
            if (d === undefined) return "not own";
            if (d.writable !== true || d.enumerable !== false || d.configurable !== true) return "attrs";
            // proto 链接 Array.prototype。
            if (Object.getPrototypeOf(TypedArray.prototype) !== Array.prototype) return "chain";
            // 实例 toString 走 Array 语义（经 TA 自身 join）。
            if (new Uint8Array([1, 2, 3]).toString() !== "1,2,3") return "instance";
            if (new Int16Array([7]).toString() !== "7") return "instance16";
            return true;
        })()"#,
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn clamped_inherits_base64_methods_but_rejects() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        r#"(function() {
            // clamped 接收者被 kind 校验拒绝（原型链当前不继承方法，
            // 调用不可见成员同样 TypeError）。
            var threw1 = false;
            try { new Uint8ClampedArray(2).toBase64(); } catch (e) { threw1 = e instanceof TypeError; }
            var threw2 = false;
            try { new Uint8ClampedArray(2).setFromHex("aa"); } catch (e) { threw2 = e instanceof TypeError; }
            var threw3 = false;
            try { new Uint8ClampedArray(2).setFromBase64("Zg=="); } catch (e) { threw3 = e instanceof TypeError; }
            var threw4 = false;
            try { new Uint8ClampedArray(2).toHex(); } catch (e) { threw4 = e instanceof TypeError; }
            return threw1 && threw2 && threw3 && threw4;
        })()"#,
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn base64_method_metadata() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        r#"(function() {
            function meta(obj, name, length) {
                var m = obj[name];
                if (typeof m !== "function") return name + " missing";
                if (m.name !== name) return name + " name";
                if (m.length !== length) return name + " length";
                var d = Object.getOwnPropertyDescriptor(obj, name);
                if (d.enumerable !== false || d.writable !== true || d.configurable !== true) return name + " attrs";
                return null;
            }
            var bad = meta(Uint8Array.prototype, "toBase64", 0);
            if (bad) return bad;
            bad = meta(Uint8Array.prototype, "toHex", 0);
            if (bad) return bad;
            bad = meta(Uint8Array.prototype, "setFromBase64", 1);
            if (bad) return bad;
            bad = meta(Uint8Array.prototype, "setFromHex", 1);
            if (bad) return bad;
            bad = meta(Uint8Array, "fromBase64", 1);
            if (bad) return bad;
            bad = meta(Uint8Array, "fromHex", 1);
            return bad === null;
        })()"#,
    )
    .unwrap();
    assert!(result.as_bool(), "metadata: {}", to_str(&vm, result));
}
