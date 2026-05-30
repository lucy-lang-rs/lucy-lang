use std::collections::HashMap;
use crate::ty::{Type, FunctionType, ClassType};
use crate::typechecker::{Namespace as TypeNamespace, TypeCheckerModuleRegistry};
use crate::compiler::{Namespace as RuntimeNamespace, LucyCompiler};
use crate::vm::{RuntimeValue, ConstantValue};

pub struct ModuleFunction {
    pub name:  &'static str,
    pub arity: u8,
    pub func:  fn(Vec<RuntimeValue>) -> RuntimeValue,
    pub ty:    FunctionType,
}

pub struct ModuleField {
    pub name:      &'static str,
    pub ty:        Type,
    pub is_public: bool,
}

pub struct ModuleClass {
    pub name:    &'static str,
    pub fields:  Vec<ModuleField>,
    pub methods: Vec<ModuleFunction>,
}

pub struct Module {
    pub name:      &'static str,
    pub functions: Vec<ModuleFunction>,
    pub classes:   Vec<ModuleClass>,
}

impl Module {
    pub fn new(name: &'static str) -> Self {
        Self { name, functions: vec![], classes: vec![] }
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

    pub fn class(mut self, class: ModuleClass) -> Self {
        self.classes.push(class);
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

        for c in &self.classes {
            // We don't have a real TypeArena here, so register as TypeVar —
            // register_into_type_registry does the full alloc
            ns.types.insert(c.name.to_string(), Type::TypeVar(c.name.to_string()));
        }

        ns
    }

    pub fn register_into_type_registry(&self, registry: &mut TypeCheckerModuleRegistry) {
        let mut ns = self.as_type_namespace();

        for c in &self.classes {
            // Build a real ClassType in the shared type arena via the registry's checker
            // For now we register a stub — caller should use register_into_typechecker
            // for full field/method resolution
            ns.types.insert(c.name.to_string(), Type::TypeVar(c.name.to_string()));
        }

        registry.modules.insert(self.name.to_string(), ns);
    }

    /// Full registration into a TypeChecker — allocates real ClassType entries
    pub fn register_into_typechecker(&self, checker: &mut crate::typechecker::TypeChecker) {
        let mut ns = TypeNamespace::new();

        for f in &self.functions {
            ns.locals.insert(
                f.name.to_string(),
                Type::Function(Box::new(f.ty.clone())),
            );
        }

        for c in &self.classes {
            let mut fields          = Vec::new();
            let mut field_index_map = HashMap::new();
            let mut methods         = HashMap::new();

            for (i, field) in c.fields.iter().enumerate() {
                field_index_map.insert(field.name.to_string(), i);
                fields.push((field.name.to_string(), field.ty.clone(), field.is_public));
            }

            for (i, method) in c.methods.iter().enumerate() {
                methods.insert(
                    method.name.to_string(),
                    (i, 0usize, method.ty.clone(), true),
                );
            }

            let class_id = checker.type_arena.alloc_class(ClassType {
                name:                 c.name.to_string(),
                fields,
                field_index_map,
                methods,
                operators:            HashMap::new(),
                class_proto_constant: None,
            });

            ns.types.insert(c.name.to_string(), Type::Class(class_id));
        }

        checker.module_registry.modules.insert(self.name.to_string(), ns);
    }

    pub fn register_into_compiler_registry(&self, compiler: &mut LucyCompiler) {
        let mut ns = RuntimeNamespace::new();

        for f in &self.functions {
            let idx = compiler.lulib_register_native_fn(f.name, f.arity, f.func);
            ns.locals.insert(f.name.to_string(), (idx, true));
            ns.constants.insert(
                f.name.to_string(),
                ConstantValue::NativeFunctionProto(idx),
            );
        }

        for c in &self.classes {
            let mut fields          = Vec::new();
            let mut field_index_map = HashMap::new();
            let mut methods         = HashMap::new();

            for (i, field) in c.fields.iter().enumerate() {
                field_index_map.insert(field.name.to_string(), i);
                fields.push((field.name.to_string(), field.ty.clone(), field.is_public));
            }

            let field_vis: Vec<bool> = fields.iter().map(|(_, _, p)| *p).collect();

            let class_proto = ConstantValue::ClassProto {
                name:      c.name.to_string(),
                fields:    field_vis,
                methods:   vec![],
                operators: HashMap::new(),
            };

            let class_id = compiler.type_arena.alloc_class(ClassType {
                name:                 c.name.to_string(),
                fields,
                field_index_map,
                methods,
                operators:            HashMap::new(),
                class_proto_constant: Some(class_proto),
            });

            compiler.scopes.define_type(c.name.to_string(), Type::Class(class_id));

            // Register under the module namespace so `use Module.{ClassName}` works
            ns.types.insert(c.name.to_string(), Type::Class(class_id));
        }

        compiler.module_registry.insert(self.name.to_string(), ns);
    }
}