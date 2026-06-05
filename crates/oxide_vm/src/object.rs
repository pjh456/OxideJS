use crate::value::JsValue;

pub type ShapeId = u32;

/// Layout:
///   header: u32 bits
///     [0:23]   shape_id
///     [24:28]  prop_count (5 bits, 0-31)
///     [29]     is_array
///     [30]     is_extensible
///     [31]     is_function
///   inline[0..=3]: 4 x JsValue (32 bytes)
///   overflow: *mut JsValue (8 bytes, null if ≤4 properties)
///   proto: JsValue (8 bytes)
///   generation: u32 (4 bytes)
///   _pad: u32 (4 bytes, 64B total = one cache line)
#[repr(C)]
pub struct JsObject {
    header: u32,
    inline: [JsValue; 4],
    overflow: *mut JsValue,
    proto: JsValue,
    generation: u32,
    _pad: u32,
}

impl JsObject {
    pub fn new_empty(shape_id: ShapeId, proto: JsValue) -> Self {
        Self {
            header: (shape_id & 0x00FF_FFFF) | (1 << 30),
            inline: [JsValue::undefined(); 4],
            overflow: std::ptr::null_mut(),
            proto,
            generation: 1,
            _pad: 0,
        }
    }

    pub fn new_array(shape_id: ShapeId, proto: JsValue, n_elements: usize) -> Self {
        assert!(
            n_elements <= 4,
            "arrays with >4 inline elements not yet supported"
        );
        Self {
            header: (shape_id & 0x00FF_FFFF) | (1 << 30) | (1 << 29),
            inline: [JsValue::undefined(); 4],
            overflow: std::ptr::null_mut(),
            proto,
            generation: 1,
            _pad: 0,
        }
    }

    pub fn shape_id(&self) -> ShapeId {
        self.header & 0x00FF_FFFF
    }

    pub fn set_shape_id(&mut self, id: ShapeId) {
        self.header = (self.header & !0x00FF_FFFF) | (id & 0x00FF_FFFF);
    }

    pub fn prop_count(&self) -> u8 {
        ((self.header >> 24) & 0x1F) as u8
    }

    pub fn set_prop_count(&mut self, count: u8) {
        let count = count.min(31);
        self.header = (self.header & !(0x1F << 24)) | ((count as u32) << 24);
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

    pub fn get_inline_prop(&self, offset: u8) -> JsValue {
        debug_assert!(offset < 4, "inline offset out of range");
        self.inline[offset as usize]
    }

    pub fn set_inline_prop(&mut self, offset: u8, val: JsValue) {
        debug_assert!(offset < 4, "inline offset out of range");
        self.inline[offset as usize] = val;
    }

    pub fn get_overflow_prop(&self, offset: u8) -> JsValue {
        debug_assert!(offset >= 4);
        if self.overflow.is_null() {
            return JsValue::undefined();
        }
        unsafe { *self.overflow.add((offset - 4) as usize) }
    }

    pub fn set_overflow_prop(&mut self, offset: u8, val: JsValue) {
        debug_assert!(offset >= 4);
        debug_assert!(!self.overflow.is_null(), "overflow buffer not allocated");
        unsafe {
            *self.overflow.add((offset - 4) as usize) = val;
        }
    }

    pub fn get_prop(&self, offset: u8) -> JsValue {
        if offset < 4 {
            self.get_inline_prop(offset)
        } else {
            self.get_overflow_prop(offset)
        }
    }

    pub fn set_prop(&mut self, offset: u8, val: JsValue) {
        if offset < 4 {
            self.set_inline_prop(offset, val);
        } else {
            self.set_overflow_prop(offset, val);
        }
    }

    pub fn set_prop_expand(&mut self, offset: u8, val: JsValue, bump: &bumpalo::Bump) {
        if offset >= 4 && (self.overflow.is_null() || offset >= self.prop_count()) {
            self.alloc_overflow(bump, (offset as usize) + 1);
        }
        self.set_prop(offset, val);
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

    pub fn alloc_overflow(&mut self, _bump: &bumpalo::Bump, new_count: usize) -> *mut JsValue {
        use std::alloc::Layout;
        debug_assert!(new_count > 4);
        debug_assert!(new_count <= 32, "overflow limited to 32 slots max");
        let n = new_count - 4;
        let layout = Layout::array::<JsValue>(n).unwrap();
        let ptr: *mut JsValue;
        unsafe {
            ptr = _bump.alloc_layout(layout).as_ptr() as *mut JsValue;
            for i in 0..n {
                *ptr.add(i) = JsValue::undefined();
            }
        }
        self.overflow = ptr;
        ptr
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shape::EMPTY_SHAPE_ID;

    #[test]
    fn object_size_64_bytes() {
        assert_eq!(
            std::mem::size_of::<JsObject>(),
            64,
            "JsObject layout mismatch — expected 64B (one cache line)"
        );
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
        obj.set_prop_count(17);
        assert_eq!(obj.prop_count(), 17);
    }

    #[test]
    fn flags_individual() {
        let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null());
        assert!(obj.is_extensible());
        obj.set_extensible(false);
        assert!(!obj.is_extensible());
    }

    #[test]
    fn inline_prop_read_write() {
        let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null());
        obj.set_inline_prop(0, JsValue::int(42));
        assert_eq!(obj.get_inline_prop(0), JsValue::int(42));
    }

    #[test]
    fn new_array_flags() {
        let obj = JsObject::new_array(5, JsValue::null(), 3);
        assert!(obj.is_array());
        assert_eq!(obj.shape_id(), 5);
    }

    #[test]
    fn generation_bump() {
        let mut obj = JsObject::new_empty(EMPTY_SHAPE_ID, JsValue::null());
        assert_eq!(obj.generation(), 1);
        obj.bump_generation();
        assert_eq!(obj.generation(), 2);
    }
}
