// lucy_lang/src/builtin_types.rs

use crate::ty::{Type, FunctionType};

pub struct BuiltinField {
    pub name:      String,
    pub ty:        Type,
    pub is_public: bool,
}

pub struct BuiltinMethod {
    pub name:        String,
    pub params:      Vec<(String, Type)>,  // (param_name, param_type)
    pub return_type: Type,
    pub is_public:   bool,
}

pub struct BuiltinClass {
    pub name:    String,
    pub fields:  Vec<BuiltinField>,
    pub methods: Vec<BuiltinMethod>,
}