use crate::value::JsValue;

/// Type-safe opaque wrapper around a native function pointer.
///
/// Stored as `*const ()` rather than a concrete `fn` type so that `oxide_types` does not
/// need to depend on `oxide_vm::Vm`. Callers in `oxide_vm` cast back to `NativeFn` via
/// `NativeFnPtr::call_with` — transmute is confined to a single generic helper there.
///
/// # Safety invariants
///
/// A `NativeFnPtr` value must always have been created from a valid `NativeFn` function
/// pointer (a bare `fn` item or function-item coercion — **not** a closure). The pointer is
/// never null. `Send + Sync` are safe because function-item pointers are inherently
/// thread-safe (they contain no data).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(transparent)]
pub struct NativeFnPtr(pub *const ());

impl NativeFnPtr {
    /// Wrap a raw function pointer. The pointer must point to a valid `NativeFn` fn-item.
    ///
    /// # Safety
    /// `ptr` must be a non-null function pointer of type `fn(&mut Vm, &[u8]) -> NativeResult`
    /// cast to `*const ()`. Using any other pointer value is UB at call time.
    #[inline(always)]
    pub unsafe fn from_raw(ptr: *const ()) -> Self {
        debug_assert!(!ptr.is_null(), "NativeFnPtr must not be null");
        Self(ptr)
    }

    /// Return the underlying raw pointer.
    #[inline(always)]
    pub fn as_ptr(self) -> *const () {
        self.0
    }
}

// SAFETY: fn-item pointers contain no mutable state; safe to share across threads.
unsafe impl Send for NativeFnPtr {}
unsafe impl Sync for NativeFnPtr {}

pub type ShapeId = u32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PropAttributes(pub u8);

impl PropAttributes {
    pub const WRITABLE: u8 = 0b001;
    pub const ENUMERABLE: u8 = 0b010;
    pub const CONFIGURABLE: u8 = 0b100;
    pub const DEFAULT_DATA: Self = Self(Self::WRITABLE | Self::ENUMERABLE | Self::CONFIGURABLE);

    pub const fn new(writable: bool, enumerable: bool, configurable: bool) -> Self {
        let mut bits = 0;
        if writable {
            bits |= Self::WRITABLE;
        }
        if enumerable {
            bits |= Self::ENUMERABLE;
        }
        if configurable {
            bits |= Self::CONFIGURABLE;
        }
        Self(bits)
    }

    pub const fn writable(self) -> bool {
        self.0 & Self::WRITABLE != 0
    }

    pub const fn enumerable(self) -> bool {
        self.0 & Self::ENUMERABLE != 0
    }

    pub const fn configurable(self) -> bool {
        self.0 & Self::CONFIGURABLE != 0
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PropMetaEntry {
    pub attributes: PropAttributes,
    pub get: JsValue,
    pub set: JsValue,
    pub is_accessor: bool,
}

impl PropMetaEntry {
    pub fn data(attributes: PropAttributes) -> Self {
        Self {
            attributes,
            get: JsValue::undefined(),
            set: JsValue::undefined(),
            is_accessor: false,
        }
    }

    pub fn accessor(get: JsValue, set: JsValue, attributes: PropAttributes) -> Self {
        Self {
            attributes,
            get,
            set,
            is_accessor: true,
        }
    }
}

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
///     [24]     is_set
///     [25]     is_map
///     [26]     is_derived_constructor
///     [27]     is_class_constructor
///     [28]     is_arrow
///     [29]     is_array
///     [30]     is_extensible
///     [31]     is_function
///   native_arg_count: u8 (1 byte + 3 pad)
///   hash_props: *mut u8 (8 bytes, points to Box<Vec<JsValue>>)
///   prop_meta: *mut u8 (8 bytes, points to Box<Vec<Option<PropMetaEntry>>>)
///   proto: JsValue (8 bytes)
///   generation: u32 (4 bytes + 4 pad)
///   native_fn: Option<NativeFnPtr> (16 bytes — Option<NonNull> optimization NOT available for
///              raw *const (); stored as Option wrapping an 8-byte pointer, with 8 bytes of
///              discriminant padding due to repr(Rust) layout rules)
///   sub_module_index: u32 (4 bytes + 4 pad, index into CompiledModule.sub_modules)
///   captured_this: JsValue (8 bytes, lexical this for arrow functions)
///   home_object: JsValue (8 bytes, [[HomeObject]] for super lookup)
pub struct JsObject {
    header: u32,
    native_arg_count: u8,
    _pad: [u8; 3],
    hash_props: *mut u8,
    prop_meta: *mut u8,
    proto: JsValue,
    generation: u32,
    _pad2: [u8; 4],
    native_fn: Option<NativeFnPtr>,
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
            prop_meta: std::ptr::null_mut(),
            proto,
            generation: 1,
            _pad2: [0; 4],
            native_fn: None,
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
            prop_meta: std::ptr::null_mut(),
            proto,
            generation: 1,
            _pad2: [0; 4],
            native_fn: None,
            sub_module_index: 0,
            _pad3: [0; 4],
            captured_this: JsValue::undefined(),
            home_object: JsValue::undefined(),
        };
        let vec = Box::new(vec![JsValue::undefined(); n_elements]);
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
            // SAFETY: hash_props is either null or was created from Box<Vec<JsValue>>
            // in ensure_hash_props/new_array and remains owned by this object.
            let vec = unsafe { &*(self.hash_props as *const Vec<JsValue>) };
            vec.len() as u32
        }
    }

    /// Sets the length of hash_props vec. Truncates or extends with undefined.
    pub fn set_prop_count(&mut self, count: impl PropIndex) {
        let target = count.to_u32() as usize;
        {
            let vec = self.ensure_hash_props();
            if target < vec.len() {
                vec.truncate(target);
            } else {
                while vec.len() < target {
                    vec.push(JsValue::undefined());
                }
            }
        }
        if let Some(meta) = self.prop_meta_vec_mut() {
            if target < meta.len() {
                meta.truncate(target);
            } else {
                while meta.len() < target {
                    meta.push(None);
                }
            }
        }
    }

    pub fn has_prop_meta(&self) -> bool {
        !self.prop_meta.is_null()
    }

    pub fn ensure_prop_meta(&mut self) -> &mut Vec<Option<PropMetaEntry>> {
        if self.prop_meta.is_null() {
            let len = self.prop_vec_len();
            let vec = Box::new(vec![None::<PropMetaEntry>; len]);
            self.prop_meta = Box::into_raw(vec) as *mut u8;
        }
        // SAFETY: prop_meta was set from Box<Vec<Option<PropMetaEntry>>> in
        // ensure_prop_meta and remains valid while this object owns it.
        unsafe { &mut *(self.prop_meta as *mut Vec<Option<PropMetaEntry>>) }
    }

    pub fn prop_meta_vec(&self) -> Option<&Vec<Option<PropMetaEntry>>> {
        if self.prop_meta.is_null() {
            None
        } else {
            // SAFETY: prop_meta was set from Box<Vec<Option<PropMetaEntry>>> in
            // ensure_prop_meta and remains valid while this object owns it.
            unsafe { Some(&*(self.prop_meta as *const Vec<Option<PropMetaEntry>>)) }
        }
    }

    fn prop_meta_vec_mut(&mut self) -> Option<&mut Vec<Option<PropMetaEntry>>> {
        if self.prop_meta.is_null() {
            None
        } else {
            // SAFETY: prop_meta was set from Box<Vec<Option<PropMetaEntry>>> in
            // ensure_prop_meta and remains valid while this object owns it.
            unsafe { Some(&mut *(self.prop_meta as *mut Vec<Option<PropMetaEntry>>)) }
        }
    }

    pub fn prop_meta_at(&self, position: impl PropIndex) -> Option<PropMetaEntry> {
        let pos = position.to_u32() as usize;
        self.prop_meta_vec().and_then(|vec| vec.get(pos).copied().flatten())
    }

    pub fn set_data_meta(&mut self, position: impl PropIndex, attributes: PropAttributes) {
        self.set_meta_at(position, PropMetaEntry::data(attributes));
    }

    pub fn set_accessor_meta(
        &mut self, position: impl PropIndex, get: JsValue, set: JsValue, attributes: PropAttributes,
    ) {
        self.set_meta_at(position, PropMetaEntry::accessor(get, set, attributes));
    }

    pub fn is_accessor_meta(&self, position: impl PropIndex) -> bool {
        self.prop_meta_at(position).is_some_and(|entry| entry.is_accessor)
    }

    fn set_meta_at(&mut self, position: impl PropIndex, entry: PropMetaEntry) {
        let pos = position.to_u32() as usize;
        let prop_len = self.prop_vec_len();
        if pos >= prop_len {
            self.set_prop_count(pos + 1);
        }
        let meta = self.ensure_prop_meta();
        while meta.len() <= pos {
            meta.push(None);
        }
        meta[pos] = Some(entry);
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

    pub fn is_set(&self) -> bool {
        (self.header >> 24) & 1 != 0
    }

    pub fn set_set(&mut self, s: bool) {
        if s {
            self.header |= 1 << 24;
        } else {
            self.header &= !(1 << 24);
        }
    }

    pub fn is_map(&self) -> bool {
        (self.header >> 25) & 1 != 0
    }

    pub fn set_map(&mut self, m: bool) {
        if m {
            self.header |= 1 << 25;
        } else {
            self.header &= !(1 << 25);
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
    pub fn ensure_hash_props(&mut self) -> &mut Vec<JsValue> {
        if self.hash_props.is_null() {
            let vec = Box::new(Vec::<JsValue>::new());
            self.hash_props = Box::into_raw(vec) as *mut u8;
        }
        // SAFETY: hash_props was set from Box<Vec<JsValue>> in this method or new_array.
        unsafe { &mut *(self.hash_props as *mut Vec<JsValue>) }
    }

    /// Safe read access to hash_props vec. Returns None if not allocated.
    pub fn hash_props_vec(&self) -> Option<&Vec<JsValue>> {
        if self.hash_props.is_null() {
            None
        } else {
            // SAFETY: hash_props was set from Box<Vec<JsValue>> in ensure_hash_props/new_array.
            unsafe { Some(&*(self.hash_props as *const Vec<JsValue>)) }
        }
    }

    /// Get property value at position index in the vec.
    /// Returns JsValue::undefined() if hash_props not allocated or position out of bounds.
    pub fn get_prop_at(&self, position: impl PropIndex) -> JsValue {
        if self.hash_props.is_null() {
            return JsValue::undefined();
        }
        // SAFETY: hash_props was set from Box<Vec<JsValue>> in ensure_hash_props/new_array.
        let vec = unsafe { &*(self.hash_props as *const Vec<JsValue>) };
        vec.get(position.to_u32() as usize).copied().unwrap_or(JsValue::undefined())
    }

    /// Set property value at position index. Vec auto-grows if needed.
    pub fn set_prop_at(&mut self, position: impl PropIndex, val: JsValue) {
        let pos = position.to_u32() as usize;
        {
            let vec = self.ensure_hash_props();
            if pos < vec.len() {
                vec[pos] = val;
            } else {
                while vec.len() < pos {
                    vec.push(JsValue::undefined());
                }
                vec.push(val);
            }
        }
        if let Some(meta) = self.prop_meta_vec_mut() {
            while meta.len() <= pos {
                meta.push(None);
            }
        }
    }

    /// Push a value onto hash_props vec. Returns the index position.
    pub fn push_prop(&mut self, val: JsValue) -> u32 {
        let vec = self.ensure_hash_props();
        let pos = vec.len();
        vec.push(val);
        if let Some(meta) = self.prop_meta_vec_mut() {
            meta.push(None);
        }
        pos as u32
    }

    /// Get a pointer to the JsValue at position.
    /// Returns None if hash_props not allocated or position out of bounds.
    pub fn prop_ptr_at(&self, position: impl PropIndex) -> Option<*const JsValue> {
        if self.hash_props.is_null() {
            return None;
        }
        // SAFETY: hash_props was set from Box<Vec<JsValue>> in ensure_hash_props/new_array.
        let vec = unsafe { &*(self.hash_props as *const Vec<JsValue>) };
        vec.get(position.to_u32() as usize).map(|v| v as *const JsValue)
    }

    /// Get the length of hash_props vec (returns 0 if not allocated).
    pub fn prop_vec_len(&self) -> usize {
        if self.hash_props.is_null() {
            0
        } else {
            // SAFETY: hash_props was set from Box<Vec<JsValue>> in ensure_hash_props/new_array.
            unsafe { &*(self.hash_props as *const Vec<JsValue>) }.len()
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
            debug_assert!(!cursor_ptr.is_null(), "prototype cursor pointer must not be null");
            // SAFETY: cursor is known to be an object JsValue, so it encodes a valid JsObject pointer.
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

    pub fn native_fn(&self) -> Option<NativeFnPtr> {
        self.native_fn
    }

    pub fn set_native_fn(&mut self, ptr: Option<NativeFnPtr>) {
        self.native_fn = ptr;
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
        assert!(sz <= 256, "JsObject grew unexpectedly: {sz}B");
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
        assert!(!obj.has_prop_meta());
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
        obj.ensure_hash_props().push(JsValue::int(17));
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
    fn hash_props_flat_storage_roundtrip() {
        let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null());
        obj.set_prop_at(0, JsValue::int(100));
        obj.set_prop_at(1, JsValue::int(200));
        assert_eq!(obj.get_prop_at(0), JsValue::int(100));
        assert_eq!(obj.get_prop_at(1), JsValue::int(200));
    }

    #[test]
    fn prop_meta_lazy_init() {
        let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null());
        obj.set_prop_at(0, JsValue::int(1));
        assert!(!obj.has_prop_meta());

        obj.set_data_meta(0, PropAttributes::new(false, true, false));
        assert!(obj.has_prop_meta());
        let meta = obj.prop_meta_at(0).expect("meta");
        assert!(!meta.is_accessor);
        assert!(!meta.attributes.writable());
        assert!(meta.attributes.enumerable());
        assert!(!meta.attributes.configurable());
    }

    #[test]
    fn accessor_meta_roundtrip_and_alignment() {
        let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null());
        obj.set_prop_at(0, JsValue::int(1));
        obj.set_accessor_meta(2, JsValue::int(10), JsValue::int(11), PropAttributes::new(false, false, true));

        assert_eq!(obj.prop_count(), 3);
        assert!(obj.is_accessor_meta(2));
        let meta = obj.prop_meta_at(2).expect("accessor meta");
        assert_eq!(meta.get, JsValue::int(10));
        assert_eq!(meta.set, JsValue::int(11));
        assert!(!meta.attributes.writable());
        assert!(!meta.attributes.enumerable());
        assert!(meta.attributes.configurable());

        obj.push_prop(JsValue::int(4));
        assert_eq!(obj.prop_meta_vec().expect("meta").len(), obj.prop_vec_len());
    }
}
