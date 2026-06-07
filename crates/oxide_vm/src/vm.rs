#![allow(clippy::arc_with_non_send_sync)]

use std::sync::Arc;

use oxide_compiler::compiler::Constant;
use oxide_compiler::module::CompiledModule;
use oxide_compiler::opcode::{self, OpCode};

use crate::coercion;
use crate::native::NativeFn;
use oxide_kernel::builtin::{
    ArrayMethods, ErrorMethods, NumberMethods, ObjectMethods, StringMethods,
};
use oxide_kernel::kernel::{KernelConfig, OxideKernel};
use oxide_kernel::prop_forge::PropTemplate;
use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_types::mem::{Epoch, P};
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

pub struct CallFrame {
    pub return_addr: usize,
    pub n_locals: u8,
    pub n_args: u8,
}

pub struct ForInIter<'bump> {
    pub keys: bumpalo::collections::Vec<'bump, JsValue>,
    pub index: usize,
}

pub struct Vm {
    regs: [JsValue; 256],
    pc: usize,
    bytecode: Vec<opcode::Instr>,
    constants: Vec<JsValue>,
    frames: Vec<CallFrame>,
    pub for_in_iters: Vec<*mut u8>,
    kernel: Arc<OxideKernel>,
    interned_strings: Vec<u32>,
    pub epoch: Epoch,
    pub object_prototype: P<JsObject>,
    pub math_rng_state: u64,
}

pub fn init_kernel_builtins(kernel: &Arc<OxideKernel>) {
    let methods = ObjectMethods {
        keys: crate::builtins::object::object_keys as *const (),
        create: crate::builtins::object::object_create as *const (),
        assign: crate::builtins::object::object_assign as *const (),
        define_property: crate::builtins::object::object_define_property as *const (),
        get_own_property_descriptor: crate::builtins::object::object_get_own_property_descriptor
            as *const (),
    };
    kernel.builtin_world().bind_object_methods(
        &methods,
        kernel.string_forge().as_ref(),
        kernel.shape_forge().as_ref(),
    );

    let global_ptr = kernel.global_object().as_ptr() as *mut JsObject;
    let global = unsafe { &mut *global_ptr };
    let si_obj = kernel.string_forge().intern("Object").0;
    let obj_shape = kernel.shape_forge().make_shape(global.shape_id(), si_obj);
    let obj_val = JsValue::from_js_object(
        kernel.builtin_world().object_constructor.as_ptr() as *mut JsObject
    );
    let cur_count = global.prop_count();
    global.set_shape_id(obj_shape);
    global.set_prop_count(cur_count + 1);
    global.set_prop_expand_heap(cur_count, obj_val);

    let _array_methods = ArrayMethods {
        push: crate::builtins::array::array_push as *const (),
        pop: crate::builtins::array::array_pop as *const (),
        slice: crate::builtins::array::array_slice as *const (),
        splice: crate::builtins::array::array_splice as *const (),
        concat: crate::builtins::array::array_concat as *const (),
        join: crate::builtins::array::array_join as *const (),
        index_of: crate::builtins::array::array_index_of as *const (),
        includes: crate::builtins::array::array_includes as *const (),
        reverse: crate::builtins::array::array_reverse as *const (),
        for_each: crate::builtins::array::array_for_each as *const (),
        map: crate::builtins::array::array_map as *const (),
        filter: crate::builtins::array::array_filter as *const (),
        reduce: crate::builtins::array::array_reduce as *const (),
        find: crate::builtins::array::array_find as *const (),
        some: crate::builtins::array::array_some as *const (),
        every: crate::builtins::array::array_every as *const (),
        flat: crate::builtins::array::array_flat as *const (),
        flat_map: crate::builtins::array::array_flat_map as *const (),
    };

    kernel.builtin_world().bind_array_methods(
        &_array_methods,
        kernel.string_forge().as_ref(),
        kernel.shape_forge().as_ref(),
    );

    let si_arr = kernel.string_forge().intern("Array").0;
    let arr_shape = kernel.shape_forge().make_shape(global.shape_id(), si_arr);
    let arr_val =
        JsValue::from_js_object(kernel.builtin_world().array_constructor.as_ptr() as *mut JsObject);
    let cur_count = global.prop_count();
    global.set_shape_id(arr_shape);
    global.set_prop_count(cur_count + 1);
    global.set_prop_expand_heap(cur_count, arr_val);
    global.bump_generation();

    let error_methods = ErrorMethods {
        error: crate::builtins::error::error_constructor as *const (),
        type_error: crate::builtins::error::type_error_constructor as *const (),
        reference_error: crate::builtins::error::reference_error_constructor as *const (),
        range_error: crate::builtins::error::range_error_constructor as *const (),
        syntax_error: crate::builtins::error::syntax_error_constructor as *const (),
        uri_error: crate::builtins::error::uri_error_constructor as *const (),
        eval_error: crate::builtins::error::eval_error_constructor as *const (),
    };
    kernel.builtin_world().bind_error_methods(
        &error_methods,
        kernel.string_forge().as_ref(),
        kernel.shape_forge().as_ref(),
    );

    let si_err = kernel.string_forge().intern("Error").0;
    let err_shape = kernel.shape_forge().make_shape(global.shape_id(), si_err);
    let err_val =
        JsValue::from_js_object(kernel.builtin_world().error_constructor.as_ptr() as *mut JsObject);
    let cur_count = global.prop_count();
    global.set_shape_id(err_shape);
    global.set_prop_count(cur_count + 1);
    global.set_prop_expand_heap(cur_count, err_val);
    global.bump_generation();

    let string_methods = StringMethods {
        index_of: crate::builtins::string::string_index_of as *const (),
        includes: crate::builtins::string::string_includes as *const (),
        char_at: crate::builtins::string::string_char_at as *const (),
        char_code_at: crate::builtins::string::string_char_code_at as *const (),
        concat: crate::builtins::string::string_concat as *const (),
        slice: crate::builtins::string::string_slice as *const (),
        substring: crate::builtins::string::string_substring as *const (),
        to_upper_case: crate::builtins::string::string_to_upper_case as *const (),
        to_lower_case: crate::builtins::string::string_to_lower_case as *const (),
        trim: crate::builtins::string::string_trim as *const (),
        repeat: crate::builtins::string::string_repeat as *const (),
        pad_start: crate::builtins::string::string_pad_start as *const (),
        pad_end: crate::builtins::string::string_pad_end as *const (),
        starts_with: crate::builtins::string::string_starts_with as *const (),
        ends_with: crate::builtins::string::string_ends_with as *const (),
        split: crate::builtins::string::string_split as *const (),
        replace: crate::builtins::string::string_replace as *const (),
        match_fn: crate::builtins::string::string_match_fn as *const (),
        search: crate::builtins::string::string_search as *const (),
    };
    kernel.builtin_world().bind_string_methods(
        &string_methods,
        kernel.string_forge().as_ref(),
        kernel.shape_forge().as_ref(),
    );

    let si_str = kernel.string_forge().intern("String").0;
    let str_shape = kernel.shape_forge().make_shape(global.shape_id(), si_str);
    let str_val = JsValue::from_js_object(
        kernel.builtin_world().string_constructor.as_ptr() as *mut JsObject
    );
    let cur_count = global.prop_count();
    global.set_shape_id(str_shape);
    global.set_prop_count(cur_count + 1);
    global.set_prop_expand_heap(cur_count, str_val);
    global.bump_generation();

    let number_methods = NumberMethods {
        is_nan: crate::builtins::number::number_is_nan as *const (),
        is_finite: crate::builtins::number::number_is_finite as *const (),
        parse_int: crate::builtins::number::number_parse_int as *const (),
        parse_float: crate::builtins::number::number_parse_float as *const (),
        to_string: crate::builtins::number::number_to_string as *const (),
        to_fixed: crate::builtins::number::number_to_fixed as *const (),
    };
    kernel.builtin_world().bind_number_methods(
        &number_methods,
        kernel.string_forge().as_ref(),
        kernel.shape_forge().as_ref(),
    );

    let si_num = kernel.string_forge().intern("Number").0;
    let num_shape = kernel.shape_forge().make_shape(global.shape_id(), si_num);
    let num_val = JsValue::from_js_object(
        kernel.builtin_world().number_constructor.as_ptr() as *mut JsObject
    );
    let cur_count = global.prop_count();
    global.set_shape_id(num_shape);
    global.set_prop_count(cur_count + 1);
    global.set_prop_expand_heap(cur_count, num_val);
    global.bump_generation();

    {
        let num_ctor_ptr = kernel.builtin_world().number_constructor.as_ptr() as *mut JsObject;
        let num_ctor = unsafe { &mut *num_ctor_ptr };
        num_ctor.set_native_fn(Some(
            crate::builtins::number::number_constructor as *const (),
        ));
        num_ctor.set_native_arg_count(1);
    }

    let pi_fn = crate::builtins::number::number_parse_int as *const ();
    let pf_fn = crate::builtins::number::number_parse_float as *const ();
    let _ = oxide_kernel::builtin::BuiltinWorld::bind_method(
        global,
        kernel.shape_forge().as_ref(),
        kernel.string_forge().as_ref(),
        "parseInt",
        pi_fn,
        1,
    );
    let _ = oxide_kernel::builtin::BuiltinWorld::bind_method(
        global,
        kernel.shape_forge().as_ref(),
        kernel.string_forge().as_ref(),
        "parseFloat",
        pf_fn,
        1,
    );

    let math_ptr = kernel.builtin_world().math_object.as_ptr() as *mut JsObject;
    let math = unsafe { &mut *math_ptr };
    let sf = kernel.string_forge().as_ref();
    let sh = kernel.shape_forge().as_ref();

    let _ = oxide_kernel::builtin::BuiltinWorld::bind_method(
        math,
        sh,
        sf,
        "abs",
        crate::builtins::math::math_abs as *const (),
        1,
    );
    let _ = oxide_kernel::builtin::BuiltinWorld::bind_method(
        math,
        sh,
        sf,
        "acos",
        crate::builtins::math::math_acos as *const (),
        1,
    );
    let _ = oxide_kernel::builtin::BuiltinWorld::bind_method(
        math,
        sh,
        sf,
        "asin",
        crate::builtins::math::math_asin as *const (),
        1,
    );
    let _ = oxide_kernel::builtin::BuiltinWorld::bind_method(
        math,
        sh,
        sf,
        "atan",
        crate::builtins::math::math_atan as *const (),
        1,
    );
    let _ = oxide_kernel::builtin::BuiltinWorld::bind_method(
        math,
        sh,
        sf,
        "atan2",
        crate::builtins::math::math_atan2 as *const (),
        2,
    );
    let _ = oxide_kernel::builtin::BuiltinWorld::bind_method(
        math,
        sh,
        sf,
        "cbrt",
        crate::builtins::math::math_cbrt as *const (),
        1,
    );
    let _ = oxide_kernel::builtin::BuiltinWorld::bind_method(
        math,
        sh,
        sf,
        "ceil",
        crate::builtins::math::math_ceil as *const (),
        1,
    );
    let _ = oxide_kernel::builtin::BuiltinWorld::bind_method(
        math,
        sh,
        sf,
        "cos",
        crate::builtins::math::math_cos as *const (),
        1,
    );
    let _ = oxide_kernel::builtin::BuiltinWorld::bind_method(
        math,
        sh,
        sf,
        "cosh",
        crate::builtins::math::math_cosh as *const (),
        1,
    );
    let _ = oxide_kernel::builtin::BuiltinWorld::bind_method(
        math,
        sh,
        sf,
        "exp",
        crate::builtins::math::math_exp as *const (),
        1,
    );
    let _ = oxide_kernel::builtin::BuiltinWorld::bind_method(
        math,
        sh,
        sf,
        "floor",
        crate::builtins::math::math_floor as *const (),
        1,
    );
    let _ = oxide_kernel::builtin::BuiltinWorld::bind_method(
        math,
        sh,
        sf,
        "hypot",
        crate::builtins::math::math_hypot as *const (),
        2,
    );
    let _ = oxide_kernel::builtin::BuiltinWorld::bind_method(
        math,
        sh,
        sf,
        "imul",
        crate::builtins::math::math_imul as *const (),
        2,
    );
    let _ = oxide_kernel::builtin::BuiltinWorld::bind_method(
        math,
        sh,
        sf,
        "log",
        crate::builtins::math::math_log as *const (),
        1,
    );
    let _ = oxide_kernel::builtin::BuiltinWorld::bind_method(
        math,
        sh,
        sf,
        "log10",
        crate::builtins::math::math_log10 as *const (),
        1,
    );
    let _ = oxide_kernel::builtin::BuiltinWorld::bind_method(
        math,
        sh,
        sf,
        "log2",
        crate::builtins::math::math_log2 as *const (),
        1,
    );
    let _ = oxide_kernel::builtin::BuiltinWorld::bind_method(
        math,
        sh,
        sf,
        "max",
        crate::builtins::math::math_max as *const (),
        2,
    );
    let _ = oxide_kernel::builtin::BuiltinWorld::bind_method(
        math,
        sh,
        sf,
        "min",
        crate::builtins::math::math_min as *const (),
        2,
    );
    let _ = oxide_kernel::builtin::BuiltinWorld::bind_method(
        math,
        sh,
        sf,
        "pow",
        crate::builtins::math::math_pow as *const (),
        2,
    );
    let _ = oxide_kernel::builtin::BuiltinWorld::bind_method(
        math,
        sh,
        sf,
        "random",
        crate::builtins::math::math_random as *const (),
        0,
    );
    let _ = oxide_kernel::builtin::BuiltinWorld::bind_method(
        math,
        sh,
        sf,
        "round",
        crate::builtins::math::math_round as *const (),
        1,
    );
    let _ = oxide_kernel::builtin::BuiltinWorld::bind_method(
        math,
        sh,
        sf,
        "sign",
        crate::builtins::math::math_sign as *const (),
        1,
    );
    let _ = oxide_kernel::builtin::BuiltinWorld::bind_method(
        math,
        sh,
        sf,
        "sin",
        crate::builtins::math::math_sin as *const (),
        1,
    );
    let _ = oxide_kernel::builtin::BuiltinWorld::bind_method(
        math,
        sh,
        sf,
        "sinh",
        crate::builtins::math::math_sinh as *const (),
        1,
    );
    let _ = oxide_kernel::builtin::BuiltinWorld::bind_method(
        math,
        sh,
        sf,
        "sqrt",
        crate::builtins::math::math_sqrt as *const (),
        1,
    );
    let _ = oxide_kernel::builtin::BuiltinWorld::bind_method(
        math,
        sh,
        sf,
        "tan",
        crate::builtins::math::math_tan as *const (),
        1,
    );
    let _ = oxide_kernel::builtin::BuiltinWorld::bind_method(
        math,
        sh,
        sf,
        "tanh",
        crate::builtins::math::math_tanh as *const (),
        1,
    );
    let _ = oxide_kernel::builtin::BuiltinWorld::bind_method(
        math,
        sh,
        sf,
        "trunc",
        crate::builtins::math::math_trunc as *const (),
        1,
    );

    let pi_val = JsValue::float(std::f64::consts::PI);
    {
        let si_pi = sf.intern("PI").0;
        let sh_pi = sh.make_shape(math.shape_id(), si_pi);
        let cur = math.prop_count();
        math.set_shape_id(sh_pi);
        math.set_prop_count(cur + 1);
        math.set_prop_expand_heap(cur, pi_val);
    }

    let si_m = kernel.string_forge().intern("Math").0;
    let m_shape = kernel.shape_forge().make_shape(global.shape_id(), si_m);
    let m_val =
        JsValue::from_js_object(kernel.builtin_world().math_object.as_ptr() as *mut JsObject);
    let cur_count = global.prop_count();
    global.set_shape_id(m_shape);
    global.set_prop_count(cur_count + 1);
    global.set_prop_expand_heap(cur_count, m_val);
    global.bump_generation();
}

impl Vm {
    pub fn new() -> Self {
        let kernel = Arc::new(OxideKernel::new(KernelConfig::minimal()));
        init_kernel_builtins(&kernel);
        Self {
            regs: [JsValue::undefined(); 256],
            pc: 0,
            bytecode: Vec::new(),
            constants: Vec::new(),
            frames: Vec::with_capacity(128),
            for_in_iters: Vec::new(),
            kernel,
            interned_strings: Vec::new(),
            epoch: Epoch::new(),
            object_prototype: P::new(JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null())),
            math_rng_state: 0,
        }
    }

    pub fn with_kernel(kernel: Arc<OxideKernel>) -> Self {
        Self {
            regs: [JsValue::undefined(); 256],
            pc: 0,
            bytecode: Vec::new(),
            constants: Vec::new(),
            frames: Vec::with_capacity(128),
            for_in_iters: Vec::new(),
            kernel: Arc::clone(&kernel),
            interned_strings: Vec::new(),
            epoch: Epoch::new(),
            object_prototype: P::clone(&kernel.builtin_world().object_proto),
            math_rng_state: 0,
        }
    }

    pub fn step_rng(&mut self) {
        if self.math_rng_state == 0 {
            self.math_rng_state = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos() as u64;
        }
        self.math_rng_state = self
            .math_rng_state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
    }

    pub fn math_rng_value(&self) -> f64 {
        (self.math_rng_state >> 33) as f64 / (1u64 << 31) as f64
    }

    pub fn kernel(&self) -> &Arc<OxideKernel> {
        &self.kernel
    }

    pub fn reg(&self, idx: u8) -> JsValue {
        self.regs[idx as usize]
    }

    pub fn set_reg(&mut self, idx: u8, val: JsValue) {
        self.regs[idx as usize] = val;
    }

    pub fn regs_mut(&mut self) -> &mut [JsValue; 256] {
        &mut self.regs
    }

    pub fn epoch_mut(&mut self) -> &mut Epoch {
        &mut self.epoch
    }

    pub fn epoch(&self) -> &Epoch {
        &self.epoch
    }

    pub fn reset(&mut self) {
        self.regs = [JsValue::undefined(); 256];
        self.pc = 0;
        self.bytecode.clear();
        self.constants.clear();
        self.frames.clear();
        self.for_in_iters.clear();
        self.epoch.reset();
        self.interned_strings.clear();
    }

    pub fn intern(&mut self, s: &str) -> JsValue {
        let (idx, hash) = self.kernel.string_forge().intern(s);
        self.interned_strings.push(idx);
        JsValue::string(idx, hash)
    }

    pub fn lookup_str(&self, val: JsValue) -> Option<String> {
        if !val.is_string() {
            return None;
        }
        self.kernel.string_forge().lookup(val.as_string_index())
    }

    fn resolve_property(&self, obj: &JsObject, prop_name_si: u32) -> Option<JsValue> {
        if let Some(offset) = self
            .kernel
            .shape_forge()
            .lookup_offset(obj.shape_id(), prop_name_si)
        {
            return Some(obj.get_prop(offset));
        }
        let mut proto = obj.proto();
        while proto.is_object() {
            let proto_obj = unsafe { &*proto.as_js_object_ptr() };
            if let Some(offset) = self
                .kernel
                .shape_forge()
                .lookup_offset(proto_obj.shape_id(), prop_name_si)
            {
                return Some(proto_obj.get_prop(offset));
            }
            proto = proto_obj.proto();
        }
        None
    }

    fn set_member_prop(
        &mut self,
        obj: &mut JsObject,
        prop_name_si: u32,
        val: JsValue,
    ) -> Result<(), String> {
        if let Some(offset) = self
            .kernel
            .shape_forge()
            .lookup_offset(obj.shape_id(), prop_name_si)
        {
            let new_ext = (obj.shape_id() & 0x00FF_FFFF) | ((offset as u32) << 24);
            self.bytecode[self.pc - 1] = new_ext;
            obj.set_prop(offset, val);
        } else {
            let new_offset = obj.prop_count();
            let new_shape_id = self
                .kernel
                .shape_forge()
                .make_shape(obj.shape_id(), prop_name_si);
            let new_ext = (new_shape_id & 0x00FF_FFFF) | ((new_offset as u32) << 24);
            self.bytecode[self.pc - 1] = new_ext;
            obj.set_shape_id(new_shape_id);
            obj.set_prop_count(new_offset + 1);
            obj.set_prop_expand(new_offset, val, self.epoch.bump());
            obj.bump_generation();
            self.kernel.prop_forge().upsert(
                new_shape_id,
                PropTemplate {
                    shape_id: new_shape_id,
                    offset: new_offset,
                    generation: obj.generation(),
                },
            );
        }
        Ok(())
    }

    pub fn rerun(&mut self) -> Result<JsValue, String> {
        self.pc = 0;
        self.regs = [JsValue::undefined(); 256];
        self.frames.clear();
        self.for_in_iters.clear();
        self.dispatch()
    }

    pub fn run(&mut self, module: &CompiledModule) -> Result<JsValue, String> {
        self.constants = module
            .constants
            .iter()
            .map(|c| match c {
                Constant::Number(v) => JsValue::float(*v),
                Constant::Int(v) => JsValue::int(*v),
                Constant::String(s) => self.intern(s),
                Constant::Boolean(b) => JsValue::bool(*b),
                Constant::Null => JsValue::null(),
                Constant::Undefined => JsValue::undefined(),
            })
            .collect();
        self.bytecode = module.bytecode.clone();
        self.pc = 0;
        self.regs = [JsValue::undefined(); 256];
        self.frames.clear();

        for (name, reg) in &module.builtin_reg_map {
            let si = self.kernel.string_forge().intern(name.as_str()).0;
            let global = self.kernel.global_object();
            let mut shape_id = global.shape_id();
            let mut found: Option<u8> = None;
            while shape_id != EMPTY_SHAPE_ID {
                if let Some(shape) = self.kernel.shape_forge().get_shape(shape_id) {
                    if shape.property_name == si {
                        found = Some(shape.property_offset);
                        break;
                    }
                    shape_id = shape.parent.unwrap_or(EMPTY_SHAPE_ID);
                } else {
                    break;
                }
            }
            if let Some(offset) = found {
                self.regs[*reg as usize] = global.get_prop(offset);
            }
        }

        self.dispatch()
    }

    fn dispatch(&mut self) -> Result<JsValue, String> {
        let mut steps: u64 = 0;
        const MAX_STEPS: u64 = 100_000_000;
        loop {
            steps += 1;
            if steps > MAX_STEPS {
                return Err(format!("VM step limit exceeded at pc={}", self.pc));
            }
            if self.pc >= self.bytecode.len() {
                return Err("program counter out of bounds".into());
            }

            let instr = self.bytecode[self.pc];
            let op = opcode::opcode(instr);
            let rd = opcode::rd(instr) as usize;
            let a = opcode::a(instr) as usize;
            let b = opcode::b(instr) as usize;
            self.pc += 1;

            match op {
                OpCode::NOP => {}

                OpCode::HALT => return Ok(self.regs[0]),

                OpCode::LOAD_CONST => {
                    let idx = opcode::imm16(instr) as usize;
                    if idx < self.constants.len() {
                        self.regs[rd] = self.constants[idx];
                    } else {
                        return Err(format!("constant index {idx} out of bounds"));
                    }
                }

                OpCode::ADD => {
                    let lhs = self.regs[a];
                    let rhs = self.regs[b];
                    if lhs.is_string() || rhs.is_string() {
                        let ls = coercion::to_string(self.kernel.string_forge().as_ref(), lhs);
                        let rs = coercion::to_string(self.kernel.string_forge().as_ref(), rhs);
                        let concat = format!("{ls}{rs}");
                        self.regs[rd] = self.intern(&concat);
                    } else {
                        let ln = coercion::to_number(lhs, self.kernel.string_forge().as_ref());
                        let rn = coercion::to_number(rhs, self.kernel.string_forge().as_ref());
                        self.regs[rd] = JsValue::float(ln + rn);
                    }
                }

                OpCode::SUB => {
                    let l = coercion::to_number(self.regs[a], self.kernel.string_forge().as_ref());
                    let r = coercion::to_number(self.regs[b], self.kernel.string_forge().as_ref());
                    self.regs[rd] = JsValue::float(l - r);
                }

                OpCode::MUL => {
                    let l = coercion::to_number(self.regs[a], self.kernel.string_forge().as_ref());
                    let r = coercion::to_number(self.regs[b], self.kernel.string_forge().as_ref());
                    self.regs[rd] = JsValue::float(l * r);
                }

                OpCode::DIV => {
                    let l = coercion::to_number(self.regs[a], self.kernel.string_forge().as_ref());
                    let r = coercion::to_number(self.regs[b], self.kernel.string_forge().as_ref());
                    self.regs[rd] = JsValue::float(l / r);
                }

                OpCode::MOD => {
                    let l = coercion::to_number(self.regs[a], self.kernel.string_forge().as_ref());
                    let r = coercion::to_number(self.regs[b], self.kernel.string_forge().as_ref());
                    self.regs[rd] = JsValue::float(l % r);
                }

                OpCode::NEG => {
                    let v = coercion::to_number(self.regs[a], self.kernel.string_forge().as_ref());
                    self.regs[rd] = JsValue::float(-v);
                }

                OpCode::EQ => {
                    let eq = coercion::abstract_eq(
                        self.regs[a],
                        self.regs[b],
                        self.kernel.string_forge().as_ref(),
                    );
                    self.regs[rd] = JsValue::bool(eq);
                }

                OpCode::NEQ => {
                    let ne = !coercion::abstract_eq(
                        self.regs[a],
                        self.regs[b],
                        self.kernel.string_forge().as_ref(),
                    );
                    self.regs[rd] = JsValue::bool(ne);
                }

                OpCode::LT => {
                    let rel = coercion::relational_compare(
                        self.kernel.string_forge().as_ref(),
                        self.regs[a],
                        self.regs[b],
                    );
                    self.regs[rd] = JsValue::bool(rel.unwrap_or(false));
                }

                OpCode::GT => {
                    let rel = coercion::relational_compare(
                        self.kernel.string_forge().as_ref(),
                        self.regs[b],
                        self.regs[a],
                    );
                    self.regs[rd] = JsValue::bool(rel.unwrap_or(false));
                }

                OpCode::LTE => {
                    let rel = coercion::relational_compare(
                        self.kernel.string_forge().as_ref(),
                        self.regs[b],
                        self.regs[a],
                    );
                    self.regs[rd] = JsValue::bool(!rel.unwrap_or(true));
                }

                OpCode::GTE => {
                    let rel = coercion::relational_compare(
                        self.kernel.string_forge().as_ref(),
                        self.regs[a],
                        self.regs[b],
                    );
                    self.regs[rd] = JsValue::bool(!rel.unwrap_or(true));
                }

                OpCode::STRICT_EQ => {
                    let eq = coercion::strict_equality(self.regs[a], self.regs[b]);
                    self.regs[rd] = JsValue::bool(eq);
                }

                OpCode::STRICT_NEQ => {
                    let ne = !coercion::strict_equality(self.regs[a], self.regs[b]);
                    self.regs[rd] = JsValue::bool(ne);
                }

                OpCode::UNARY_PLUS => {
                    let v = coercion::to_number(self.regs[a], self.kernel.string_forge().as_ref());
                    self.regs[rd] = JsValue::float(v);
                }

                OpCode::JMP => {
                    let offset = opcode::offset16(instr) as isize;
                    self.pc = ((self.pc as isize) + offset - 1) as usize;
                }

                OpCode::JMP_IF_FALSE => {
                    let cond =
                        coercion::to_boolean(self.regs[rd], self.kernel.string_forge().as_ref());
                    if !cond {
                        let offset = opcode::offset16(instr) as isize;
                        self.pc = ((self.pc as isize) + offset - 1) as usize;
                    }
                }

                OpCode::JMP_IF_TRUE => {
                    let cond =
                        coercion::to_boolean(self.regs[rd], self.kernel.string_forge().as_ref());
                    if cond {
                        let offset = opcode::offset16(instr) as isize;
                        self.pc = ((self.pc as isize) + offset - 1) as usize;
                    }
                }

                OpCode::LOAD_VAR => {
                    self.regs[rd] = self.regs[a];
                }

                OpCode::STORE_VAR => {
                    self.regs[rd] = self.regs[a];
                }

                OpCode::CALL => {
                    let callee_reg = rd;
                    let this_reg = a as u8;
                    let first_arg_reg = b as u8;

                    let callee = self.regs[callee_reg];

                    if callee.is_object() {
                        let obj_ptr = callee.as_js_object_ptr();
                        if !obj_ptr.is_null() {
                            let obj = unsafe { &*obj_ptr };
                            if obj.is_function() && obj.native_fn().is_some() {
                                let ext = self.bytecode[self.pc];
                                self.pc += 1;
                                let arg_count = (ext & 0xFF) as usize;

                                let mut args_buf = [0u8; 257];
                                args_buf[0] = this_reg;
                                for i in 0..arg_count.min(256) {
                                    args_buf[i + 1] = first_arg_reg.wrapping_add(i as u8);
                                }
                                let args_slice = &args_buf[..arg_count + 1];

                                let func: NativeFn =
                                    unsafe { std::mem::transmute(obj.native_fn().unwrap()) };
                                match func(self, args_slice) {
                                    Ok(val) => self.regs[0] = val,
                                    Err(err_val) => {
                                        return Err(format!("Native error: {:?}", err_val));
                                    }
                                }
                                continue;
                            }
                        }
                    }

                    return Err("CALL target is not a native function".into());
                }

                OpCode::RETURN => {
                    let result = self.regs[rd];
                    if let Some(frame) = self.frames.pop() {
                        self.pc = frame.return_addr;
                        self.regs[0] = result;
                    } else {
                        return Ok(result);
                    }
                }

                OpCode::IC_GET_PROP => {
                    let val = self.regs[a];
                    let obj_ptr = val.as_object_ptr() as *mut JsObject;
                    if obj_ptr.is_null() {
                        if val.is_string() {
                            let proto_ptr =
                                self.kernel.builtin_world().string_proto.as_ptr() as *mut JsObject;
                            let proto = unsafe { &*proto_ptr };
                            let prop_name_si = self.regs[b].as_string_index();
                            if let Some(resolved) = self.resolve_property(proto, prop_name_si) {
                                self.regs[a] = resolved;
                                self.pc += 1;
                                continue;
                            }
                        }
                        if val.is_int() || val.is_double() {
                            let proto_ptr =
                                self.kernel.builtin_world().number_proto.as_ptr() as *mut JsObject;
                            let proto = unsafe { &*proto_ptr };
                            let prop_name_si = self.regs[b].as_string_index();
                            if let Some(resolved) = self.resolve_property(proto, prop_name_si) {
                                self.regs[a] = resolved;
                                self.pc += 1;
                                continue;
                            }
                        }
                        return Err("IC_GET_PROP on non-object".into());
                    }
                    let obj = unsafe { &*obj_ptr };
                    let prop_name_si = self.regs[b].as_string_index();
                    let ext = self.bytecode[self.pc];
                    self.pc += 1;
                    let cached_shape_id = ext & 0x00FF_FFFF;
                    let cached_offset = ((ext >> 24) & 0xFF) as u8;

                    if cached_shape_id != 0 && cached_shape_id == obj.shape_id() {
                        self.regs[a] = obj.get_prop(cached_offset);
                    } else if let Some(template) =
                        self.kernel.prop_forge().get_template(obj.shape_id())
                    {
                        let new_ext =
                            (obj.shape_id() & 0x00FF_FFFF) | ((template.offset as u32) << 24);
                        self.bytecode[self.pc - 1] = new_ext;
                        self.regs[a] = obj.get_prop(template.offset);
                    } else if let Some(val) = self.resolve_property(obj, prop_name_si) {
                        let offset = self
                            .kernel
                            .shape_forge()
                            .lookup_offset(obj.shape_id(), prop_name_si)
                            .unwrap_or(0);
                        let new_ext = (obj.shape_id() & 0x00FF_FFFF) | ((offset as u32) << 24);
                        self.bytecode[self.pc - 1] = new_ext;
                        self.regs[a] = val;
                    } else {
                        self.regs[a] = JsValue::undefined();
                    }
                }

                OpCode::IC_SET_PROP => {
                    let obj_ptr = self.regs[rd].as_object_ptr() as *mut JsObject;
                    if obj_ptr.is_null() {
                        return Err("IC_SET_PROP on non-object".into());
                    }
                    let obj = unsafe { &mut *obj_ptr };
                    let prop_name_si = self.regs[b].as_string_index();
                    let ext = self.bytecode[self.pc];
                    self.pc += 1;
                    let cached_shape_id = ext & 0x00FF_FFFF;
                    let cached_offset = ((ext >> 24) & 0xFF) as u8;

                    if cached_shape_id != 0 && cached_shape_id == obj.shape_id() {
                        obj.set_prop(cached_offset, self.regs[a]);
                    } else {
                        if let Some(offset) = self
                            .kernel
                            .shape_forge()
                            .lookup_offset(obj.shape_id(), prop_name_si)
                        {
                            let new_ext = (obj.shape_id() & 0x00FF_FFFF) | ((offset as u32) << 24);
                            self.bytecode[self.pc - 1] = new_ext;
                            obj.set_prop(offset, self.regs[a]);
                        } else {
                            let new_offset = obj.prop_count();
                            let new_shape_id = self
                                .kernel
                                .shape_forge()
                                .make_shape(obj.shape_id(), prop_name_si);
                            obj.set_shape_id(new_shape_id);
                            obj.set_prop_count(new_offset + 1);
                            obj.set_prop(new_offset, self.regs[a]);
                            obj.bump_generation();
                            let new_ext =
                                (new_shape_id & 0x00FF_FFFF) | ((new_offset as u32) << 24);
                            self.bytecode[self.pc - 1] = new_ext;
                            self.kernel.prop_forge().upsert(
                                new_shape_id,
                                PropTemplate {
                                    shape_id: new_shape_id,
                                    offset: new_offset,
                                    generation: obj.generation(),
                                },
                            );
                        }
                    }
                }

                OpCode::GET_PROP => {
                    let obj_ptr = self.regs[rd].as_object_ptr() as *mut JsObject;
                    if obj_ptr.is_null() {
                        return Err("GET_PROP on non-object".into());
                    }
                    let obj = unsafe { &*obj_ptr };
                    let prop_name_si = self.regs[b].as_string_index();
                    self.regs[a] = self
                        .resolve_property(obj, prop_name_si)
                        .unwrap_or(JsValue::undefined());
                }
                OpCode::SET_PROP => {
                    let obj_ptr = self.regs[rd].as_object_ptr() as *mut JsObject;
                    if obj_ptr.is_null() {
                        return Err("SET_PROP on non-object".into());
                    }
                    let obj = unsafe { &mut *obj_ptr };
                    let prop_name_si = self.regs[b].as_string_index();
                    if let Some(offset) = self
                        .kernel
                        .shape_forge()
                        .lookup_offset(obj.shape_id(), prop_name_si)
                    {
                        obj.set_prop(offset, self.regs[a]);
                    } else {
                        let new_offset = obj.prop_count();
                        let new_shape_id = self
                            .kernel
                            .shape_forge()
                            .make_shape(obj.shape_id(), prop_name_si);
                        obj.set_shape_id(new_shape_id);
                        obj.set_prop_count(new_offset + 1);
                        obj.set_prop_expand(new_offset, self.regs[a], self.epoch.bump());
                        obj.bump_generation();
                    }
                }

                OpCode::GET_PROP_DYNAMIC => {
                    let obj_ptr = self.regs[rd].as_object_ptr() as *mut JsObject;
                    if obj_ptr.is_null() {
                        return Err("GET_PROP_DYNAMIC on non-object".into());
                    }
                    let obj = unsafe { &*obj_ptr };
                    let key_val = self.regs[a];
                    if !key_val.is_string() {
                        return Err("GET_PROP_DYNAMIC key not a string".into());
                    }
                    let prop_name_si = key_val.as_string_index();
                    self.regs[b] = self
                        .resolve_property(obj, prop_name_si)
                        .unwrap_or(JsValue::undefined());
                }

                OpCode::SET_PROP_DYNAMIC => {
                    let obj_ptr = self.regs[rd].as_object_ptr() as *mut JsObject;
                    if obj_ptr.is_null() {
                        return Err("SET_PROP_DYNAMIC on non-object".into());
                    }
                    let obj = unsafe { &mut *obj_ptr };
                    let key_val = self.regs[a];
                    if !key_val.is_string() {
                        return Err("SET_PROP_DYNAMIC key not a string".into());
                    }
                    let prop_name_si = key_val.as_string_index();
                    if let Some(offset) = self
                        .kernel
                        .shape_forge()
                        .lookup_offset(obj.shape_id(), prop_name_si)
                    {
                        obj.set_prop(offset, self.regs[b]);
                    } else {
                        let new_offset = obj.prop_count();
                        let new_shape_id = self
                            .kernel
                            .shape_forge()
                            .make_shape(obj.shape_id(), prop_name_si);
                        obj.set_shape_id(new_shape_id);
                        obj.set_prop_count(new_offset + 1);
                        obj.set_prop_expand(new_offset, self.regs[b], self.epoch.bump());
                        obj.bump_generation();
                    }
                }

                OpCode::NEW_OBJECT => {
                    let proto_ptr = &*self.object_prototype as *const JsObject as *mut JsObject;
                    let obj = self.epoch.alloc(JsObject::new_empty(
                        EMPTY_SHAPE_ID,
                        JsValue::from_js_object(proto_ptr),
                    ));
                    self.regs[rd] = JsValue::object(obj as *mut u8);
                }

                OpCode::NEW_ARRAY => {
                    let proto_ptr =
                        self.kernel.builtin_world().array_proto.as_ptr() as *mut JsObject;
                    let n = opcode::imm16(instr) as usize;
                    let bump = self.epoch.bump();
                    let obj = self.epoch.alloc(JsObject::new_array(
                        EMPTY_SHAPE_ID,
                        JsValue::from_js_object(proto_ptr),
                        n,
                        bump,
                    ));
                    self.regs[rd] = JsValue::object(obj as *mut u8);
                }

                OpCode::SET_ELEM => {
                    let obj_ptr = self.regs[rd].as_object_ptr() as *mut JsObject;
                    if obj_ptr.is_null() {
                        return Err("SET_ELEM on non-object".into());
                    }
                    let idx = self.regs[a].as_int() as usize;
                    let obj = unsafe { &mut *obj_ptr };
                    obj.set_prop(idx as u8, self.regs[b]);
                    if idx as u8 >= obj.prop_count() {
                        obj.set_prop_count(idx as u8 + 1);
                    }
                }

                OpCode::COMPOUND_ADD => {
                    let lhs = self.regs[rd];
                    let rhs = self.regs[a];
                    if lhs.is_string() || rhs.is_string() {
                        let ls = coercion::to_string(self.kernel.string_forge().as_ref(), lhs);
                        let rs = coercion::to_string(self.kernel.string_forge().as_ref(), rhs);
                        let concat = format!("{ls}{rs}");
                        self.regs[rd] = self.intern(&concat);
                    } else {
                        let ln = coercion::to_number(lhs, self.kernel.string_forge().as_ref());
                        let rn = coercion::to_number(rhs, self.kernel.string_forge().as_ref());
                        self.regs[rd] = JsValue::float(ln + rn);
                    }
                }

                OpCode::COMPOUND_SUB => {
                    let l = coercion::to_number(self.regs[rd], self.kernel.string_forge().as_ref());
                    let r = coercion::to_number(self.regs[a], self.kernel.string_forge().as_ref());
                    self.regs[rd] = JsValue::float(l - r);
                }

                OpCode::COMPOUND_MUL => {
                    let l = coercion::to_number(self.regs[rd], self.kernel.string_forge().as_ref());
                    let r = coercion::to_number(self.regs[a], self.kernel.string_forge().as_ref());
                    self.regs[rd] = JsValue::float(l * r);
                }

                OpCode::COMPOUND_DIV => {
                    let l = coercion::to_number(self.regs[rd], self.kernel.string_forge().as_ref());
                    let r = coercion::to_number(self.regs[a], self.kernel.string_forge().as_ref());
                    self.regs[rd] = JsValue::float(l / r);
                }

                OpCode::COMPOUND_MOD => {
                    let l = coercion::to_number(self.regs[rd], self.kernel.string_forge().as_ref());
                    let r = coercion::to_number(self.regs[a], self.kernel.string_forge().as_ref());
                    self.regs[rd] = JsValue::float(l % r);
                }

                OpCode::COMPOUND_EXP => {
                    let l = coercion::to_number(self.regs[rd], self.kernel.string_forge().as_ref());
                    let r = coercion::to_number(self.regs[a], self.kernel.string_forge().as_ref());
                    self.regs[rd] = JsValue::float(l.powf(r));
                }

                OpCode::TYPEOF => {
                    let val = self.regs[a];
                    let result = if val.is_undefined() {
                        "undefined"
                    } else if val.is_null() {
                        "object"
                    } else if val.is_bool() {
                        "boolean"
                    } else if val.is_int() || val.is_double() {
                        "number"
                    } else if val.is_string() {
                        "string"
                    } else if val.is_object() {
                        let obj = unsafe { &*val.as_js_object_ptr() };
                        if obj.is_function() {
                            "function"
                        } else {
                            "object"
                        }
                    } else {
                        "undefined"
                    };
                    self.regs[rd] = self.intern(result);
                }

                OpCode::VOID => {
                    self.regs[rd] = JsValue::undefined();
                }

                OpCode::IN => {
                    let key_val = self.regs[a];
                    let obj_ptr = self.regs[b].as_object_ptr() as *mut JsObject;
                    if obj_ptr.is_null() {
                        return Err("IN right-hand side is not an object".into());
                    }
                    let obj = unsafe { &*obj_ptr };
                    let prop_name_si = if key_val.is_string() {
                        key_val.as_string_index()
                    } else {
                        return Err("IN key must be a string".into());
                    };
                    let found = self.resolve_property(obj, prop_name_si).is_some();
                    self.regs[rd] = JsValue::bool(found);
                }

                OpCode::NOT => {
                    let cond =
                        coercion::to_boolean(self.regs[a], self.kernel.string_forge().as_ref());
                    self.regs[rd] = JsValue::bool(!cond);
                }

                OpCode::AND => {
                    let cond =
                        coercion::to_boolean(self.regs[a], self.kernel.string_forge().as_ref());
                    self.regs[rd] = if cond { self.regs[b] } else { self.regs[a] };
                }

                OpCode::OR => {
                    let cond =
                        coercion::to_boolean(self.regs[a], self.kernel.string_forge().as_ref());
                    self.regs[rd] = if cond { self.regs[a] } else { self.regs[b] };
                }

                OpCode::INC_PRE => {
                    let n = coercion::to_number(self.regs[rd], self.kernel.string_forge().as_ref());
                    let result = JsValue::float(n + 1.0);
                    self.regs[rd] = result;
                    self.regs[a] = result;
                }

                OpCode::INC_POST => {
                    let n = coercion::to_number(self.regs[rd], self.kernel.string_forge().as_ref());
                    self.regs[a] = JsValue::float(n);
                    self.regs[rd] = JsValue::float(n + 1.0);
                }

                OpCode::DEC_PRE => {
                    let n = coercion::to_number(self.regs[rd], self.kernel.string_forge().as_ref());
                    let result = JsValue::float(n - 1.0);
                    self.regs[rd] = result;
                    self.regs[a] = result;
                }

                OpCode::DEC_POST => {
                    let n = coercion::to_number(self.regs[rd], self.kernel.string_forge().as_ref());
                    self.regs[a] = JsValue::float(n);
                    self.regs[rd] = JsValue::float(n - 1.0);
                }

                OpCode::MEMBER_INC => {
                    let obj_ptr = self.regs[rd].as_object_ptr() as *mut JsObject;
                    if obj_ptr.is_null() {
                        return Err("MEMBER_INC on non-object".into());
                    }
                    let obj = unsafe { &mut *obj_ptr };
                    let prop_name_si = self.regs[b].as_string_index();
                    let ext = self.bytecode[self.pc];
                    self.pc += 1;
                    let cached_shape_id = ext & 0x00FF_FFFF;
                    let cached_offset = ((ext >> 24) & 0xFF) as u8;

                    let prop_val = if cached_shape_id != 0 && cached_shape_id == obj.shape_id() {
                        obj.get_prop(cached_offset)
                    } else if let Some(template) =
                        self.kernel.prop_forge().get_template(obj.shape_id())
                    {
                        let new_ext =
                            (obj.shape_id() & 0x00FF_FFFF) | ((template.offset as u32) << 24);
                        self.bytecode[self.pc - 1] = new_ext;
                        obj.get_prop(template.offset)
                    } else if let Some(val) = self.resolve_property(obj, prop_name_si) {
                        let offset = self
                            .kernel
                            .shape_forge()
                            .lookup_offset(obj.shape_id(), prop_name_si)
                            .unwrap_or(0);
                        let new_ext = (obj.shape_id() & 0x00FF_FFFF) | ((offset as u32) << 24);
                        self.bytecode[self.pc - 1] = new_ext;
                        val
                    } else {
                        JsValue::undefined()
                    };

                    let n = coercion::to_number(prop_val, self.kernel.string_forge().as_ref());
                    let new_val = JsValue::float(n + 1.0);

                    let offset = if cached_shape_id != 0 && cached_shape_id == obj.shape_id() {
                        cached_offset
                    } else if let Some(off) = self
                        .kernel
                        .shape_forge()
                        .lookup_offset(obj.shape_id(), prop_name_si)
                    {
                        off
                    } else {
                        let new_offset = obj.prop_count();
                        let new_shape_id = self
                            .kernel
                            .shape_forge()
                            .make_shape(obj.shape_id(), prop_name_si);
                        obj.set_shape_id(new_shape_id);
                        obj.set_prop_count(new_offset + 1);
                        obj.set_prop_expand(new_offset, new_val, self.epoch.bump());
                        obj.bump_generation();
                        let new_ext = (new_shape_id & 0x00FF_FFFF) | ((new_offset as u32) << 24);
                        self.bytecode[self.pc - 1] = new_ext;
                        self.kernel.prop_forge().upsert(
                            new_shape_id,
                            PropTemplate {
                                shape_id: new_shape_id,
                                offset: new_offset,
                                generation: obj.generation(),
                            },
                        );
                        self.regs[a] = new_val;
                        continue;
                    };
                    obj.set_prop(offset, new_val);
                    self.regs[a] = new_val;
                }

                OpCode::MEMBER_DEC => {
                    let obj_ptr = self.regs[rd].as_object_ptr() as *mut JsObject;
                    if obj_ptr.is_null() {
                        return Err("MEMBER_DEC on non-object".into());
                    }
                    let obj = unsafe { &mut *obj_ptr };
                    let prop_name_si = self.regs[b].as_string_index();
                    let ext = self.bytecode[self.pc];
                    self.pc += 1;
                    let cached_shape_id = ext & 0x00FF_FFFF;
                    let cached_offset = ((ext >> 24) & 0xFF) as u8;

                    let prop_val = if cached_shape_id != 0 && cached_shape_id == obj.shape_id() {
                        obj.get_prop(cached_offset)
                    } else if let Some(template) =
                        self.kernel.prop_forge().get_template(obj.shape_id())
                    {
                        let new_ext =
                            (obj.shape_id() & 0x00FF_FFFF) | ((template.offset as u32) << 24);
                        self.bytecode[self.pc - 1] = new_ext;
                        obj.get_prop(template.offset)
                    } else if let Some(val) = self.resolve_property(obj, prop_name_si) {
                        let offset = self
                            .kernel
                            .shape_forge()
                            .lookup_offset(obj.shape_id(), prop_name_si)
                            .unwrap_or(0);
                        let new_ext = (obj.shape_id() & 0x00FF_FFFF) | ((offset as u32) << 24);
                        self.bytecode[self.pc - 1] = new_ext;
                        val
                    } else {
                        JsValue::undefined()
                    };

                    let n = coercion::to_number(prop_val, self.kernel.string_forge().as_ref());
                    let new_val = JsValue::float(n - 1.0);

                    let offset = if cached_shape_id != 0 && cached_shape_id == obj.shape_id() {
                        cached_offset
                    } else if let Some(off) = self
                        .kernel
                        .shape_forge()
                        .lookup_offset(obj.shape_id(), prop_name_si)
                    {
                        off
                    } else {
                        let new_offset = obj.prop_count();
                        let new_shape_id = self
                            .kernel
                            .shape_forge()
                            .make_shape(obj.shape_id(), prop_name_si);
                        obj.set_shape_id(new_shape_id);
                        obj.set_prop_count(new_offset + 1);
                        obj.set_prop_expand(new_offset, new_val, self.epoch.bump());
                        obj.bump_generation();
                        let new_ext = (new_shape_id & 0x00FF_FFFF) | ((new_offset as u32) << 24);
                        self.bytecode[self.pc - 1] = new_ext;
                        self.kernel.prop_forge().upsert(
                            new_shape_id,
                            PropTemplate {
                                shape_id: new_shape_id,
                                offset: new_offset,
                                generation: obj.generation(),
                            },
                        );
                        self.regs[a] = new_val;
                        continue;
                    };
                    obj.set_prop(offset, new_val);
                    self.regs[a] = new_val;
                }

                OpCode::DYN_MEMBER_INC => {
                    let obj_ptr = self.regs[rd].as_object_ptr() as *mut JsObject;
                    if obj_ptr.is_null() {
                        return Err("DYN_MEMBER_INC on non-object".into());
                    }
                    let obj = unsafe { &mut *obj_ptr };
                    let key_val = self.regs[a];
                    if !key_val.is_string() {
                        return Err("DYN_MEMBER_INC key not a string".into());
                    }
                    let prop_name_si = key_val.as_string_index();
                    let prop_val = self
                        .resolve_property(obj, prop_name_si)
                        .unwrap_or(JsValue::undefined());
                    let n = coercion::to_number(prop_val, self.kernel.string_forge().as_ref());
                    let new_val = JsValue::float(n + 1.0);

                    if let Some(offset) = self
                        .kernel
                        .shape_forge()
                        .lookup_offset(obj.shape_id(), prop_name_si)
                    {
                        obj.set_prop(offset, new_val);
                    } else {
                        let new_offset = obj.prop_count();
                        let new_shape_id = self
                            .kernel
                            .shape_forge()
                            .make_shape(obj.shape_id(), prop_name_si);
                        obj.set_shape_id(new_shape_id);
                        obj.set_prop_count(new_offset + 1);
                        obj.set_prop_expand(new_offset, new_val, self.epoch.bump());
                        obj.bump_generation();
                    };
                    self.regs[b] = new_val;
                }

                OpCode::DYN_MEMBER_DEC => {
                    let obj_ptr = self.regs[rd].as_object_ptr() as *mut JsObject;
                    if obj_ptr.is_null() {
                        return Err("DYN_MEMBER_DEC on non-object".into());
                    }
                    let obj = unsafe { &mut *obj_ptr };
                    let key_val = self.regs[a];
                    if !key_val.is_string() {
                        return Err("DYN_MEMBER_DEC key not a string".into());
                    }
                    let prop_name_si = key_val.as_string_index();
                    let prop_val = self
                        .resolve_property(obj, prop_name_si)
                        .unwrap_or(JsValue::undefined());
                    let n = coercion::to_number(prop_val, self.kernel.string_forge().as_ref());
                    let new_val = JsValue::float(n - 1.0);

                    if let Some(offset) = self
                        .kernel
                        .shape_forge()
                        .lookup_offset(obj.shape_id(), prop_name_si)
                    {
                        obj.set_prop(offset, new_val);
                    } else {
                        let new_offset = obj.prop_count();
                        let new_shape_id = self
                            .kernel
                            .shape_forge()
                            .make_shape(obj.shape_id(), prop_name_si);
                        obj.set_shape_id(new_shape_id);
                        obj.set_prop_count(new_offset + 1);
                        obj.set_prop_expand(new_offset, new_val, self.epoch.bump());
                        obj.bump_generation();
                    };
                    self.regs[b] = new_val;
                }

                OpCode::COMPOUND_MEMBER_ADD => {
                    let obj_ptr = self.regs[rd].as_object_ptr() as *mut JsObject;
                    if obj_ptr.is_null() {
                        return Err("COMPOUND_MEMBER_ADD on non-object".into());
                    }
                    let obj = unsafe { &mut *obj_ptr };
                    let prop_name_si = self.regs[b].as_string_index();
                    let ext = self.bytecode[self.pc];
                    self.pc += 1;
                    let cached_shape_id = ext & 0x00FF_FFFF;
                    let cached_offset = ((ext >> 24) & 0xFF) as u8;

                    let prop_val = if cached_shape_id != 0 && cached_shape_id == obj.shape_id() {
                        obj.get_prop(cached_offset)
                    } else if let Some(template) =
                        self.kernel.prop_forge().get_template(obj.shape_id())
                    {
                        let new_ext =
                            (obj.shape_id() & 0x00FF_FFFF) | ((template.offset as u32) << 24);
                        self.bytecode[self.pc - 1] = new_ext;
                        obj.get_prop(template.offset)
                    } else if let Some(val) = self.resolve_property(obj, prop_name_si) {
                        let offset = self
                            .kernel
                            .shape_forge()
                            .lookup_offset(obj.shape_id(), prop_name_si)
                            .unwrap_or(0);
                        let new_ext = (obj.shape_id() & 0x00FF_FFFF) | ((offset as u32) << 24);
                        self.bytecode[self.pc - 1] = new_ext;
                        val
                    } else {
                        JsValue::undefined()
                    };

                    let rhs = self.regs[a];
                    let new_val = if prop_val.is_string() || rhs.is_string() {
                        let ls = coercion::to_string(self.kernel.string_forge().as_ref(), prop_val);
                        let rs = coercion::to_string(self.kernel.string_forge().as_ref(), rhs);
                        let concat = format!("{ls}{rs}");
                        self.intern(&concat)
                    } else {
                        let ln = coercion::to_number(prop_val, self.kernel.string_forge().as_ref());
                        let rn = coercion::to_number(rhs, self.kernel.string_forge().as_ref());
                        JsValue::float(ln + rn)
                    };

                    if let Some(offset) = self
                        .kernel
                        .shape_forge()
                        .lookup_offset(obj.shape_id(), prop_name_si)
                    {
                        let new_ext = (obj.shape_id() & 0x00FF_FFFF) | ((offset as u32) << 24);
                        self.bytecode[self.pc - 1] = new_ext;
                        obj.set_prop(offset, new_val);
                    } else {
                        let new_offset = obj.prop_count();
                        let new_shape_id = self
                            .kernel
                            .shape_forge()
                            .make_shape(obj.shape_id(), prop_name_si);
                        let new_ext = (new_shape_id & 0x00FF_FFFF) | ((new_offset as u32) << 24);
                        self.bytecode[self.pc - 1] = new_ext;
                        obj.set_shape_id(new_shape_id);
                        obj.set_prop_count(new_offset + 1);
                        obj.set_prop_expand(new_offset, new_val, self.epoch.bump());
                        obj.bump_generation();
                        self.kernel.prop_forge().upsert(
                            new_shape_id,
                            PropTemplate {
                                shape_id: new_shape_id,
                                offset: new_offset,
                                generation: obj.generation(),
                            },
                        );
                    }
                    self.regs[a] = new_val;
                }

                OpCode::COMPOUND_MEMBER_SUB => {
                    let obj_ptr = self.regs[rd].as_object_ptr() as *mut JsObject;
                    if obj_ptr.is_null() {
                        return Err("COMPOUND_MEMBER_SUB on non-object".into());
                    }
                    let obj = unsafe { &mut *obj_ptr };
                    let prop_name_si = self.regs[b].as_string_index();
                    let ext = self.bytecode[self.pc];
                    self.pc += 1;
                    let cached_shape_id = ext & 0x00FF_FFFF;
                    let cached_offset = ((ext >> 24) & 0xFF) as u8;

                    let prop_val = if cached_shape_id != 0 && cached_shape_id == obj.shape_id() {
                        obj.get_prop(cached_offset)
                    } else if let Some(template) =
                        self.kernel.prop_forge().get_template(obj.shape_id())
                    {
                        let new_ext =
                            (obj.shape_id() & 0x00FF_FFFF) | ((template.offset as u32) << 24);
                        self.bytecode[self.pc - 1] = new_ext;
                        obj.get_prop(template.offset)
                    } else if let Some(val) = self.resolve_property(obj, prop_name_si) {
                        let offset = self
                            .kernel
                            .shape_forge()
                            .lookup_offset(obj.shape_id(), prop_name_si)
                            .unwrap_or(0);
                        let new_ext = (obj.shape_id() & 0x00FF_FFFF) | ((offset as u32) << 24);
                        self.bytecode[self.pc - 1] = new_ext;
                        val
                    } else {
                        JsValue::undefined()
                    };

                    let ln = coercion::to_number(prop_val, self.kernel.string_forge().as_ref());
                    let rn = coercion::to_number(self.regs[a], self.kernel.string_forge().as_ref());
                    let new_val = JsValue::float(ln - rn);
                    self.regs[a] = new_val;
                    if let Some(offset) = self
                        .kernel
                        .shape_forge()
                        .lookup_offset(obj.shape_id(), prop_name_si)
                    {
                        let new_ext = (obj.shape_id() & 0x00FF_FFFF) | ((offset as u32) << 24);
                        self.bytecode[self.pc - 1] = new_ext;
                        obj.set_prop(offset, new_val);
                    } else {
                        let new_offset = obj.prop_count();
                        let new_shape_id = self
                            .kernel
                            .shape_forge()
                            .make_shape(obj.shape_id(), prop_name_si);
                        let new_ext = (new_shape_id & 0x00FF_FFFF) | ((new_offset as u32) << 24);
                        self.bytecode[self.pc - 1] = new_ext;
                        obj.set_shape_id(new_shape_id);
                        obj.set_prop_count(new_offset + 1);
                        obj.set_prop_expand(new_offset, new_val, self.epoch.bump());
                        obj.bump_generation();
                        self.kernel.prop_forge().upsert(
                            new_shape_id,
                            PropTemplate {
                                shape_id: new_shape_id,
                                offset: new_offset,
                                generation: obj.generation(),
                            },
                        );
                    }
                }

                OpCode::COMPOUND_MEMBER_MUL => {
                    let obj_ptr = self.regs[rd].as_object_ptr() as *mut JsObject;
                    if obj_ptr.is_null() {
                        return Err("COMPOUND_MEMBER_MUL on non-object".into());
                    }
                    let obj = unsafe { &mut *obj_ptr };
                    let prop_name_si = self.regs[b].as_string_index();
                    let ext = self.bytecode[self.pc];
                    self.pc += 1;
                    let cached_shape_id = ext & 0x00FF_FFFF;

                    let prop_val = if cached_shape_id != 0 && cached_shape_id == obj.shape_id() {
                        obj.get_prop(((ext >> 24) & 0xFF) as u8)
                    } else if let Some(template) =
                        self.kernel.prop_forge().get_template(obj.shape_id())
                    {
                        let new_ext =
                            (obj.shape_id() & 0x00FF_FFFF) | ((template.offset as u32) << 24);
                        self.bytecode[self.pc - 1] = new_ext;
                        obj.get_prop(template.offset)
                    } else if let Some(val) = self.resolve_property(obj, prop_name_si) {
                        let offset = self
                            .kernel
                            .shape_forge()
                            .lookup_offset(obj.shape_id(), prop_name_si)
                            .unwrap_or(0);
                        let new_ext = (obj.shape_id() & 0x00FF_FFFF) | ((offset as u32) << 24);
                        self.bytecode[self.pc - 1] = new_ext;
                        val
                    } else {
                        JsValue::undefined()
                    };

                    let ln = coercion::to_number(prop_val, self.kernel.string_forge().as_ref());
                    let rn = coercion::to_number(self.regs[a], self.kernel.string_forge().as_ref());
                    let new_val = JsValue::float(ln * rn);
                    self.regs[a] = new_val;
                    self.set_member_prop(obj, prop_name_si, new_val)?;
                }

                OpCode::COMPOUND_MEMBER_DIV => {
                    let obj_ptr = self.regs[rd].as_object_ptr() as *mut JsObject;
                    if obj_ptr.is_null() {
                        return Err("COMPOUND_MEMBER_DIV on non-object".into());
                    }
                    let obj = unsafe { &mut *obj_ptr };
                    let prop_name_si = self.regs[b].as_string_index();
                    let ext = self.bytecode[self.pc];
                    self.pc += 1;
                    let cached_shape_id = ext & 0x00FF_FFFF;

                    let prop_val = if cached_shape_id != 0 && cached_shape_id == obj.shape_id() {
                        obj.get_prop(((ext >> 24) & 0xFF) as u8)
                    } else if let Some(template) =
                        self.kernel.prop_forge().get_template(obj.shape_id())
                    {
                        let new_ext =
                            (obj.shape_id() & 0x00FF_FFFF) | ((template.offset as u32) << 24);
                        self.bytecode[self.pc - 1] = new_ext;
                        obj.get_prop(template.offset)
                    } else if let Some(val) = self.resolve_property(obj, prop_name_si) {
                        let offset = self
                            .kernel
                            .shape_forge()
                            .lookup_offset(obj.shape_id(), prop_name_si)
                            .unwrap_or(0);
                        let new_ext = (obj.shape_id() & 0x00FF_FFFF) | ((offset as u32) << 24);
                        self.bytecode[self.pc - 1] = new_ext;
                        val
                    } else {
                        JsValue::undefined()
                    };

                    let ln = coercion::to_number(prop_val, self.kernel.string_forge().as_ref());
                    let rn = coercion::to_number(self.regs[a], self.kernel.string_forge().as_ref());
                    let new_val = JsValue::float(ln / rn);
                    self.regs[a] = new_val;
                    self.set_member_prop(obj, prop_name_si, new_val)?;
                }

                OpCode::COMPOUND_MEMBER_MOD => {
                    let obj_ptr = self.regs[rd].as_object_ptr() as *mut JsObject;
                    if obj_ptr.is_null() {
                        return Err("COMPOUND_MEMBER_MOD on non-object".into());
                    }
                    let obj = unsafe { &mut *obj_ptr };
                    let prop_name_si = self.regs[b].as_string_index();
                    let ext = self.bytecode[self.pc];
                    self.pc += 1;
                    let cached_shape_id = ext & 0x00FF_FFFF;

                    let prop_val = if cached_shape_id != 0 && cached_shape_id == obj.shape_id() {
                        obj.get_prop(((ext >> 24) & 0xFF) as u8)
                    } else if let Some(template) =
                        self.kernel.prop_forge().get_template(obj.shape_id())
                    {
                        let new_ext =
                            (obj.shape_id() & 0x00FF_FFFF) | ((template.offset as u32) << 24);
                        self.bytecode[self.pc - 1] = new_ext;
                        obj.get_prop(template.offset)
                    } else if let Some(val) = self.resolve_property(obj, prop_name_si) {
                        let offset = self
                            .kernel
                            .shape_forge()
                            .lookup_offset(obj.shape_id(), prop_name_si)
                            .unwrap_or(0);
                        let new_ext = (obj.shape_id() & 0x00FF_FFFF) | ((offset as u32) << 24);
                        self.bytecode[self.pc - 1] = new_ext;
                        val
                    } else {
                        JsValue::undefined()
                    };

                    let ln = coercion::to_number(prop_val, self.kernel.string_forge().as_ref());
                    let rn = coercion::to_number(self.regs[a], self.kernel.string_forge().as_ref());
                    let new_val = JsValue::float(ln % rn);
                    self.regs[a] = new_val;
                    self.set_member_prop(obj, prop_name_si, new_val)?;
                }

                OpCode::COMPOUND_MEMBER_EXP => {
                    let obj_ptr = self.regs[rd].as_object_ptr() as *mut JsObject;
                    if obj_ptr.is_null() {
                        return Err("COMPOUND_MEMBER_EXP on non-object".into());
                    }
                    let obj = unsafe { &mut *obj_ptr };
                    let prop_name_si = self.regs[b].as_string_index();
                    let ext = self.bytecode[self.pc];
                    self.pc += 1;
                    let cached_shape_id = ext & 0x00FF_FFFF;

                    let prop_val = if cached_shape_id != 0 && cached_shape_id == obj.shape_id() {
                        obj.get_prop(((ext >> 24) & 0xFF) as u8)
                    } else if let Some(template) =
                        self.kernel.prop_forge().get_template(obj.shape_id())
                    {
                        let new_ext =
                            (obj.shape_id() & 0x00FF_FFFF) | ((template.offset as u32) << 24);
                        self.bytecode[self.pc - 1] = new_ext;
                        obj.get_prop(template.offset)
                    } else if let Some(val) = self.resolve_property(obj, prop_name_si) {
                        let offset = self
                            .kernel
                            .shape_forge()
                            .lookup_offset(obj.shape_id(), prop_name_si)
                            .unwrap_or(0);
                        let new_ext = (obj.shape_id() & 0x00FF_FFFF) | ((offset as u32) << 24);
                        self.bytecode[self.pc - 1] = new_ext;
                        val
                    } else {
                        JsValue::undefined()
                    };

                    let ln = coercion::to_number(prop_val, self.kernel.string_forge().as_ref());
                    let rn = coercion::to_number(self.regs[a], self.kernel.string_forge().as_ref());
                    let new_val = JsValue::float(ln.powf(rn));
                    self.regs[a] = new_val;
                    self.set_member_prop(obj, prop_name_si, new_val)?;
                }

                OpCode::FOR_IN_INIT => {
                    let obj_val = self.regs[a];
                    if !obj_val.is_object() {
                        return Err("TypeError: for-in right-hand side is not an object".into());
                    }
                    let obj = unsafe { &*obj_val.as_js_object_ptr() };
                    let shape_id = obj.shape_id();

                    let mut keys_vec = bumpalo::collections::Vec::new_in(self.epoch.bump());
                    let mut cursor = Some(shape_id);
                    while let Some(id) = cursor {
                        if id == oxide_kernel::shape_forge::EMPTY_SHAPE_ID {
                            break;
                        }
                        if let Some(shape) = self.kernel.shape_forge().get_shape(id) {
                            if shape.property_name != u32::MAX {
                                let hash = self
                                    .kernel
                                    .string_forge()
                                    .get_hash(shape.property_name)
                                    .unwrap_or(0);
                                keys_vec.push(JsValue::string(shape.property_name, hash));
                            }
                            cursor = shape.parent;
                        } else {
                            break;
                        }
                    }

                    let iter = self.epoch.alloc(ForInIter {
                        keys: keys_vec,
                        index: 0,
                    });
                    self.for_in_iters.push(iter as *mut u8);
                }

                OpCode::FOR_IN_NEXT => {
                    let iter_ptr = self
                        .for_in_iters
                        .last()
                        .copied()
                        .map(|p| p as *mut ForInIter)
                        .unwrap_or(std::ptr::null_mut());
                    if iter_ptr.is_null() {
                        return Err("FOR_IN_NEXT without active iterator".into());
                    }
                    let iter = unsafe { &mut *iter_ptr };
                    if iter.index < iter.keys.len() {
                        self.regs[rd] = iter.keys[iter.index];
                        iter.index += 1;
                    } else {
                        self.regs[rd] = JsValue::undefined();
                    }
                }

                OpCode::FOR_IN_DONE => {
                    let iter_ptr = self
                        .for_in_iters
                        .last()
                        .copied()
                        .map(|p| p as *mut ForInIter)
                        .unwrap_or(std::ptr::null_mut());
                    if iter_ptr.is_null() {
                        self.regs[rd] = JsValue::bool(true);
                    } else {
                        let iter = unsafe { &*iter_ptr };
                        self.regs[rd] = JsValue::bool(iter.index >= iter.keys.len());
                    }
                }

                OpCode::FOR_IN_CLEANUP => {
                    self.for_in_iters.pop();
                }

                _ => {
                    if !op.is_implemented() {
                        return Ok(JsValue::undefined());
                    }
                    return Err(format!("opcode {op} not yet implemented"));
                }
            }
        }
    }
}

impl Default for Vm {
    fn default() -> Self {
        Self::new()
    }
}
