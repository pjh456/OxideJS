#![allow(clippy::arc_with_non_send_sync)]

use std::sync::Arc;

use oxide_compiler::compiler::Constant;
use oxide_compiler::module::CompiledModule;
use oxide_compiler::opcode::{self, OpCode};

use crate::coercion;
use crate::native::NativeFn;
use oxide_kernel::bind_method;
use oxide_kernel::builtin::{
    ArrayMethods, ErrorMethods, FunctionMethods, NumberMethods, ObjectMethods, StringMethods,
};
use oxide_kernel::kernel::{KernelConfig, OxideKernel};
use oxide_kernel::prop_forge::PropTemplate;
use oxide_kernel::shape_forge::EMPTY_SHAPE_ID;
use oxide_types::mem::{Epoch, P};
use oxide_types::object::JsObject;
use oxide_types::value::JsValue;

/// Invoke a native function via CALL_NATIVE dispatch.
/// args[0] = this_reg, args[1..=n] = contiguous arg registers.
#[allow(unused_macros)]
macro_rules! native_call {
    ($vm:ident, $callee:expr, $this_reg:expr, $first_arg_reg:expr, $arg_count:expr) => {{
        let mut args_buf = [0u8; 257];
        args_buf[0] = $this_reg;
        for i in 0..($arg_count as usize).min(256) {
            args_buf[i + 1] = $first_arg_reg.wrapping_add(i as u8);
        }
        let args_slice = &args_buf[..($arg_count as usize).min(256) + 1];
        let func: NativeFn = unsafe { std::mem::transmute($callee.native_fn().unwrap()) };
        $vm.regs[254] = JsValue::from_js_object($callee as *const JsObject as *mut JsObject);
        func($vm, args_slice)
    }};
}

pub struct CallFrame {
    pub return_addr: usize,
    pub n_locals: u8,
    pub n_args: u8,
    pub function_obj_reg: u8,
    pub frame_base: u8,
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
    sub_modules: Vec<CompiledModule>,
    sub_module_constants: Vec<Vec<JsValue>>,
    /// Stack of saved bytecode for nested bytecode calls
    saved_bytecode_stack: Vec<Vec<opcode::Instr>>,
    /// Stack of saved constants for nested bytecode calls
    saved_constants_stack: Vec<Vec<JsValue>>,
}

fn bind_object(kernel: &Arc<OxideKernel>, global: &mut JsObject) {
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

    let si_obj = kernel.string_forge().intern("Object").0;
    let obj_shape = kernel.shape_forge().make_shape(global.shape_id(), si_obj);
    let obj_val = JsValue::from_js_object(
        kernel.builtin_world().object_constructor.as_ptr() as *mut JsObject
    );
    global.set_shape_id(obj_shape);
    global.push_prop(obj_val);

    // Set native constructor on Object
    {
        let obj_ctor_ptr = kernel.builtin_world().object_constructor.as_ptr() as *mut JsObject;
        let obj_ctor = unsafe { &mut *obj_ctor_ptr };
        obj_ctor.set_native_fn(Some(
            crate::builtins::object::object_constructor as *const (),
        ));
        obj_ctor.set_native_arg_count(1);
    }
}

fn bind_array(kernel: &Arc<OxideKernel>, global: &mut JsObject) {
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
    global.set_shape_id(arr_shape);
    global.ensure_hash_props().push(Box::new(arr_val));
    global.bump_generation();

    // Set native constructor on Array
    {
        let arr_ctor_ptr = kernel.builtin_world().array_constructor.as_ptr() as *mut JsObject;
        let arr_ctor = unsafe { &mut *arr_ctor_ptr };
        arr_ctor.set_native_fn(Some(crate::builtins::array::array_constructor as *const ()));
        arr_ctor.set_native_arg_count(1);
    }
}

fn bind_error(kernel: &Arc<OxideKernel>, global: &mut JsObject) {
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
    global.set_shape_id(err_shape);
    global.ensure_hash_props().push(Box::new(err_val));
    global.bump_generation();
}

fn bind_string(kernel: &Arc<OxideKernel>, global: &mut JsObject) {
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
    global.set_shape_id(str_shape);
    global.ensure_hash_props().push(Box::new(str_val));
    global.bump_generation();
}

fn bind_number(kernel: &Arc<OxideKernel>, global: &mut JsObject) {
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
    global.set_shape_id(num_shape);
    global.ensure_hash_props().push(Box::new(num_val));
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
    bind_method!(
        kernel.builtin_world(),
        global,
        kernel.string_forge().as_ref(),
        kernel.shape_forge().as_ref(),
        "parseInt",
        pi_fn,
        1
    );
    bind_method!(
        kernel.builtin_world(),
        global,
        kernel.string_forge().as_ref(),
        kernel.shape_forge().as_ref(),
        "parseFloat",
        pf_fn,
        1
    );
}

fn bind_math(kernel: &Arc<OxideKernel>, global: &mut JsObject) {
    let math_ptr = kernel.builtin_world().math_object.as_ptr() as *mut JsObject;
    let math = unsafe { &mut *math_ptr };

    bind_method!(
        kernel.builtin_world(),
        math,
        kernel.string_forge().as_ref(),
        kernel.shape_forge().as_ref(),
        "abs",
        crate::builtins::math::math_abs,
        1
    );
    bind_method!(
        kernel.builtin_world(),
        math,
        kernel.string_forge().as_ref(),
        kernel.shape_forge().as_ref(),
        "acos",
        crate::builtins::math::math_acos,
        1
    );
    bind_method!(
        kernel.builtin_world(),
        math,
        kernel.string_forge().as_ref(),
        kernel.shape_forge().as_ref(),
        "asin",
        crate::builtins::math::math_asin,
        1
    );
    bind_method!(
        kernel.builtin_world(),
        math,
        kernel.string_forge().as_ref(),
        kernel.shape_forge().as_ref(),
        "atan",
        crate::builtins::math::math_atan,
        1
    );
    bind_method!(
        kernel.builtin_world(),
        math,
        kernel.string_forge().as_ref(),
        kernel.shape_forge().as_ref(),
        "atan2",
        crate::builtins::math::math_atan2,
        2
    );
    bind_method!(
        kernel.builtin_world(),
        math,
        kernel.string_forge().as_ref(),
        kernel.shape_forge().as_ref(),
        "cbrt",
        crate::builtins::math::math_cbrt,
        1
    );
    bind_method!(
        kernel.builtin_world(),
        math,
        kernel.string_forge().as_ref(),
        kernel.shape_forge().as_ref(),
        "ceil",
        crate::builtins::math::math_ceil,
        1
    );
    bind_method!(
        kernel.builtin_world(),
        math,
        kernel.string_forge().as_ref(),
        kernel.shape_forge().as_ref(),
        "cos",
        crate::builtins::math::math_cos,
        1
    );
    bind_method!(
        kernel.builtin_world(),
        math,
        kernel.string_forge().as_ref(),
        kernel.shape_forge().as_ref(),
        "cosh",
        crate::builtins::math::math_cosh,
        1
    );
    bind_method!(
        kernel.builtin_world(),
        math,
        kernel.string_forge().as_ref(),
        kernel.shape_forge().as_ref(),
        "exp",
        crate::builtins::math::math_exp,
        1
    );
    bind_method!(
        kernel.builtin_world(),
        math,
        kernel.string_forge().as_ref(),
        kernel.shape_forge().as_ref(),
        "floor",
        crate::builtins::math::math_floor,
        1
    );
    bind_method!(
        kernel.builtin_world(),
        math,
        kernel.string_forge().as_ref(),
        kernel.shape_forge().as_ref(),
        "hypot",
        crate::builtins::math::math_hypot,
        2
    );
    bind_method!(
        kernel.builtin_world(),
        math,
        kernel.string_forge().as_ref(),
        kernel.shape_forge().as_ref(),
        "imul",
        crate::builtins::math::math_imul,
        2
    );
    bind_method!(
        kernel.builtin_world(),
        math,
        kernel.string_forge().as_ref(),
        kernel.shape_forge().as_ref(),
        "log",
        crate::builtins::math::math_log,
        1
    );
    bind_method!(
        kernel.builtin_world(),
        math,
        kernel.string_forge().as_ref(),
        kernel.shape_forge().as_ref(),
        "log10",
        crate::builtins::math::math_log10,
        1
    );
    bind_method!(
        kernel.builtin_world(),
        math,
        kernel.string_forge().as_ref(),
        kernel.shape_forge().as_ref(),
        "log2",
        crate::builtins::math::math_log2,
        1
    );
    bind_method!(
        kernel.builtin_world(),
        math,
        kernel.string_forge().as_ref(),
        kernel.shape_forge().as_ref(),
        "max",
        crate::builtins::math::math_max,
        2
    );
    bind_method!(
        kernel.builtin_world(),
        math,
        kernel.string_forge().as_ref(),
        kernel.shape_forge().as_ref(),
        "min",
        crate::builtins::math::math_min,
        2
    );
    bind_method!(
        kernel.builtin_world(),
        math,
        kernel.string_forge().as_ref(),
        kernel.shape_forge().as_ref(),
        "pow",
        crate::builtins::math::math_pow,
        2
    );
    bind_method!(
        kernel.builtin_world(),
        math,
        kernel.string_forge().as_ref(),
        kernel.shape_forge().as_ref(),
        "random",
        crate::builtins::math::math_random,
        0
    );
    bind_method!(
        kernel.builtin_world(),
        math,
        kernel.string_forge().as_ref(),
        kernel.shape_forge().as_ref(),
        "round",
        crate::builtins::math::math_round,
        1
    );
    bind_method!(
        kernel.builtin_world(),
        math,
        kernel.string_forge().as_ref(),
        kernel.shape_forge().as_ref(),
        "sign",
        crate::builtins::math::math_sign,
        1
    );
    bind_method!(
        kernel.builtin_world(),
        math,
        kernel.string_forge().as_ref(),
        kernel.shape_forge().as_ref(),
        "sin",
        crate::builtins::math::math_sin,
        1
    );
    bind_method!(
        kernel.builtin_world(),
        math,
        kernel.string_forge().as_ref(),
        kernel.shape_forge().as_ref(),
        "sinh",
        crate::builtins::math::math_sinh,
        1
    );
    bind_method!(
        kernel.builtin_world(),
        math,
        kernel.string_forge().as_ref(),
        kernel.shape_forge().as_ref(),
        "sqrt",
        crate::builtins::math::math_sqrt,
        1
    );
    bind_method!(
        kernel.builtin_world(),
        math,
        kernel.string_forge().as_ref(),
        kernel.shape_forge().as_ref(),
        "tan",
        crate::builtins::math::math_tan,
        1
    );
    bind_method!(
        kernel.builtin_world(),
        math,
        kernel.string_forge().as_ref(),
        kernel.shape_forge().as_ref(),
        "tanh",
        crate::builtins::math::math_tanh,
        1
    );
    bind_method!(
        kernel.builtin_world(),
        math,
        kernel.string_forge().as_ref(),
        kernel.shape_forge().as_ref(),
        "trunc",
        crate::builtins::math::math_trunc,
        1
    );
    bind_method!(
        kernel.builtin_world(),
        math,
        kernel.string_forge().as_ref(),
        kernel.shape_forge().as_ref(),
        "acosh",
        crate::builtins::math::math_acosh,
        1
    );
    bind_method!(
        kernel.builtin_world(),
        math,
        kernel.string_forge().as_ref(),
        kernel.shape_forge().as_ref(),
        "asinh",
        crate::builtins::math::math_asinh,
        1
    );
    bind_method!(
        kernel.builtin_world(),
        math,
        kernel.string_forge().as_ref(),
        kernel.shape_forge().as_ref(),
        "atanh",
        crate::builtins::math::math_atanh,
        1
    );
    bind_method!(
        kernel.builtin_world(),
        math,
        kernel.string_forge().as_ref(),
        kernel.shape_forge().as_ref(),
        "clz32",
        crate::builtins::math::math_clz32,
        1
    );
    bind_method!(
        kernel.builtin_world(),
        math,
        kernel.string_forge().as_ref(),
        kernel.shape_forge().as_ref(),
        "expm1",
        crate::builtins::math::math_expm1,
        1
    );
    bind_method!(
        kernel.builtin_world(),
        math,
        kernel.string_forge().as_ref(),
        kernel.shape_forge().as_ref(),
        "fround",
        crate::builtins::math::math_fround,
        1
    );
    bind_method!(
        kernel.builtin_world(),
        math,
        kernel.string_forge().as_ref(),
        kernel.shape_forge().as_ref(),
        "log1p",
        crate::builtins::math::math_log1p,
        1
    );

    for (name, val) in [
        ("PI", std::f64::consts::PI),
        ("E", std::f64::consts::E),
        ("LN10", std::f64::consts::LN_10),
        ("LN2", std::f64::consts::LN_2),
        ("LOG10E", std::f64::consts::LOG10_E),
        ("LOG2E", std::f64::consts::LOG2_E),
        ("SQRT1_2", std::f64::consts::FRAC_1_SQRT_2),
        ("SQRT2", std::f64::consts::SQRT_2),
    ] {
        let si = kernel.string_forge().as_ref().intern(name).0;
        let sh_c = kernel
            .shape_forge()
            .as_ref()
            .make_shape(math.shape_id(), si);
        math.set_shape_id(sh_c);
        math.ensure_hash_props().push(Box::new(JsValue::float(val)));
    }

    let si_m = kernel.string_forge().intern("Math").0;
    let m_shape = kernel.shape_forge().make_shape(global.shape_id(), si_m);
    let m_val =
        JsValue::from_js_object(kernel.builtin_world().math_object.as_ptr() as *mut JsObject);
    global.set_shape_id(m_shape);
    global.ensure_hash_props().push(Box::new(m_val));
    global.bump_generation();
}

fn bind_function(kernel: &Arc<OxideKernel>) {
    let function_methods = FunctionMethods {
        call: crate::builtins::function::function_call as *const (),
        apply: crate::builtins::function::function_apply as *const (),
        bind: crate::builtins::function::function_bind as *const (),
    };
    kernel.builtin_world().bind_function_methods(
        &function_methods,
        kernel.string_forge().as_ref(),
        kernel.shape_forge().as_ref(),
    );
}

pub fn init_kernel_builtins(kernel: &Arc<OxideKernel>) {
    let global_ptr = kernel.global_object().as_ptr() as *mut JsObject;
    let global = unsafe { &mut *global_ptr };

    bind_object(kernel, global);
    bind_array(kernel, global);
    bind_error(kernel, global);
    bind_string(kernel, global);
    bind_number(kernel, global);
    bind_math(kernel, global);
    bind_function(kernel);
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
            sub_modules: Vec::new(),
            sub_module_constants: Vec::new(),
            saved_bytecode_stack: Vec::new(),
            saved_constants_stack: Vec::new(),
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
            sub_modules: Vec::new(),
            sub_module_constants: Vec::new(),
            saved_bytecode_stack: Vec::new(),
            saved_constants_stack: Vec::new(),
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

    /// Create a function JsObject for a BytecodeFunc constant.
    fn create_function_object(&mut self, sub_idx: u32) -> JsValue {
        let func_proto_ptr = self.kernel.builtin_world().function_proto.as_ptr() as *mut JsObject;
        let proto_val = JsValue::from_js_object(func_proto_ptr);
        let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, proto_val);
        obj.set_function(true);
        obj.set_sub_module_index(sub_idx);
        let obj_ptr = self.epoch.alloc(obj);
        JsValue::object(obj_ptr as *mut u8)
    }

    /// Convert a CompiledModule's constants into JsValue vec.
    fn convert_constants(&mut self, module: &CompiledModule) -> Vec<JsValue> {
        module
            .constants
            .iter()
            .map(|c| match c {
                Constant::Number(v) => JsValue::float(*v),
                Constant::Int(v) => JsValue::int(*v),
                Constant::String(s) => self.intern(s),
                Constant::Boolean(b) => JsValue::bool(*b),
                Constant::Null => JsValue::null(),
                Constant::Undefined => JsValue::undefined(),
                Constant::BytecodeFunc(idx) => self.create_function_object(*idx),
            })
            .collect()
    }

    pub fn lookup_str(&self, val: JsValue) -> Option<String> {
        if !val.is_string() {
            return None;
        }
        self.kernel.string_forge().lookup(val.as_string_index())
    }

    fn template_prop_ptr(&self, obj: &JsObject, template: &PropTemplate) -> Option<*const JsValue> {
        let pos = template.position as usize;
        obj.hash_props_vec().and_then(|vec| {
            if pos < vec.len() {
                Some(&*vec[pos] as *const JsValue)
            } else {
                None
            }
        })
    }

    fn resolve_property(&self, obj: &JsObject, prop_name_si: u32) -> Option<JsValue> {
        if let Some(pos) = self
            .kernel
            .shape_forge()
            .lookup_position(obj.shape_id(), prop_name_si)
        {
            let val = obj.get_prop_at(pos);
            if !val.is_undefined() || obj.prop_vec_len() > pos as usize {
                return Some(val);
            }
        }
        let mut proto = obj.proto();
        while proto.is_object() {
            let proto_obj = unsafe { &*proto.as_js_object_ptr() };
            if let Some(pos) = self
                .kernel
                .shape_forge()
                .lookup_position(proto_obj.shape_id(), prop_name_si)
            {
                let val = proto_obj.get_prop_at(pos);
                if !val.is_undefined() || proto_obj.prop_vec_len() > pos as usize {
                    return Some(val);
                }
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
        if let Some(pos) = self
            .kernel
            .shape_forge()
            .lookup_position(obj.shape_id(), prop_name_si)
        {
            obj.set_prop_at(pos, val);
            // IC write-back: 3 extension words (shape_id + ptr_lo + ptr_hi)
            if let Some(ptr) = obj.prop_ptr_at(pos) {
                self.bytecode[self.pc - 3] = obj.shape_id() & 0x00FF_FFFF;
                self.bytecode[self.pc - 2] = ptr as u32;
                self.bytecode[self.pc - 1] = (ptr as u64 >> 32) as u32;
            }
        } else {
            let new_shape_id = self
                .kernel
                .shape_forge()
                .make_shape(obj.shape_id(), prop_name_si);
            obj.set_shape_id(new_shape_id);
            let new_pos = obj.push_prop(val);
            obj.bump_generation();
            if let Some(ptr) = obj.prop_ptr_at(new_pos) {
                self.bytecode[self.pc - 3] = new_shape_id & 0x00FF_FFFF;
                self.bytecode[self.pc - 2] = ptr as u32;
                self.bytecode[self.pc - 1] = (ptr as u64 >> 32) as u32;
                self.kernel.prop_forge().upsert(
                    new_shape_id,
                    PropTemplate {
                        shape_id: new_shape_id,
                        position: new_pos,
                        generation: obj.generation(),
                    },
                );
            }
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
        self.constants = self.convert_constants(module);
        self.sub_modules = module.sub_modules.clone();
        self.sub_module_constants = vec![Vec::new(); self.sub_modules.len()];
        self.bytecode = module.bytecode.clone();
        self.pc = 0;
        self.regs = [JsValue::undefined(); 256];
        self.frames.clear();
        self.saved_bytecode_stack.clear();
        self.saved_constants_stack.clear();

        for (name, reg) in &module.builtin_reg_map {
            let si = self.kernel.string_forge().intern(name.as_str()).0;
            let global = self.kernel.global_object();
            if let Some(pos) = self
                .kernel
                .shape_forge()
                .lookup_position(global.shape_id(), si)
            {
                self.regs[*reg as usize] = global.get_prop_at(pos);
            }
        }

        self.dispatch()
    }

    /// Call a bytecode function from native code (D-09).
    /// Stub: sub_module storage not yet wired (plan 12.1-03).
    #[allow(dead_code)]
    pub fn call_bytecode_func(
        &mut self,
        _callback_obj: &JsObject,
        _args_regs: &[u8],
    ) -> Result<JsValue, String> {
        Err("bytecode function calls not yet supported".into())
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
                            if obj.is_function() {
                                if obj.native_fn().is_some() {
                                    // Native call - use extension word for arg_count
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
                                    self.regs[254] = callee;
                                    match func(self, args_slice) {
                                        Ok(val) => self.regs[0] = val,
                                        Err(err_val) => {
                                            return Err(format!("Native error: {:?}", err_val));
                                        }
                                    }
                                    continue;
                                } else if obj.sub_module_index() > 0 {
                                    // 1-indexed sub_module_index (0 = not a bytecode function)
                                    let sub_idx = obj.sub_module_index() as usize - 1;
                                    if sub_idx >= self.sub_modules.len() {
                                        return Err(format!(
                                            "CALL: sub_module_index {} out of bounds (max {})",
                                            sub_idx,
                                            self.sub_modules.len()
                                        ));
                                    }

                                    // Clone sub_module data before mutably borrowing self
                                    let sub_bytecode = self.sub_modules[sub_idx].bytecode.clone();
                                    let sub_n_args = self.sub_modules[sub_idx].n_args as usize;
                                    let sub_constants = self.sub_modules[sub_idx].constants.clone();
                                    let sub_builtin_count =
                                        self.sub_modules[sub_idx].builtin_reg_map.len();

                                    // Copy args to regs[sub_builtin_count..sub_builtin_count+n_args]
                                    // (builtins occupy regs[0..sub_builtin_count-1])
                                    for i in 0..sub_n_args {
                                        let src_reg = first_arg_reg.wrapping_add(i as u8) as usize;
                                        self.regs[sub_builtin_count + i] = self.regs[src_reg];
                                    }
                                    // Set this (reg 254 convention)
                                    self.regs[254] = callee;

                                    // Save current bytecode/constants
                                    self.saved_bytecode_stack
                                        .push(std::mem::take(&mut self.bytecode));
                                    self.saved_constants_stack
                                        .push(std::mem::take(&mut self.constants));

                                    // Push call frame
                                    self.frames.push(CallFrame {
                                        return_addr: self.pc,
                                        n_locals: sub_n_args as u8,
                                        n_args: sub_n_args as u8,
                                        function_obj_reg: callee_reg as u8,
                                        frame_base: sub_builtin_count as u8,
                                    });

                                    // Convert sub_module constants
                                    self.bytecode = sub_bytecode;
                                    self.constants = sub_constants
                                        .iter()
                                        .map(|c| match c {
                                            Constant::Number(v) => JsValue::float(*v),
                                            Constant::Int(v) => JsValue::int(*v),
                                            Constant::String(s) => self.intern(s),
                                            Constant::Boolean(b) => JsValue::bool(*b),
                                            Constant::Null => JsValue::null(),
                                            Constant::Undefined => JsValue::undefined(),
                                            Constant::BytecodeFunc(idx) => {
                                                self.create_function_object(*idx)
                                            }
                                        })
                                        .collect();

                                    // Pre-fill builtin registers from the global object
                                    // (same as Vm::run does for the main module).
                                    for (name, reg) in &self.sub_modules[sub_idx].builtin_reg_map {
                                        let si = self.kernel.string_forge().intern(name.as_str()).0;
                                        let global = self.kernel.global_object();
                                        if let Some(pos) = self
                                            .kernel
                                            .shape_forge()
                                            .lookup_position(global.shape_id(), si)
                                        {
                                            self.regs[*reg as usize] = global.get_prop_at(pos);
                                        }
                                    }

                                    self.pc = 0;
                                    continue;
                                }
                            }
                        }
                    }

                    return Err("TypeError: CALL target is not callable".into());
                }

                OpCode::CALL_NATIVE => {
                    let callee_reg = rd;
                    let this_reg = a as u8;
                    let first_arg_reg = b as u8;

                    let callee = self.regs[callee_reg];

                    if !callee.is_object() {
                        return Err("TypeError: CALL_NATIVE target is not an object".into());
                    }
                    let obj_ptr = callee.as_js_object_ptr();
                    if obj_ptr.is_null() {
                        return Err("TypeError: CALL_NATIVE target is null".into());
                    }
                    let obj = unsafe { &*obj_ptr };
                    if !obj.is_function() || obj.native_fn().is_none() {
                        return Err("TypeError: CALL_NATIVE target is not a native function".into());
                    }

                    let ext = self.bytecode[self.pc];
                    self.pc += 1;
                    let arg_count = (ext & 0xFF) as usize;

                    let mut args_buf = [0u8; 257];
                    args_buf[0] = this_reg;
                    for i in 0..arg_count.min(256) {
                        args_buf[i + 1] = first_arg_reg.wrapping_add(i as u8);
                    }
                    let args_slice = &args_buf[..arg_count + 1];

                    let func: NativeFn = unsafe { std::mem::transmute(obj.native_fn().unwrap()) };
                    self.regs[254] = callee;
                    match func(self, args_slice) {
                        Ok(val) => self.regs[0] = val,
                        Err(err_val) => {
                            return Err(format!("Native error: {:?}", err_val));
                        }
                    }
                }

                OpCode::NEW_EXPRESSION => {
                    let constructor_reg = a as usize;
                    let first_arg_reg = b as u8;

                    let constructor = self.regs[constructor_reg];
                    if !constructor.is_object() {
                        return Err(
                            "TypeError: NEW_EXPRESSION: constructor is not an object".into()
                        );
                    }
                    let ctor_ptr = constructor.as_js_object_ptr();
                    if ctor_ptr.is_null() {
                        return Err("TypeError: NEW_EXPRESSION: constructor is null".into());
                    }
                    let ctor_obj = unsafe { &*ctor_ptr };
                    if !ctor_obj.is_function() {
                        return Err(
                            "TypeError: NEW_EXPRESSION: constructor is not a function".into()
                        );
                    }

                    // Read extension word for arg_count
                    let ext = self.bytecode[self.pc];
                    self.pc += 1;
                    let arg_count = (ext & 0xFF) as usize;

                    // Create new empty object
                    let proto_ptr = &*self.object_prototype as *const JsObject as *mut JsObject;
                    let new_obj = self.epoch.alloc(JsObject::new_empty(
                        EMPTY_SHAPE_ID,
                        JsValue::from_js_object(proto_ptr),
                    ));

                    // Look up constructor.prototype and set as proto of new object
                    let proto_si = self.kernel.string_forge().intern("prototype").0;
                    if let Some(proto_val) = self.resolve_property(ctor_obj, proto_si) {
                        if proto_val.is_object() {
                            let new_obj_mut = unsafe { &mut *new_obj };
                            let proto_obj_ptr = proto_val.as_js_object_ptr();
                            let _ = new_obj_mut.set_proto(JsValue::from_js_object(proto_obj_ptr));
                        }
                    }

                    // If constructor has native_fn, call it with this=new_obj
                    if ctor_obj.native_fn().is_some() {
                        let new_obj_val = JsValue::object(new_obj as *mut u8);
                        self.regs[255] = new_obj_val;

                        let mut args_buf = [0u8; 257];
                        args_buf[0] = 255u8;
                        for i in 0..arg_count.min(256) {
                            args_buf[i + 1] = first_arg_reg.wrapping_add(i as u8);
                        }
                        let args_slice = &args_buf[..arg_count + 1];

                        let func: NativeFn =
                            unsafe { std::mem::transmute(ctor_obj.native_fn().unwrap()) };
                        self.regs[254] = constructor;
                        match func(self, args_slice) {
                            Ok(val) => {
                                if val.is_object() {
                                    self.regs[rd] = val;
                                } else {
                                    self.regs[rd] = new_obj_val;
                                }
                            }
                            Err(err_val) => {
                                return Err(format!("Native constructor error: {:?}", err_val));
                            }
                        }
                    } else {
                        // Bytecode constructor - wired in plan 12.1-03
                        return Err(
                            "NEW_EXPRESSION: bytecode constructors not yet supported".into()
                        );
                    }
                }

                OpCode::RETURN => {
                    let result = self.regs[rd];
                    if let Some(frame) = self.frames.pop() {
                        // Restore caller's saved bytecode and constants
                        if let Some(saved_bc) = self.saved_bytecode_stack.pop() {
                            self.bytecode = saved_bc;
                        }
                        if let Some(saved_consts) = self.saved_constants_stack.pop() {
                            self.constants = saved_consts;
                        }
                        // Restore caller's pc and merge result into caller's regs[0].
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
                                self.pc += 3;
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
                                self.pc += 3;
                                continue;
                            }
                        }
                        return Err("TypeError: IC_GET_PROP on non-object".into());
                    }
                    let obj = unsafe { &*obj_ptr };
                    let prop_name_si = self.regs[b].as_string_index();
                    let ext0 = self.bytecode[self.pc];
                    let ext1 = self.bytecode[self.pc + 1];
                    let ext2 = self.bytecode[self.pc + 2];
                    self.pc += 3;
                    let cached_shape_id = ext0 & 0x00FF_FFFF;
                    let cached_ptr = ((ext2 as u64) << 32) | (ext1 as u64);

                    if cached_shape_id != 0 && cached_shape_id == obj.shape_id() && cached_ptr != 0
                    {
                        self.regs[a] = unsafe { *(cached_ptr as *const JsValue) };
                    } else if let Some(template) =
                        self.kernel.prop_forge().get_template(obj.shape_id())
                    {
                        let pos = template.position as usize;
                        if let Some(vec) = obj.hash_props_vec() {
                            if pos < vec.len() {
                                let ptr = &*vec[pos] as *const JsValue;
                                self.bytecode[self.pc - 3] = obj.shape_id() & 0x00FF_FFFF;
                                self.bytecode[self.pc - 2] = ptr as u32;
                                self.bytecode[self.pc - 1] = (ptr as u64 >> 32) as u32;
                                self.regs[a] = unsafe { *(ptr) };
                            } else {
                                self.regs[a] = self
                                    .resolve_property(obj, prop_name_si)
                                    .unwrap_or(JsValue::undefined());
                            }
                        } else {
                            self.regs[a] = self
                                .resolve_property(obj, prop_name_si)
                                .unwrap_or(JsValue::undefined());
                        }
                    } else if let Some(val) = self.resolve_property(obj, prop_name_si) {
                        let pos = self
                            .kernel
                            .shape_forge()
                            .lookup_position(obj.shape_id(), prop_name_si)
                            .unwrap_or(0);
                        if let Some(ptr) = obj.prop_ptr_at(pos) {
                            self.bytecode[self.pc - 3] = obj.shape_id() & 0x00FF_FFFF;
                            self.bytecode[self.pc - 2] = ptr as u32;
                            self.bytecode[self.pc - 1] = (ptr as u64 >> 32) as u32;
                        }
                        self.regs[a] = val;
                    } else {
                        self.regs[a] = JsValue::undefined();
                    }
                }

                OpCode::IC_SET_PROP => {
                    let obj_ptr = self.regs[rd].as_object_ptr() as *mut JsObject;
                    if obj_ptr.is_null() {
                        return Err("TypeError: IC_SET_PROP on non-object".into());
                    }
                    let obj = unsafe { &mut *obj_ptr };
                    let prop_name_si = self.regs[b].as_string_index();
                    let ext0 = self.bytecode[self.pc];
                    let ext1 = self.bytecode[self.pc + 1];
                    let ext2 = self.bytecode[self.pc + 2];
                    self.pc += 3;
                    let cached_shape_id = ext0 & 0x00FF_FFFF;
                    let cached_ptr = ((ext2 as u64) << 32) | (ext1 as u64);

                    if cached_shape_id != 0 && cached_shape_id == obj.shape_id() && cached_ptr != 0
                    {
                        unsafe {
                            *(cached_ptr as *mut JsValue) = self.regs[a];
                        }
                    } else {
                        if let Some(pos) = self
                            .kernel
                            .shape_forge()
                            .lookup_position(obj.shape_id(), prop_name_si)
                        {
                            obj.set_prop_at(pos, self.regs[a]);
                            if let Some(ptr) = obj.prop_ptr_at(pos) {
                                self.bytecode[self.pc - 3] = obj.shape_id() & 0x00FF_FFFF;
                                self.bytecode[self.pc - 2] = ptr as u32;
                                self.bytecode[self.pc - 1] = (ptr as u64 >> 32) as u32;
                            }
                        } else {
                            let new_shape_id = self
                                .kernel
                                .shape_forge()
                                .make_shape(obj.shape_id(), prop_name_si);
                            obj.set_shape_id(new_shape_id);
                            let new_pos = obj.push_prop(self.regs[a]);
                            obj.bump_generation();
                            if let Some(ptr) = obj.prop_ptr_at(new_pos) {
                                self.bytecode[self.pc - 3] = new_shape_id & 0x00FF_FFFF;
                                self.bytecode[self.pc - 2] = ptr as u32;
                                self.bytecode[self.pc - 1] = (ptr as u64 >> 32) as u32;
                                self.kernel.prop_forge().upsert(
                                    new_shape_id,
                                    PropTemplate {
                                        shape_id: new_shape_id,
                                        position: new_pos,
                                        generation: obj.generation(),
                                    },
                                );
                            }
                        }
                    }
                }

                OpCode::GET_PROP => {
                    let obj_ptr = self.regs[rd].as_object_ptr() as *mut JsObject;
                    if obj_ptr.is_null() {
                        return Err("TypeError: GET_PROP on non-object".into());
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
                        return Err("TypeError: SET_PROP on non-object".into());
                    }
                    let obj = unsafe { &mut *obj_ptr };
                    let prop_name_si = self.regs[b].as_string_index();
                    if let Some(pos) = self
                        .kernel
                        .shape_forge()
                        .lookup_position(obj.shape_id(), prop_name_si)
                    {
                        obj.set_prop_at(pos, self.regs[a]);
                    } else {
                        let new_shape_id = self
                            .kernel
                            .shape_forge()
                            .make_shape(obj.shape_id(), prop_name_si);
                        obj.set_shape_id(new_shape_id);
                        obj.push_prop(self.regs[a]);
                        obj.bump_generation();
                    }
                }

                OpCode::GET_PROP_DYNAMIC => {
                    let obj_ptr = self.regs[rd].as_object_ptr() as *mut JsObject;
                    if obj_ptr.is_null() {
                        return Err("TypeError: GET_PROP_DYNAMIC on non-object".into());
                    }
                    let obj = unsafe { &*obj_ptr };
                    let key_val = self.regs[a];
                    if !key_val.is_string() {
                        return Err("TypeError: GET_PROP_DYNAMIC key not a string".into());
                    }
                    let prop_name_si = key_val.as_string_index();
                    self.regs[b] = self
                        .resolve_property(obj, prop_name_si)
                        .unwrap_or(JsValue::undefined());
                }

                OpCode::SET_PROP_DYNAMIC => {
                    let obj_ptr = self.regs[rd].as_object_ptr() as *mut JsObject;
                    if obj_ptr.is_null() {
                        return Err("TypeError: SET_PROP_DYNAMIC on non-object".into());
                    }
                    let obj = unsafe { &mut *obj_ptr };
                    let key_val = self.regs[a];
                    if !key_val.is_string() {
                        return Err("TypeError: SET_PROP_DYNAMIC key not a string".into());
                    }
                    let prop_name_si = key_val.as_string_index();
                    if let Some(pos) = self
                        .kernel
                        .shape_forge()
                        .lookup_position(obj.shape_id(), prop_name_si)
                    {
                        obj.set_prop_at(pos, self.regs[b]);
                    } else {
                        let new_shape_id = self
                            .kernel
                            .shape_forge()
                            .make_shape(obj.shape_id(), prop_name_si);
                        obj.set_shape_id(new_shape_id);
                        obj.push_prop(self.regs[b]);
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
                        return Err("TypeError: SET_ELEM on non-object".into());
                    }
                    let idx = self.regs[a].as_int() as usize;
                    let obj = unsafe { &mut *obj_ptr };
                    obj.set_prop_at(idx as u8, self.regs[b]);
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
                        return Err("TypeError: IN right-hand side is not an object".into());
                    }
                    let obj = unsafe { &*obj_ptr };
                    let prop_name_si = if key_val.is_string() {
                        key_val.as_string_index()
                    } else {
                        return Err("TypeError: IN key must be a string".into());
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
                        return Err("TypeError: MEMBER_INC on non-object".into());
                    }
                    let obj = unsafe { &mut *obj_ptr };
                    let prop_name_si = self.regs[b].as_string_index();
                    let ext0 = self.bytecode[self.pc];
                    let ext1 = self.bytecode[self.pc + 1];
                    let ext2 = self.bytecode[self.pc + 2];
                    self.pc += 3;
                    let cached_shape_id = ext0 & 0x00FF_FFFF;
                    let cached_ptr = ((ext2 as u64) << 32) | (ext1 as u64);

                    let prop_val = if cached_shape_id != 0
                        && cached_shape_id == obj.shape_id()
                        && cached_ptr != 0
                    {
                        unsafe { *(cached_ptr as *const JsValue) }
                    } else if let Some(template) =
                        self.kernel.prop_forge().get_template(obj.shape_id())
                    {
                        if let Some(ptr) = self.template_prop_ptr(obj, &template) {
                            self.bytecode[self.pc - 3] = obj.shape_id() & 0x00FF_FFFF;
                            self.bytecode[self.pc - 2] = ptr as u32;
                            self.bytecode[self.pc - 1] = (ptr as u64 >> 32) as u32;
                            unsafe { *ptr }
                        } else {
                            self.resolve_property(obj, prop_name_si)
                                .unwrap_or(JsValue::undefined())
                        }
                    } else if let Some(val) = self.resolve_property(obj, prop_name_si) {
                        let pos = self
                            .kernel
                            .shape_forge()
                            .lookup_position(obj.shape_id(), prop_name_si)
                            .unwrap_or(0);
                        if let Some(ptr) = obj.prop_ptr_at(pos) {
                            self.bytecode[self.pc - 3] = obj.shape_id() & 0x00FF_FFFF;
                            self.bytecode[self.pc - 2] = ptr as u32;
                            self.bytecode[self.pc - 1] = (ptr as u64 >> 32) as u32;
                        }
                        val
                    } else {
                        JsValue::undefined()
                    };

                    let n = coercion::to_number(prop_val, self.kernel.string_forge().as_ref());
                    let new_val = JsValue::float(n + 1.0);

                    if cached_shape_id != 0 && cached_shape_id == obj.shape_id() && cached_ptr != 0
                    {
                        unsafe {
                            *(cached_ptr as *mut JsValue) = new_val;
                        }
                    } else if let Some(pos) = self
                        .kernel
                        .shape_forge()
                        .lookup_position(obj.shape_id(), prop_name_si)
                    {
                        obj.set_prop_at(pos, new_val);
                        if let Some(ptr) = obj.prop_ptr_at(pos) {
                            self.bytecode[self.pc - 3] = obj.shape_id() & 0x00FF_FFFF;
                            self.bytecode[self.pc - 2] = ptr as u32;
                            self.bytecode[self.pc - 1] = (ptr as u64 >> 32) as u32;
                        }
                    } else {
                        let new_shape_id = self
                            .kernel
                            .shape_forge()
                            .make_shape(obj.shape_id(), prop_name_si);
                        obj.set_shape_id(new_shape_id);
                        let new_pos = obj.push_prop(new_val);
                        obj.bump_generation();
                        if let Some(ptr) = obj.prop_ptr_at(new_pos) {
                            self.bytecode[self.pc - 3] = new_shape_id & 0x00FF_FFFF;
                            self.bytecode[self.pc - 2] = ptr as u32;
                            self.bytecode[self.pc - 1] = (ptr as u64 >> 32) as u32;
                            self.kernel.prop_forge().upsert(
                                new_shape_id,
                                PropTemplate {
                                    shape_id: new_shape_id,
                                    position: new_pos,
                                    generation: obj.generation(),
                                },
                            );
                        }
                        self.regs[a] = new_val;
                        continue;
                    };
                    self.regs[a] = new_val;
                }

                OpCode::MEMBER_DEC => {
                    let obj_ptr = self.regs[rd].as_object_ptr() as *mut JsObject;
                    if obj_ptr.is_null() {
                        return Err("TypeError: MEMBER_DEC on non-object".into());
                    }
                    let obj = unsafe { &mut *obj_ptr };
                    let prop_name_si = self.regs[b].as_string_index();
                    let ext0 = self.bytecode[self.pc];
                    let ext1 = self.bytecode[self.pc + 1];
                    let ext2 = self.bytecode[self.pc + 2];
                    self.pc += 3;
                    let cached_shape_id = ext0 & 0x00FF_FFFF;
                    let cached_ptr = ((ext2 as u64) << 32) | (ext1 as u64);

                    let prop_val = if cached_shape_id != 0
                        && cached_shape_id == obj.shape_id()
                        && cached_ptr != 0
                    {
                        unsafe { *(cached_ptr as *const JsValue) }
                    } else if let Some(template) =
                        self.kernel.prop_forge().get_template(obj.shape_id())
                    {
                        if let Some(ptr) = self.template_prop_ptr(obj, &template) {
                            self.bytecode[self.pc - 3] = obj.shape_id() & 0x00FF_FFFF;
                            self.bytecode[self.pc - 2] = ptr as u32;
                            self.bytecode[self.pc - 1] = (ptr as u64 >> 32) as u32;
                            unsafe { *ptr }
                        } else {
                            self.resolve_property(obj, prop_name_si)
                                .unwrap_or(JsValue::undefined())
                        }
                    } else if let Some(val) = self.resolve_property(obj, prop_name_si) {
                        let pos = self
                            .kernel
                            .shape_forge()
                            .lookup_position(obj.shape_id(), prop_name_si)
                            .unwrap_or(0);
                        if let Some(ptr) = obj.prop_ptr_at(pos) {
                            self.bytecode[self.pc - 3] = obj.shape_id() & 0x00FF_FFFF;
                            self.bytecode[self.pc - 2] = ptr as u32;
                            self.bytecode[self.pc - 1] = (ptr as u64 >> 32) as u32;
                        }
                        val
                    } else {
                        JsValue::undefined()
                    };

                    let n = coercion::to_number(prop_val, self.kernel.string_forge().as_ref());
                    let new_val = JsValue::float(n - 1.0);

                    if cached_shape_id != 0 && cached_shape_id == obj.shape_id() && cached_ptr != 0
                    {
                        unsafe {
                            *(cached_ptr as *mut JsValue) = new_val;
                        }
                    } else if let Some(pos) = self
                        .kernel
                        .shape_forge()
                        .lookup_position(obj.shape_id(), prop_name_si)
                    {
                        obj.set_prop_at(pos, new_val);
                        if let Some(ptr) = obj.prop_ptr_at(pos) {
                            self.bytecode[self.pc - 3] = obj.shape_id() & 0x00FF_FFFF;
                            self.bytecode[self.pc - 2] = ptr as u32;
                            self.bytecode[self.pc - 1] = (ptr as u64 >> 32) as u32;
                        }
                    } else {
                        let new_shape_id = self
                            .kernel
                            .shape_forge()
                            .make_shape(obj.shape_id(), prop_name_si);
                        obj.set_shape_id(new_shape_id);
                        let new_pos = obj.push_prop(new_val);
                        obj.bump_generation();
                        if let Some(ptr) = obj.prop_ptr_at(new_pos) {
                            self.bytecode[self.pc - 3] = new_shape_id & 0x00FF_FFFF;
                            self.bytecode[self.pc - 2] = ptr as u32;
                            self.bytecode[self.pc - 1] = (ptr as u64 >> 32) as u32;
                            self.kernel.prop_forge().upsert(
                                new_shape_id,
                                PropTemplate {
                                    shape_id: new_shape_id,
                                    position: new_pos,
                                    generation: obj.generation(),
                                },
                            );
                        }
                        self.regs[a] = new_val;
                        continue;
                    };
                    self.regs[a] = new_val;
                }

                OpCode::DYN_MEMBER_INC => {
                    let obj_ptr = self.regs[rd].as_object_ptr() as *mut JsObject;
                    if obj_ptr.is_null() {
                        return Err("TypeError: DYN_MEMBER_INC on non-object".into());
                    }
                    let obj = unsafe { &mut *obj_ptr };
                    let key_val = self.regs[a];
                    if !key_val.is_string() {
                        return Err("TypeError: DYN_MEMBER_INC key not a string".into());
                    }
                    let prop_name_si = key_val.as_string_index();
                    let prop_val = self
                        .resolve_property(obj, prop_name_si)
                        .unwrap_or(JsValue::undefined());
                    let n = coercion::to_number(prop_val, self.kernel.string_forge().as_ref());
                    let new_val = JsValue::float(n + 1.0);

                    if let Some(pos) = self
                        .kernel
                        .shape_forge()
                        .lookup_position(obj.shape_id(), prop_name_si)
                    {
                        obj.set_prop_at(pos, new_val);
                    } else {
                        let new_shape_id = self
                            .kernel
                            .shape_forge()
                            .make_shape(obj.shape_id(), prop_name_si);
                        obj.set_shape_id(new_shape_id);
                        obj.push_prop(new_val);
                        obj.bump_generation();
                    };
                    self.regs[b] = new_val;
                }

                OpCode::DYN_MEMBER_DEC => {
                    let obj_ptr = self.regs[rd].as_object_ptr() as *mut JsObject;
                    if obj_ptr.is_null() {
                        return Err("TypeError: DYN_MEMBER_DEC on non-object".into());
                    }
                    let obj = unsafe { &mut *obj_ptr };
                    let key_val = self.regs[a];
                    if !key_val.is_string() {
                        return Err("TypeError: DYN_MEMBER_DEC key not a string".into());
                    }
                    let prop_name_si = key_val.as_string_index();
                    let prop_val = self
                        .resolve_property(obj, prop_name_si)
                        .unwrap_or(JsValue::undefined());
                    let n = coercion::to_number(prop_val, self.kernel.string_forge().as_ref());
                    let new_val = JsValue::float(n - 1.0);

                    if let Some(pos) = self
                        .kernel
                        .shape_forge()
                        .lookup_position(obj.shape_id(), prop_name_si)
                    {
                        obj.set_prop_at(pos, new_val);
                    } else {
                        let new_shape_id = self
                            .kernel
                            .shape_forge()
                            .make_shape(obj.shape_id(), prop_name_si);
                        obj.set_shape_id(new_shape_id);
                        obj.push_prop(new_val);
                        obj.bump_generation();
                    };
                    self.regs[b] = new_val;
                }

                OpCode::COMPOUND_MEMBER_ADD => {
                    let obj_ptr = self.regs[rd].as_object_ptr() as *mut JsObject;
                    if obj_ptr.is_null() {
                        return Err("TypeError: COMPOUND_MEMBER_ADD on non-object".into());
                    }
                    let obj = unsafe { &mut *obj_ptr };
                    let prop_name_si = self.regs[b].as_string_index();
                    let ext0 = self.bytecode[self.pc];
                    let ext1 = self.bytecode[self.pc + 1];
                    let ext2 = self.bytecode[self.pc + 2];
                    self.pc += 3;
                    let cached_shape_id = ext0 & 0x00FF_FFFF;
                    let cached_ptr = ((ext2 as u64) << 32) | (ext1 as u64);

                    let prop_val = if cached_shape_id != 0
                        && cached_shape_id == obj.shape_id()
                        && cached_ptr != 0
                    {
                        unsafe { *(cached_ptr as *const JsValue) }
                    } else if let Some(template) =
                        self.kernel.prop_forge().get_template(obj.shape_id())
                    {
                        if let Some(ptr) = self.template_prop_ptr(obj, &template) {
                            self.bytecode[self.pc - 3] = obj.shape_id() & 0x00FF_FFFF;
                            self.bytecode[self.pc - 2] = ptr as u32;
                            self.bytecode[self.pc - 1] = (ptr as u64 >> 32) as u32;
                            unsafe { *ptr }
                        } else {
                            self.resolve_property(obj, prop_name_si)
                                .unwrap_or(JsValue::undefined())
                        }
                    } else if let Some(val) = self.resolve_property(obj, prop_name_si) {
                        let pos = self
                            .kernel
                            .shape_forge()
                            .lookup_position(obj.shape_id(), prop_name_si)
                            .unwrap_or(0);
                        if let Some(ptr) = obj.prop_ptr_at(pos) {
                            self.bytecode[self.pc - 3] = obj.shape_id() & 0x00FF_FFFF;
                            self.bytecode[self.pc - 2] = ptr as u32;
                            self.bytecode[self.pc - 1] = (ptr as u64 >> 32) as u32;
                        }
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

                    self.set_member_prop(obj, prop_name_si, new_val)?;
                    self.regs[a] = new_val;
                }

                OpCode::COMPOUND_MEMBER_SUB => {
                    let obj_ptr = self.regs[rd].as_object_ptr() as *mut JsObject;
                    if obj_ptr.is_null() {
                        return Err("TypeError: COMPOUND_MEMBER_SUB on non-object".into());
                    }
                    let obj = unsafe { &mut *obj_ptr };
                    let prop_name_si = self.regs[b].as_string_index();
                    let ext0 = self.bytecode[self.pc];
                    let ext1 = self.bytecode[self.pc + 1];
                    let ext2 = self.bytecode[self.pc + 2];
                    self.pc += 3;
                    let cached_shape_id = ext0 & 0x00FF_FFFF;
                    let cached_ptr = ((ext2 as u64) << 32) | (ext1 as u64);

                    let prop_val = if cached_shape_id != 0
                        && cached_shape_id == obj.shape_id()
                        && cached_ptr != 0
                    {
                        unsafe { *(cached_ptr as *const JsValue) }
                    } else if let Some(template) =
                        self.kernel.prop_forge().get_template(obj.shape_id())
                    {
                        if let Some(ptr) = self.template_prop_ptr(obj, &template) {
                            self.bytecode[self.pc - 3] = obj.shape_id() & 0x00FF_FFFF;
                            self.bytecode[self.pc - 2] = ptr as u32;
                            self.bytecode[self.pc - 1] = (ptr as u64 >> 32) as u32;
                            unsafe { *ptr }
                        } else {
                            self.resolve_property(obj, prop_name_si)
                                .unwrap_or(JsValue::undefined())
                        }
                    } else if let Some(val) = self.resolve_property(obj, prop_name_si) {
                        let pos = self
                            .kernel
                            .shape_forge()
                            .lookup_position(obj.shape_id(), prop_name_si)
                            .unwrap_or(0);
                        if let Some(ptr) = obj.prop_ptr_at(pos) {
                            self.bytecode[self.pc - 3] = obj.shape_id() & 0x00FF_FFFF;
                            self.bytecode[self.pc - 2] = ptr as u32;
                            self.bytecode[self.pc - 1] = (ptr as u64 >> 32) as u32;
                        }
                        val
                    } else {
                        JsValue::undefined()
                    };

                    let ln = coercion::to_number(prop_val, self.kernel.string_forge().as_ref());
                    let rn = coercion::to_number(self.regs[a], self.kernel.string_forge().as_ref());
                    let new_val = JsValue::float(ln - rn);
                    self.regs[a] = new_val;
                    self.set_member_prop(obj, prop_name_si, new_val)?;
                }

                OpCode::COMPOUND_MEMBER_MUL => {
                    let obj_ptr = self.regs[rd].as_object_ptr() as *mut JsObject;
                    if obj_ptr.is_null() {
                        return Err("TypeError: COMPOUND_MEMBER_MUL on non-object".into());
                    }
                    let obj = unsafe { &mut *obj_ptr };
                    let prop_name_si = self.regs[b].as_string_index();
                    let ext0 = self.bytecode[self.pc];
                    let ext1 = self.bytecode[self.pc + 1];
                    let ext2 = self.bytecode[self.pc + 2];
                    self.pc += 3;
                    let cached_shape_id = ext0 & 0x00FF_FFFF;
                    let cached_ptr = ((ext2 as u64) << 32) | (ext1 as u64);

                    let prop_val = if cached_shape_id != 0
                        && cached_shape_id == obj.shape_id()
                        && cached_ptr != 0
                    {
                        unsafe { *(cached_ptr as *const JsValue) }
                    } else if let Some(template) =
                        self.kernel.prop_forge().get_template(obj.shape_id())
                    {
                        if let Some(ptr) = self.template_prop_ptr(obj, &template) {
                            self.bytecode[self.pc - 3] = obj.shape_id() & 0x00FF_FFFF;
                            self.bytecode[self.pc - 2] = ptr as u32;
                            self.bytecode[self.pc - 1] = (ptr as u64 >> 32) as u32;
                            unsafe { *ptr }
                        } else {
                            self.resolve_property(obj, prop_name_si)
                                .unwrap_or(JsValue::undefined())
                        }
                    } else if let Some(val) = self.resolve_property(obj, prop_name_si) {
                        let pos = self
                            .kernel
                            .shape_forge()
                            .lookup_position(obj.shape_id(), prop_name_si)
                            .unwrap_or(0);
                        if let Some(ptr) = obj.prop_ptr_at(pos) {
                            self.bytecode[self.pc - 3] = obj.shape_id() & 0x00FF_FFFF;
                            self.bytecode[self.pc - 2] = ptr as u32;
                            self.bytecode[self.pc - 1] = (ptr as u64 >> 32) as u32;
                        }
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
                        return Err("TypeError: COMPOUND_MEMBER_DIV on non-object".into());
                    }
                    let obj = unsafe { &mut *obj_ptr };
                    let prop_name_si = self.regs[b].as_string_index();
                    let ext0 = self.bytecode[self.pc];
                    let ext1 = self.bytecode[self.pc + 1];
                    let ext2 = self.bytecode[self.pc + 2];
                    self.pc += 3;
                    let cached_shape_id = ext0 & 0x00FF_FFFF;
                    let cached_ptr = ((ext2 as u64) << 32) | (ext1 as u64);

                    let prop_val = if cached_shape_id != 0
                        && cached_shape_id == obj.shape_id()
                        && cached_ptr != 0
                    {
                        unsafe { *(cached_ptr as *const JsValue) }
                    } else if let Some(template) =
                        self.kernel.prop_forge().get_template(obj.shape_id())
                    {
                        if let Some(ptr) = self.template_prop_ptr(obj, &template) {
                            self.bytecode[self.pc - 3] = obj.shape_id() & 0x00FF_FFFF;
                            self.bytecode[self.pc - 2] = ptr as u32;
                            self.bytecode[self.pc - 1] = (ptr as u64 >> 32) as u32;
                            unsafe { *ptr }
                        } else {
                            self.resolve_property(obj, prop_name_si)
                                .unwrap_or(JsValue::undefined())
                        }
                    } else if let Some(val) = self.resolve_property(obj, prop_name_si) {
                        let pos = self
                            .kernel
                            .shape_forge()
                            .lookup_position(obj.shape_id(), prop_name_si)
                            .unwrap_or(0);
                        if let Some(ptr) = obj.prop_ptr_at(pos) {
                            self.bytecode[self.pc - 3] = obj.shape_id() & 0x00FF_FFFF;
                            self.bytecode[self.pc - 2] = ptr as u32;
                            self.bytecode[self.pc - 1] = (ptr as u64 >> 32) as u32;
                        }
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
                        return Err("TypeError: COMPOUND_MEMBER_MOD on non-object".into());
                    }
                    let obj = unsafe { &mut *obj_ptr };
                    let prop_name_si = self.regs[b].as_string_index();
                    let ext0 = self.bytecode[self.pc];
                    let ext1 = self.bytecode[self.pc + 1];
                    let ext2 = self.bytecode[self.pc + 2];
                    self.pc += 3;
                    let cached_shape_id = ext0 & 0x00FF_FFFF;
                    let cached_ptr = ((ext2 as u64) << 32) | (ext1 as u64);

                    let prop_val = if cached_shape_id != 0
                        && cached_shape_id == obj.shape_id()
                        && cached_ptr != 0
                    {
                        unsafe { *(cached_ptr as *const JsValue) }
                    } else if let Some(template) =
                        self.kernel.prop_forge().get_template(obj.shape_id())
                    {
                        if let Some(ptr) = self.template_prop_ptr(obj, &template) {
                            self.bytecode[self.pc - 3] = obj.shape_id() & 0x00FF_FFFF;
                            self.bytecode[self.pc - 2] = ptr as u32;
                            self.bytecode[self.pc - 1] = (ptr as u64 >> 32) as u32;
                            unsafe { *ptr }
                        } else {
                            self.resolve_property(obj, prop_name_si)
                                .unwrap_or(JsValue::undefined())
                        }
                    } else if let Some(val) = self.resolve_property(obj, prop_name_si) {
                        let pos = self
                            .kernel
                            .shape_forge()
                            .lookup_position(obj.shape_id(), prop_name_si)
                            .unwrap_or(0);
                        if let Some(ptr) = obj.prop_ptr_at(pos) {
                            self.bytecode[self.pc - 3] = obj.shape_id() & 0x00FF_FFFF;
                            self.bytecode[self.pc - 2] = ptr as u32;
                            self.bytecode[self.pc - 1] = (ptr as u64 >> 32) as u32;
                        }
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
                        return Err("TypeError: COMPOUND_MEMBER_EXP on non-object".into());
                    }
                    let obj = unsafe { &mut *obj_ptr };
                    let prop_name_si = self.regs[b].as_string_index();
                    let ext0 = self.bytecode[self.pc];
                    let ext1 = self.bytecode[self.pc + 1];
                    let ext2 = self.bytecode[self.pc + 2];
                    self.pc += 3;
                    let cached_shape_id = ext0 & 0x00FF_FFFF;
                    let cached_ptr = ((ext2 as u64) << 32) | (ext1 as u64);

                    let prop_val = if cached_shape_id != 0
                        && cached_shape_id == obj.shape_id()
                        && cached_ptr != 0
                    {
                        unsafe { *(cached_ptr as *const JsValue) }
                    } else if let Some(template) =
                        self.kernel.prop_forge().get_template(obj.shape_id())
                    {
                        if let Some(ptr) = self.template_prop_ptr(obj, &template) {
                            self.bytecode[self.pc - 3] = obj.shape_id() & 0x00FF_FFFF;
                            self.bytecode[self.pc - 2] = ptr as u32;
                            self.bytecode[self.pc - 1] = (ptr as u64 >> 32) as u32;
                            unsafe { *ptr }
                        } else {
                            self.resolve_property(obj, prop_name_si)
                                .unwrap_or(JsValue::undefined())
                        }
                    } else if let Some(val) = self.resolve_property(obj, prop_name_si) {
                        let pos = self
                            .kernel
                            .shape_forge()
                            .lookup_position(obj.shape_id(), prop_name_si)
                            .unwrap_or(0);
                        if let Some(ptr) = obj.prop_ptr_at(pos) {
                            self.bytecode[self.pc - 3] = obj.shape_id() & 0x00FF_FFFF;
                            self.bytecode[self.pc - 2] = ptr as u32;
                            self.bytecode[self.pc - 1] = (ptr as u64 >> 32) as u32;
                        }
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
