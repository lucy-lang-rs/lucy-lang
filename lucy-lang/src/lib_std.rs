use crate::module::{Module, ModuleClass, ModuleField};
use crate::ty::{FunctionType, Type};

pub fn stdio_module() -> Module {
    Module::new("stdio")
        .function("println", 1,
            FunctionType { params: vec![Type::Unknown], return_type: Box::new(Type::Empty) },
            |args| { println!("{:?}", args[0]); crate::vm::RuntimeValue::Empty }
        )
        .function("print", 1,
            FunctionType { params: vec![Type::Unknown], return_type: Box::new(Type::Empty) },
            |args| { print!("{:?}", args[0]); crate::vm::RuntimeValue::Empty }
        )
}