use std::collections::HashMap;
use crate::ty::{Type, FunctionType};
use crate::typechecker::{Namespace as TypeNamespace, TypeCheckerModuleRegistry};
use crate::compiler::{Namespace as RuntimeNamespace, LucyCompiler};
use crate::vm::RuntimeValue;

pub struct ModuleFunction {
    pub name:  &'static str,
    pub arity: u8,
    pub func:  fn(Vec<RuntimeValue>) -> RuntimeValue,
    pub ty:    FunctionType,
}

pub struct Module {
    pub name:      &'static str,
    pub functions: Vec<ModuleFunction>,
}

impl Module {
    pub fn new(name: &'static str) -> Self {
        Self { name, functions: vec![] }
    }

    pub fn function(
        mut self,
        name:  &'static str,
        arity: u8,
        ty:    FunctionType,
        func:  fn(Vec<RuntimeValue>) -> RuntimeValue,
    ) -> Self {
        self.functions.push(ModuleFunction { name, arity, func, ty });
        self
    }

    pub fn as_type_namespace(&self) -> TypeNamespace {
        let mut ns = TypeNamespace::new();
        for f in &self.functions {
            ns.locals.insert(
                f.name.to_string(),
                Type::Function(Box::new(f.ty.clone())),
            );
        }
        ns
    }

    pub fn register_into_type_registry(&self, registry: &mut TypeCheckerModuleRegistry) {
        registry.modules.insert(self.name.to_string(), self.as_type_namespace());
    }

    pub fn register_into_compiler_registry(&self, compiler: &mut LucyCompiler) {
        let mut ns = RuntimeNamespace::new();
        for f in &self.functions {
            let idx = compiler.lulib_register_native_fn(f.name, f.arity, f.func);
            ns.locals.insert(f.name.to_string(), (idx, true));
            ns.constants.insert(
                f.name.to_string(),
                crate::vm::ConstantValue::NativeFunctionProto(idx),
            );
        }
        compiler.module_registry.insert(self.name.to_string(), ns);
    }
}