use crate::value::JsValue;

pub type ShapeId = u32;

pub trait PropIndex {
    fn to_u32(self) -> u32;
}

impl PropIndex for u8 {
    fn to_u32(self) -> u32 {
        self as u32
    }
}

impl PropIndex for u16 {
    fn to_u32(self) -> u32 {
        self as u32
    }
}

impl PropIndex for u32 {
    fn to_u32(self) -> u32 {
        self
    }
}

impl PropIndex for usize {
    fn to_u32(self) -> u32 {
        self as u32
    }
}

impl PropIndex for i32 {
    fn to_u32(self) -> u32 {
        debug_assert!(self >= 0, "property index must be non-negative");
        self.max(0) as u32
    }
}

/// Layout:
///   header: u32 bits
///     [0:23]   shape_id
///     [26]     is_derived_constructor
///     [27]     is_class_constructor
///     [28]     is_arrow
///     [29]     is_array
///     [30]     is_extensible
///     [31]     is_function
///   native_arg_count: u8 (1 byte + 3 pad)
///   hash_props: *mut u8 (8 bytes, points to Box<Vec<Box<JsValue>>>)
///   proto: JsValue (8 bytes)
///   generation: u32 (4 bytes + 4 pad)
///   native_fn: u64 (8 bytes, 0 = None sentinel)
///   sub_module_index: u32 (4 bytes + 4 pad, index into CompiledModule.sub_modules)
///   captured_this: JsValue (8 bytes, lexical this for arrow functions)
///   home_object: JsValue (8 bytes, [[HomeObject]] for super lookup)
#[repr(C)]
pub struct JsObject {
    header: u32,
    native_arg_count: u8,
    _pad: [u8; 3],
    hash_props: *mut u8,
    proto: JsValue,
    generation: u32,
    _pad2: [u8; 4],
    native_fn: u64,
    sub_module_index: u32,
    _pad3: [u8; 4],
    captured_this: JsValue,
    home_object: JsValue,
}

impl JsObject {
    pub fn new_empty(shape_id: ShapeId, proto: JsValue) -> Self {
        Self {
            header: (shape_id & 0x00FF_FFFF) | (1 << 30),
            native_arg_count: 0,
            _pad: [0; 3],
            hash_props: std::ptr::null_mut(),
            proto,
            generation: 1,
            _pad2: [0; 4],
            native_fn: 0,
            sub_module_index: 0,
            _pad3: [0; 4],
            captured_this: JsValue::undefined(),
            home_object: JsValue::undefined(),
        }
    }

    pub fn new_array(shape_id: ShapeId, proto: JsValue, n_elements: usize, _bump: &bumpalo::Bump) -> Self {
        let mut obj = Self {
            header: (shape_id & 0x00FF_FFFF) | (1 << 30) | (1 << 29),
            native_arg_count: 0,
            _pad: [0; 3],
            hash_props: std::ptr::null_mut(),
            proto,
            generation: 1,
            _pad2: [0; 4],
            native_fn: 0,
            sub_module_index: 0,
            _pad3: [0; 4],
            captured_this: JsValue::undefined(),
            home_object: JsValue::undefined(),
        };
        let vec = Box::new(vec![Box::new(JsValue::undefined()); n_elements]);
        obj.hash_props = Box::into_raw(vec) as *mut u8;
        obj
    }

    pub fn shape_id(&self) -> ShapeId {
        self.header & 0x00FF_FFFF
    }

    pub fn set_shape_id(&mut self, id: ShapeId) {
        self.header = (self.header & !0x00FF_FFFF) | (id & 0x00FF_FFFF);
    }

    /// Returns property count from the hash_props vec length.
    /// Returns 0 if hash_props has not been allocated.
    pub fn prop_count(&self) -> u32 {
        if self.hash_props.is_null() {
            0
        } else {
            let vec = unsafe { &*(self.hash_props as *const Vec<Box<JsValue>>) };
            vec.len() as u32
        }
    }

    /// Sets the length of hash_props vec. Truncates or extends with undefined.
    pub fn set_prop_count(&mut self, count: impl PropIndex) {
        let vec = self.ensure_hash_props();
        let target = count.to_u32() as usize;
        if target < vec.len() {
            vec.truncate(target);
        } else {
            while vec.len() < target {
                vec.push(Box::new(JsValue::undefined()));
            }
        }
    }

    pub fn is_array(&self) -> bool {
        (self.header >> 29) & 1 != 0
    }

    pub fn is_extensible(&self) -> bool {
        (self.header >> 30) & 1 != 0
    }

    pub fn set_extensible(&mut self, ext: bool) {
        if ext {
            self.header |= 1 << 30;
        } else {
            self.header &= !(1 << 30);
        }
    }

    pub fn is_function(&self) -> bool {
        (self.header >> 31) & 1 != 0
    }

    pub fn set_function(&mut self, f: bool) {
        if f {
            self.header |= 1 << 31;
        } else {
            self.header &= !(1 << 31);
        }
    }

    /// Initialize hash_props if null, return mutable reference to Vec.
    pub fn ensure_hash_props(&mut self) -> &mut Vec<Box<JsValue>> {
        if self.hash_props.is_null() {
            let vec = Box::new(Vec::<Box<JsValue>>::new());
            self.hash_props = Box::into_raw(vec) as *mut u8;
        }
        unsafe { &mut *(self.hash_props as *mut Vec<Box<JsValue>>) }
    }

    /// Safe read access to hash_props vec. Returns None if not allocated.
    pub fn hash_props_vec(&self) -> Option<&Vec<Box<JsValue>>> {
        if self.hash_props.is_null() {
            None
        } else {
            unsafe { Some(&*(self.hash_props as *const Vec<Box<JsValue>>)) }
        }
    }

    /// Get property value at position index in the vec.
    /// Returns JsValue::undefined() if hash_props not allocated or position out of bounds.
    pub fn get_prop_at(&self, position: impl PropIndex) -> JsValue {
        if self.hash_props.is_null() {
            return JsValue::undefined();
        }
        let vec = unsafe { &*(self.hash_props as *const Vec<Box<JsValue>>) };
        vec.get(position.to_u32() as usize).map(|b| **b).unwrap_or(JsValue::undefined())
    }

    /// Set property value at position index. Vec auto-grows if needed.
    pub fn set_prop_at(&mut self, position: impl PropIndex, val: JsValue) {
        let vec = self.ensure_hash_props();
        let pos = position.to_u32() as usize;
        if pos < vec.len() {
            *vec[pos] = val;
        } else {
            while vec.len() < pos {
                vec.push(Box::new(JsValue::undefined()));
            }
            vec.push(Box::new(val));
        }
    }

    /// Push a value onto hash_props vec. Returns the index position.
    pub fn push_prop(&mut self, val: JsValue) -> u32 {
        let vec = self.ensure_hash_props();
        let pos = vec.len();
        vec.push(Box::new(val));
        pos as u32
    }

    /// Get a stable pointer to the Box<JsValue> at position.
    /// Returns None if hash_props not allocated or position out of bounds.
    pub fn prop_ptr_at(&self, position: impl PropIndex) -> Option<*const JsValue> {
        if self.hash_props.is_null() {
            return None;
        }
        let vec = unsafe { &*(self.hash_props as *const Vec<Box<JsValue>>) };
        vec.get(position.to_u32() as usize).map(|b| &**b as *const JsValue)
    }

    /// Get the length of hash_props vec (returns 0 if not allocated).
    pub fn prop_vec_len(&self) -> usize {
        if self.hash_props.is_null() {
            0
        } else {
            unsafe { &*(self.hash_props as *const Vec<Box<JsValue>>) }.len()
        }
    }

    pub fn proto(&self) -> JsValue {
        self.proto
    }

    pub fn set_proto(&mut self, proto: JsValue) -> Result<(), &'static str> {
        if proto.is_null() {
            self.proto = proto;
            self.generation = self.generation.wrapping_add(1);
            return Ok(());
        }
        if !proto.is_object() {
            return Err("__proto__ must be an object or null");
        }
        let mut cursor = proto;
        let self_ptr = self as *const JsObject;
        while cursor.is_object() {
            let cursor_ptr = cursor.as_js_object_ptr();
            if std::ptr::eq(cursor_ptr, self_ptr) {
                return Err("cyclic __proto__ value");
            }
            let obj = unsafe { &*cursor_ptr };
            cursor = obj.proto;
        }
        self.proto = proto;
        self.generation = self.generation.wrapping_add(1);
        Ok(())
    }

    pub fn generation(&self) -> u32 {
        self.generation
    }

    pub fn bump_generation(&mut self) {
        self.generation = self.generation.wrapping_add(1);
    }

    pub fn native_fn(&self) -> Option<*const ()> {
        if self.native_fn == 0 {
            None
        } else {
            Some(self.native_fn as *const ())
        }
    }

    pub fn set_native_fn(&mut self, ptr: Option<*const ()>) {
        self.native_fn = ptr.map_or(0, |p| p as u64);
    }

    pub fn native_arg_count(&self) -> u8 {
        self.native_arg_count
    }

    pub fn set_native_arg_count(&mut self, n: u8) {
        self.native_arg_count = n;
    }

    pub fn sub_module_index(&self) -> u32 {
        self.sub_module_index
    }

    pub fn set_sub_module_index(&mut self, idx: u32) {
        self.sub_module_index = idx;
    }

    /// Arrow function flag (header bit 28).
    /// When true, CALL dispatch captures lexical `this` from creation time.
    pub fn is_arrow(&self) -> bool {
        (self.header >> 28) & 1 != 0
    }

    pub fn set_arrow(&mut self, v: bool) {
        if v {
            self.header |= 1 << 28;
        } else {
            self.header &= !(1 << 28);
        }
    }

    /// Lexical `this` captured at arrow function creation time.
    /// Only meaningful when `is_arrow()` returns true.
    pub fn captured_this(&self) -> JsValue {
        self.captured_this
    }

    pub fn set_captured_this(&mut self, v: JsValue) {
        self.captured_this = v;
    }

    /// Class constructor flag (header bit 27).
    /// Ordinary CALL rejects objects with this flag; NEW_EXPRESSION is allowed.
    pub fn is_class_constructor(&self) -> bool {
        (self.header >> 27) & 1 != 0
    }

    pub fn set_class_constructor(&mut self, v: bool) {
        if v {
            self.header |= 1 << 27;
        } else {
            self.header &= !(1 << 27);
        }
    }

    pub fn is_derived_constructor(&self) -> bool {
        (self.header >> 26) & 1 != 0
    }

    pub fn set_derived_constructor(&mut self, v: bool) {
        if v {
            self.header |= 1 << 26;
        } else {
            self.header &= !(1 << 26);
        }
    }

    pub fn home_object(&self) -> JsValue {
        self.home_object
    }

    pub fn set_home_object(&mut self, v: JsValue) {
        self.home_object = v;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shape::EMPTY_SHAPE_ID;

    #[test]
    fn object_size_bounds() {
        let sz = std::mem::size_of::<JsObject>();
        assert!(sz >= 40 && sz <= 72, "JsObject layout mismatch - expected 40-72B, got {sz}B");
    }

    #[test]
    fn new_empty_defaults() {
        let obj = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null());
        assert_eq!(obj.shape_id(), EMPTY_SHAPE_ID);
        assert_eq!(obj.prop_count(), 0);
        assert!(obj.is_extensible());
        assert!(!obj.is_array());
        assert!(!obj.is_function());
        assert_eq!(obj.generation(), 1);
        assert!(obj.hash_props_vec().is_none());
    }

    #[test]
    fn shape_id_roundtrip() {
        let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null());
        obj.set_shape_id(0x00AB_CDEF);
        assert_eq!(obj.shape_id(), 0x00AB_CDEF);
    }

    #[test]
    fn prop_count_roundtrip() {
        let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null());
        assert_eq!(obj.prop_count(), 0);
        obj.ensure_hash_props().push(Box::new(JsValue::int(17)));
        assert_eq!(obj.prop_count(), 1);
    }

    #[test]
    fn flags_individual() {
        let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null());
        assert!(obj.is_extensible());
        obj.set_extensible(false);
        assert!(!obj.is_extensible());
    }

    #[test]
    fn hash_prop_read_write() {
        let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null());
        obj.set_prop_at(0, JsValue::int(42));
        assert_eq!(obj.get_prop_at(0), JsValue::int(42));
    }

    #[test]
    fn new_array_flags() {
        let bump = bumpalo::Bump::new();
        let obj = JsObject::new_array(5, JsValue::null(), 3, &bump);
        assert!(obj.is_array());
        assert_eq!(obj.shape_id(), 5);
        assert_eq!(obj.prop_count(), 3);
    }

    #[test]
    fn generation_bump() {
        let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null());
        assert_eq!(obj.generation(), 1);
        obj.bump_generation();
        assert_eq!(obj.generation(), 2);
    }

    #[test]
    fn hash_props_lazy_init() {
        let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null());
        assert!(obj.hash_props_vec().is_none());
        assert_eq!(obj.prop_count(), 0);
        obj.set_prop_at(0, JsValue::int(1));
        assert!(obj.hash_props_vec().is_some());
        assert_eq!(obj.prop_count(), 1);
    }

    #[test]
    fn hash_props_stable_pointer() {
        let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null());
        obj.set_prop_at(0, JsValue::int(100));
        let ptr = obj.prop_ptr_at(0);
        obj.set_prop_at(1, JsValue::int(200));
        // Pointer to first element should be stable (per Box allocation)
        let ptr2 = obj.prop_ptr_at(0);
        assert_eq!(ptr, ptr2);
        assert_eq!(obj.get_prop_at(0), JsValue::int(100));
        assert_eq!(obj.get_prop_at(1), JsValue::int(200));
    }
}
