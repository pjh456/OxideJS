//! 脏重建职责：按脏标记选择性重建 builtin world（干净家族 Arc::clone 复用、
//! Function/Object 4 个对象经 `mem::forget` 抬升 Arc 计数永久保留），收尾把保留
//! 对象 proto 槽改写到新指针，并恰好释放一次被替换旧 P 对象的属性区。

use oxide_types::mem::P;
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

use super::construct::{
    builtin_labels, make_error_subtypes, make_named_pair, make_typed_array_family, tag_boolean_proto, tag_number_proto,
    tag_string_proto,
    wire_builtin_world_links, ErrorSubtypeProtos, TypedArrayFamily,
};
use super::BuiltinWorld;
use crate::kernel::BuiltinDirtySet;
use crate::shape_forge::{ShapeForge, EMPTY_SHAPE_ID};
use crate::string_forge::PermInterner;

impl BuiltinWorld {
    /// 选择性重建收尾：把保留对象的原型槽改写到新对象，并释放被替换旧对象的
    /// 属性区。
    ///
    /// 选择性重建（[`Self::rebuild_with_dirty`]）只为脏家族新建对象，未脏家族
    /// 的新旧 world 持有同一个 `Arc` 对象。本函数在旧 world 被丢弃前做两件事：
    ///
    /// 1. 原型槽改写：保留对象（登记表 wrapper 与新旧 world 共享的 P 字段）
    ///    的 `[[Prototype]]` 槽若仍指向已被替换的旧对象，改写为对应的新对象；
    /// 2. 释放：被替换旧 P 对象的四处堆外属性区（命名属性值 / 命名属性元数据 /
    ///    数组元素 / 数组元素元数据）逐一释放并置空。
    ///
    /// # 边界与前提
    /// - 须在 `inherit_leaked_objects` 之后、旧 world 被替换前调用，且此刻
    ///   无存活 VM（宿主的重置边界）；
    /// - 原型槽改写必须先于释放完成，否则保留对象的原型链会读到已释放的
    ///   属性区；
    /// - 替换集按逐字段新旧指针比较判定（stub 按指针集合），与保留集不相交：
    ///   未替换字段的属性区归 session 收尾（`teardown_heap_data`）释放，此处
    ///   不碰，不双放；
    /// - Function/Object 4 个对象本体由 `rebuild_with_dirty` 经 `mem::forget`
    ///   抬升 Arc 计数后永久保留（兜底用途见该函数注意事项）；此处只释放其
    ///   属性区——原型槽改写遗漏时读者看到的是属性静默缺失（本体存活），
    ///   不是 use-after-free；
    /// - 只改写原型槽；属性值之间的链接由同家族同批替换与绑定层重新同步
    ///   覆盖，不在此处改写。
    ///
    /// # 副作用
    /// - 保留对象原型槽改写，每次改写递增该对象 generation；
    /// - 被替换旧 P 对象四处属性区释放并置空（幂等，重入为 no-op）。
    pub fn retire_replaced(&self, old: &BuiltinWorld) {
        // 替换映射：逐字段新旧指针比较（stubs 按指针集合），记录被替换的
        // 旧指针 → 新指针；stub 无继任者记空指针，只进释放集。
        let old_fields = old.all_p_fields();
        let new_fields = self.all_p_fields();
        let mut remap: std::collections::HashMap<*mut JsObject, *mut JsObject> = std::collections::HashMap::new();
        for (o, n) in old_fields.iter().zip(new_fields.iter()) {
            let (op, np) = (o.as_ptr() as *mut JsObject, n.as_ptr() as *mut JsObject);
            if !std::ptr::eq(op, np) {
                remap.insert(op, np);
            }
        }
        for p in &old.stub_objects {
            let op = p.as_ptr() as *mut JsObject;
            if !self.stub_objects.iter().any(|q| std::ptr::eq(q.as_ptr() as *mut JsObject, op)) {
                remap.insert(op, std::ptr::null_mut());
            }
        }
        if remap.is_empty() {
            return;
        }
        // 原型槽改写：保留对象（新旧 world 同一指针）proto 槽仍指被替换旧
        // 指针的，改写到新指针——与 `wire_builtin_world_links` 的
        // set_proto_if_changed 同模式。
        let repoint = |obj: &mut JsObject| {
            let cur = obj.proto();
            if !cur.is_object() {
                return;
            }
            let cur_ptr = cur.as_js_object_ptr();
            let Some(np) = remap.get(&cur_ptr).copied() else {
                return;
            };
            if np.is_null() {
                return;
            }
            // SAFETY: np 是本 world 的 P 对象；full_reset 是无存活 VM 的
            // 时刻，无并发读者，成环检查由 set_proto 内部完成。
            obj.set_proto(JsValue::from_js_object(np)).ok();
        };
        for (o, n) in old_fields.iter().zip(new_fields.iter()) {
            let op = o.as_ptr() as *mut JsObject;
            let np = n.as_ptr() as *mut JsObject;
            if std::ptr::eq(op, np) {
                // SAFETY: op/np 指向同一保留 P 对象；full_reset 是无存活 VM
                // 的时刻，无并发读者。
                unsafe {
                    repoint(&mut *np);
                }
            }
        }
        for slot in self.leaked_objects.borrow().iter() {
            let ptr = slot.ptr;
            // SAFETY: 登记表指针 session 存活期内有效；full_reset 是无存活 VM
            // 的时刻，无并发读者。
            unsafe {
                repoint(&mut *ptr);
            }
        }
        // 后置条件检查：原型槽改写后保留对象 proto 槽不得残留任何被替换旧指针。
        for (o, n) in old_fields.iter().zip(new_fields.iter()) {
            let op = o.as_ptr() as *mut JsObject;
            let np = n.as_ptr() as *mut JsObject;
            if std::ptr::eq(op, np) {
                let cur = unsafe { (*np).proto() };
                debug_assert!(
                    !cur.is_object() || !remap.contains_key(&cur.as_js_object_ptr()),
                    "保留字段 proto 槽不得残留被替换旧指针"
                );
            }
        }
        for slot in self.leaked_objects.borrow().iter() {
            let cur = unsafe { (*slot.ptr).proto() };
            debug_assert!(
                !cur.is_object() || !remap.contains_key(&cur.as_js_object_ptr()),
                "登记表 wrapper proto 槽不得残留被替换旧指针"
            );
        }
        // 释放：被替换旧 P 对象属性区逐一恰好释放一次（本体不释放；经
        // `mem::forget` 抬升 Arc 计数的 4 个对象保留本体、属性区同样释放）。
        for &op in remap.keys() {
            // SAFETY: op 是旧 world 被替换的 P 对象，属性区仅此一处释放并置空
            // （幂等）；full_reset 是无存活 VM 的时刻，无并发读者。
            unsafe {
                (&mut *op).release_raw_heap();
            }
        }
    }

    /// 按脏标记选择性重建 builtin world：仅重建被污染的对象家族，未污染的保留原指针。
    ///
    /// # 注意事项
    /// - Function/Object 家族脏时，先对旧的 `function_proto` /
    ///   `function_constructor` / `object_proto` / `object_constructor` 各用
    ///   `mem::forget` 抬升一次 Arc 计数（本体永久保留）再重建。原因：方法
    ///   wrapper 经 `Box::into_raw` 分配后登记在释放登记表、跨重建存活，其
    ///   `[[Prototype]]` 裸指针指向绑定时的旧 function_proto；执行期原型链查找
    ///   （如 `push.call` 沿 wrapper 原型链取 `call`）仍走这些对象，session
    ///   重置只清执行态、不切断该路径，旧对 Arc 若在此归零则指针悬空。保留的
    ///   本体同时充当 `retire_replaced` 原型槽改写遗漏的兜底：遗漏时读者仅见
    ///   属性缺失，不产生 use-after-free。属性区于原型槽改写完成后由
    ///   `retire_replaced` 释放。
    pub fn rebuild_with_dirty(
        current: &BuiltinWorld, string_forge: &PermInterner, shape_forge: &ShapeForge, dirty: &BuiltinDirtySet,
    ) -> BuiltinWorld {
        let labels = builtin_labels(string_forge);

        // 用 `mem::forget` 抬升 4 个对象（旧 Function/Object 对）的 Arc 计数
        // 使其永不归零——保留对象的原型槽改写在 `retire_replaced`（本函数
        // 返回后、旧 world 被替换前）完成，抬升计数的本体是原型槽改写遗漏
        // 的兜底，不经任何路径释放。
        if dirty.function || dirty.object {
            std::mem::forget(current.function_proto.clone());
            std::mem::forget(current.function_constructor.clone());
            std::mem::forget(current.object_proto.clone());
            std::mem::forget(current.object_constructor.clone());
        }
        let (object_proto, object_constructor) = if dirty.object {
            make_named_pair(string_forge, shape_forge, labels, "Object")
        } else {
            (current.object_proto.clone(), current.object_constructor.clone())
        };
        let (array_proto, array_constructor) = if dirty.array {
            make_named_pair(string_forge, shape_forge, labels, "Array")
        } else {
            (current.array_proto.clone(), current.array_constructor.clone())
        };
        let (function_proto, function_constructor) = if dirty.function {
            make_named_pair(string_forge, shape_forge, labels, "Function")
        } else {
            (current.function_proto.clone(), current.function_constructor.clone())
        };
        let (string_proto, string_constructor) = if dirty.string {
            // 脏重建与全量构造同形：原型本体须带 String 对象 tag 与空串包值
            // （+length 物化），漏此分支 full_reset 后 tag 翻回 "Object"。
            let (proto, ctor) = make_named_pair(string_forge, shape_forge, labels, "String");
            tag_string_proto(&proto, string_forge, shape_forge);
            (proto, ctor)
        } else {
            (current.string_proto.clone(), current.string_constructor.clone())
        };
        let (number_proto, number_constructor) = if dirty.number {
            // 脏重建与全量构造同形：原型本体须带 Number 对象 tag 与 +0 包值，
            // 漏此分支 full_reset 后 tag 翻回 "Object"。
            let (proto, ctor) = make_named_pair(string_forge, shape_forge, labels, "Number");
            tag_number_proto(&proto);
            (proto, ctor)
        } else {
            (current.number_proto.clone(), current.number_constructor.clone())
        };
        let (boolean_proto, boolean_constructor) = if dirty.boolean {
            // 脏重建与全量构造同形：原型本体须带 Boolean 对象 tag 与 false 包值，
            // 漏此分支 full_reset 后 tag 翻回 "Object"。
            let (proto, ctor) = make_named_pair(string_forge, shape_forge, labels, "Boolean");
            tag_boolean_proto(&proto);
            (proto, ctor)
        } else {
            (current.boolean_proto.clone(), current.boolean_constructor.clone())
        };
        let (error_proto, error_constructor, error_subtypes) = if dirty.error_family {
            let (error_proto, error_constructor) = make_named_pair(string_forge, shape_forge, labels, "Error");
            let error_subtypes = make_error_subtypes(&error_proto);
            (error_proto, error_constructor, error_subtypes)
        } else {
            (
                current.error_proto.clone(),
                current.error_constructor.clone(),
                ErrorSubtypeProtos {
                    type_error_proto: current.type_error_proto.clone(),
                    reference_error_proto: current.reference_error_proto.clone(),
                    range_error_proto: current.range_error_proto.clone(),
                    syntax_error_proto: current.syntax_error_proto.clone(),
                    uri_error_proto: current.uri_error_proto.clone(),
                    eval_error_proto: current.eval_error_proto.clone(),
                    suppressed_error_proto: current.suppressed_error_proto.clone(),
                },
            )
        };
        let (
            symbol_proto,
            symbol_constructor,
            sym_match,
            sym_replace,
            sym_search,
            sym_split,
            sym_iterator,
            sym_to_primitive,
            sym_has_instance,
            sym_match_all,
            sym_async_iterator,
            sym_to_string_tag,
            sym_species,
            sym_async_dispose,
            sym_dispose,
        ) = if dirty.symbol_family {
            let (symbol_proto, symbol_constructor) = make_named_pair(string_forge, shape_forge, labels, "Symbol");
            (
                symbol_proto,
                symbol_constructor,
                P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null())),
                P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null())),
                P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null())),
                P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null())),
                P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null())),
                P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null())),
                P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null())),
                P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null())),
                P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null())),
                P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null())),
                P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null())),
                P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null())),
                P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null())),
            )
        } else {
            (
                current.symbol_proto.clone(),
                current.symbol_constructor.clone(),
                current.sym_match.clone(),
                current.sym_replace.clone(),
                current.sym_search.clone(),
                current.sym_split.clone(),
                current.sym_iterator.clone(),
                current.sym_to_primitive.clone(),
                current.sym_has_instance.clone(),
                current.sym_match_all.clone(),
                current.sym_async_iterator.clone(),
                current.sym_to_string_tag.clone(),
                current.sym_species.clone(),
                current.sym_async_dispose.clone(),
                current.sym_dispose.clone(),
            )
        };

        let math_object = if dirty.math {
            P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null()))
        } else {
            current.math_object.clone()
        };
        let json_object = if dirty.json {
            P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null()))
        } else {
            current.json_object.clone()
        };
        let (date_proto, date_constructor) = if dirty.date {
            make_named_pair(string_forge, shape_forge, labels, "Date")
        } else {
            (current.date_proto.clone(), current.date_constructor.clone())
        };
        let (set_proto, set_constructor) = if dirty.set {
            make_named_pair(string_forge, shape_forge, labels, "Set")
        } else {
            (current.set_proto.clone(), current.set_constructor.clone())
        };
        let (map_proto, map_constructor) = if dirty.map {
            make_named_pair(string_forge, shape_forge, labels, "Map")
        } else {
            (current.map_proto.clone(), current.map_constructor.clone())
        };
        let (regexp_proto, regexp_constructor) = if dirty.regexp {
            make_named_pair(string_forge, shape_forge, labels, "RegExp")
        } else {
            (current.regexp_proto.clone(), current.regexp_constructor.clone())
        };
        let (array_buffer_proto, array_buffer_constructor) = if dirty.array_buffer {
            make_named_pair(string_forge, shape_forge, labels, "ArrayBuffer")
        } else {
            (current.array_buffer_proto.clone(), current.array_buffer_constructor.clone())
        };
        let (shared_array_buffer_proto, shared_array_buffer_constructor) = if dirty.shared_array_buffer {
            make_named_pair(string_forge, shape_forge, labels, "SharedArrayBuffer")
        } else {
            (current.shared_array_buffer_proto.clone(), current.shared_array_buffer_constructor.clone())
        };
        let atomics_object = if dirty.atomics {
            P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null()))
        } else {
            current.atomics_object.clone()
        };
        let (data_view_proto, data_view_constructor) = if dirty.data_view {
            make_named_pair(string_forge, shape_forge, labels, "DataView")
        } else {
            (current.data_view_proto.clone(), current.data_view_constructor.clone())
        };
        let typed_arrays = if dirty.typed_array_family {
            make_typed_array_family(string_forge, shape_forge, labels, &object_proto)
        } else {
            TypedArrayFamily {
                typed_array_proto: current.typed_array_proto.clone(),
                typed_array_constructor: current.typed_array_constructor.clone(),
                int8array_constructor: current.int8array_constructor.clone(),
                int8array_proto: current.int8array_proto.clone(),
                uint8array_constructor: current.uint8array_constructor.clone(),
                uint8array_proto: current.uint8array_proto.clone(),
                uint8clampedarray_constructor: current.uint8clampedarray_constructor.clone(),
                uint8clampedarray_proto: current.uint8clampedarray_proto.clone(),
                int16array_constructor: current.int16array_constructor.clone(),
                int16array_proto: current.int16array_proto.clone(),
                uint16array_constructor: current.uint16array_constructor.clone(),
                uint16array_proto: current.uint16array_proto.clone(),
                int32array_constructor: current.int32array_constructor.clone(),
                int32array_proto: current.int32array_proto.clone(),
                uint32array_constructor: current.uint32array_constructor.clone(),
                uint32array_proto: current.uint32array_proto.clone(),
                float32array_constructor: current.float32array_constructor.clone(),
                float32array_proto: current.float32array_proto.clone(),
                float64array_constructor: current.float64array_constructor.clone(),
                float64array_proto: current.float64array_proto.clone(),
                bigint64array_constructor: current.bigint64array_constructor.clone(),
                bigint64array_proto: current.bigint64array_proto.clone(),
                biguint64array_constructor: current.biguint64array_constructor.clone(),
                biguint64array_proto: current.biguint64array_proto.clone(),
            }
        };
        let (
            temporal_object,
            temporal_now_object,
            instant_proto,
            instant_constructor,
            plain_date_proto,
            plain_date_constructor,
            plain_time_proto,
            plain_time_constructor,
            duration_proto,
            duration_constructor,
            zoned_date_time_proto,
            zoned_date_time_constructor,
            plain_date_time_proto,
            plain_date_time_constructor,
            plain_month_day_proto,
            plain_month_day_constructor,
            plain_year_month_proto,
            plain_year_month_constructor,
        ) = if dirty.temporal {
            let (instant_proto, instant_constructor) = make_named_pair(string_forge, shape_forge, labels, "Instant");
            let (plain_date_proto, plain_date_constructor) =
                make_named_pair(string_forge, shape_forge, labels, "PlainDate");
            let (plain_time_proto, plain_time_constructor) =
                make_named_pair(string_forge, shape_forge, labels, "PlainTime");
            let (duration_proto, duration_constructor) = make_named_pair(string_forge, shape_forge, labels, "Duration");
            let (zoned_date_time_proto, zoned_date_time_constructor) =
                make_named_pair(string_forge, shape_forge, labels, "ZonedDateTime");
            let (plain_date_time_proto, plain_date_time_constructor) =
                make_named_pair(string_forge, shape_forge, labels, "PlainDateTime");
            let (plain_month_day_proto, plain_month_day_constructor) =
                make_named_pair(string_forge, shape_forge, labels, "PlainMonthDay");
            let (plain_year_month_proto, plain_year_month_constructor) =
                make_named_pair(string_forge, shape_forge, labels, "PlainYearMonth");
            (
                P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null())),
                P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null())),
                instant_proto,
                instant_constructor,
                plain_date_proto,
                plain_date_constructor,
                plain_time_proto,
                plain_time_constructor,
                duration_proto,
                duration_constructor,
                zoned_date_time_proto,
                zoned_date_time_constructor,
                plain_date_time_proto,
                plain_date_time_constructor,
                plain_month_day_proto,
                plain_month_day_constructor,
                plain_year_month_proto,
                plain_year_month_constructor,
            )
        } else {
            (
                current.temporal_object.clone(),
                current.temporal_now_object.clone(),
                current.instant_proto.clone(),
                current.instant_constructor.clone(),
                current.plain_date_proto.clone(),
                current.plain_date_constructor.clone(),
                current.plain_time_proto.clone(),
                current.plain_time_constructor.clone(),
                current.duration_proto.clone(),
                current.duration_constructor.clone(),
                current.zoned_date_time_proto.clone(),
                current.zoned_date_time_constructor.clone(),
                current.plain_date_time_proto.clone(),
                current.plain_date_time_constructor.clone(),
                current.plain_month_day_proto.clone(),
                current.plain_month_day_constructor.clone(),
                current.plain_year_month_proto.clone(),
                current.plain_year_month_constructor.clone(),
            )
        };
        let (bigint_proto, bigint_constructor) = if dirty.stubs {
            make_named_pair(string_forge, shape_forge, labels, "BigInt")
        } else {
            (current.bigint_proto.clone(), current.bigint_constructor.clone())
        };
        let stub_objects = if dirty.stubs { Vec::new() } else { current.stub_objects.clone() };
        let console_object = if dirty.console {
            P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null()))
        } else {
            current.console_object.clone()
        };

        // 迭代器原型与资源栈原型依赖 Object.prototype（链到其上）：object 家族重建时
        // 一并重建，否则旧原型链指向已释放的 object_proto。
        let (
            iterator_proto,
            array_iterator_proto,
            map_iterator_proto,
            set_iterator_proto,
            string_iterator_proto,
            regexp_string_iterator_proto,
            iterator_helper_proto,
            disposable_stack_proto,
            async_disposable_stack_proto,
        ) = if dirty.object {
            (
                P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null())),
                P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null())),
                P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null())),
                P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null())),
                P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null())),
                P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null())),
                P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null())),
                P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null())),
                P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null())),
            )
        } else {
            (
                current.iterator_proto.clone(),
                current.array_iterator_proto.clone(),
                current.map_iterator_proto.clone(),
                current.set_iterator_proto.clone(),
                current.string_iterator_proto.clone(),
                current.regexp_string_iterator_proto.clone(),
                current.iterator_helper_proto.clone(),
                current.disposable_stack_proto.clone(),
                current.async_disposable_stack_proto.clone(),
            )
        };

        let world = BuiltinWorld {
            object_proto,
            string_default_iterator: std::cell::Cell::new(if dirty.string {
                std::ptr::null()
            } else {
                current.string_default_iterator.get()
            }),
            array_proto,
            function_proto,
            string_proto,
            number_proto,
            boolean_proto,
            error_proto,
            symbol_proto,
            object_constructor,
            array_constructor,
            function_constructor,
            string_constructor,
            number_constructor,
            boolean_constructor,
            error_constructor,
            symbol_constructor,
            type_error_proto: error_subtypes.type_error_proto,
            reference_error_proto: error_subtypes.reference_error_proto,
            range_error_proto: error_subtypes.range_error_proto,
            syntax_error_proto: error_subtypes.syntax_error_proto,
            uri_error_proto: error_subtypes.uri_error_proto,
            eval_error_proto: error_subtypes.eval_error_proto,
            suppressed_error_proto: error_subtypes.suppressed_error_proto,
            math_object,
            json_object,
            date_constructor,
            date_proto,
            set_constructor,
            set_proto,
            map_constructor,
            map_proto,
            regexp_constructor,
            regexp_proto,
            array_buffer_constructor,
            array_buffer_proto,
            shared_array_buffer_proto,
            shared_array_buffer_constructor,
            atomics_object,
            data_view_constructor,
            data_view_proto,
            typed_array_proto: typed_arrays.typed_array_proto,
            typed_array_constructor: typed_arrays.typed_array_constructor,
            int8array_constructor: typed_arrays.int8array_constructor,
            int8array_proto: typed_arrays.int8array_proto,
            uint8array_constructor: typed_arrays.uint8array_constructor,
            uint8array_proto: typed_arrays.uint8array_proto,
            uint8clampedarray_constructor: typed_arrays.uint8clampedarray_constructor,
            uint8clampedarray_proto: typed_arrays.uint8clampedarray_proto,
            int16array_constructor: typed_arrays.int16array_constructor,
            int16array_proto: typed_arrays.int16array_proto,
            uint16array_constructor: typed_arrays.uint16array_constructor,
            uint16array_proto: typed_arrays.uint16array_proto,
            int32array_constructor: typed_arrays.int32array_constructor,
            int32array_proto: typed_arrays.int32array_proto,
            uint32array_constructor: typed_arrays.uint32array_constructor,
            uint32array_proto: typed_arrays.uint32array_proto,
            float32array_constructor: typed_arrays.float32array_constructor,
            float32array_proto: typed_arrays.float32array_proto,
            float64array_constructor: typed_arrays.float64array_constructor,
            float64array_proto: typed_arrays.float64array_proto,
            bigint64array_constructor: typed_arrays.bigint64array_constructor,
            bigint64array_proto: typed_arrays.bigint64array_proto,
            biguint64array_constructor: typed_arrays.biguint64array_constructor,
            biguint64array_proto: typed_arrays.biguint64array_proto,
            sym_match,
            sym_replace,
            sym_search,
            sym_split,
            sym_iterator,
            sym_to_primitive,
            sym_has_instance,
            sym_match_all,
            sym_async_iterator,
            sym_to_string_tag,
            sym_species,
            sym_async_dispose,
            sym_dispose,
            temporal_object,
            temporal_now_object,
            instant_constructor,
            instant_proto,
            plain_date_constructor,
            plain_date_proto,
            plain_time_constructor,
            plain_time_proto,
            duration_constructor,
            duration_proto,
            zoned_date_time_constructor,
            zoned_date_time_proto,
            plain_date_time_constructor,
            plain_date_time_proto,
            plain_month_day_constructor,
            plain_month_day_proto,
            plain_year_month_constructor,
            plain_year_month_proto,
            bigint_constructor,
            bigint_proto,
            iterator_proto,
            array_iterator_proto,
            map_iterator_proto,
            set_iterator_proto,
            string_iterator_proto,
            regexp_string_iterator_proto,
            iterator_helper_proto,
            disposable_stack_proto,
            async_disposable_stack_proto,
            stub_objects,
            console_object,
            leaked_objects: std::cell::RefCell::new(Vec::new()),
        };
        wire_builtin_world_links(&world);
        world
    }
}
