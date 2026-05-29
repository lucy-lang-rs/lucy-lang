#![allow(unused)]

use std::collections::HashMap;

use crate::operator::Operator;
use crate::vm::ConstantValue;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TypeId(pub usize);

#[derive(Debug, Clone, PartialEq)]
pub enum Type {
    U8, I8,
    U16, I16,
    U32, I32,
    U64, I64,
    F32, F64,
    USize,
    String,
    Bool,
    Empty,

    Array(Box<Type>),

    TypeVar(String),
    Generic { name: String, args: Vec<Type> },

    Qualified {
        inner:   Box<Type>,
        mutable: bool,
        borrowed: bool,
        moved:    bool,
    },

    Class(TypeId),
    Function(Box<FunctionType>),

    Unknown,
}

impl Type {
    pub fn is_numeric(&self) -> bool {
        matches!(
            self,
            Type::U8  | Type::I8  |
            Type::U16 | Type::I16 |
            Type::U32 | Type::I32 |
            Type::U64 | Type::I64 |
            Type::USize
        )
    }
    pub fn is_numeric_or_float(&self) -> bool {
        self.is_numeric() || matches!(self, Type::F32 | Type::F64)
    }
    pub fn inner(&self) -> &Type {
        match self {
            Type::Qualified { inner, .. } => inner.as_ref(),
            other => other,
        }
    }
    pub fn is_mutable(&self) -> bool {
        match self {
            Type::Qualified { mutable, .. } => *mutable,
            _ => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct FunctionType {
    pub params: Vec<Type>,
    pub return_type: Box<Type>
}

#[derive(Debug)]
pub struct ClassType {
    pub name: String,
    pub fields: Vec<(String, Type, bool)>,
    pub field_index_map: HashMap<String, usize>,
    pub methods: HashMap<String, (usize, usize, FunctionType, bool)>,
    pub operators: HashMap<Operator, (usize, FunctionType)>,
    pub class_proto_constant: Option<ConstantValue>,
}

#[derive(Debug)]
pub struct TypeArena {
    pub classes: Vec<ClassType>,
}

impl TypeArena {
    pub fn new() -> Self {
        Self { classes: Vec::new() }
    }

    pub fn alloc_class(&mut self, class: ClassType) -> TypeId {
        let id = TypeId(self.classes.len());
        self.classes.push(class);
        id
    }

    pub fn get_class(&self, id: TypeId) -> &ClassType {
        &self.classes[id.0]
    }

    pub fn get_class_mut(&mut self, id: TypeId) -> &mut ClassType {
        &mut self.classes[id.0]
    }
}