use crate::ty::Type;
use crate::builtin_types::{BuiltinClass, BuiltinField, BuiltinMethod};
use crate::vm::{RuntimeValue, LucyVM, Closure};

use std::fs;
use crate::lexer;
use crate::parser::LucyParser;
use crate::compiler::LucyCompiler;
use crate::typechecker;
use crate::lib_std;

use crate::module::{Module, ModuleClass, ModuleField};

#[derive(Debug, Clone)]
pub struct LucyConfig {
    pub lib_path: String,
}

impl Default for LucyConfig {
    fn default() -> Self {
        Self {
            lib_path: "./src".to_string(),
        }
    }
}

fn runtime_value_to_lucy_config(val: &RuntimeValue) -> LucyConfig {
    let inst = match val {
        RuntimeValue::Instance(rc) => rc.borrow(),
        _ => return LucyConfig::default(),
    };

    // Field order must match the order you registered in lucy_config_class()
    // field 0 = lib_path
    let lib_path = match inst.field_values.get(0) {
        Some(RuntimeValue::String(s)) => s.clone(),
        _ => "./src".to_string(),
    };

    LucyConfig { lib_path }
}

pub fn read_configs(prefix: String) -> LucyConfig {
    let Ok(src) = fs::read_to_string(format!("{}/Lucy.luc", prefix)) else {
        return LucyConfig::default();
    };

    let lucy_module = lucy_config_module();

    let tokens = lexer::tokenize(src.clone());
    let mut parser = LucyParser::new(tokens);
    let ast = parser.parse_file_source();
    if !parser.errors.is_empty() {
        eprintln!("Lucy.luc parse error: {}", parser.errors[0].message);
        return LucyConfig::default();
    }

    let mut checker = typechecker::TypeChecker::new();

    let stdio = lib_std::stdio_module();
    stdio.register_into_type_registry(&mut checker.module_registry);
    lucy_module.register_into_typechecker(&mut checker);
    
    checker.check_program(&ast);
    if !checker.errors.is_empty() {
        eprintln!("Lucy.luc type error: {}", checker.errors[0].message);
        return LucyConfig::default();
    }

    let mut compiler = LucyCompiler::new(src);
    stdio.register_into_compiler_registry(&mut compiler);
    lucy_module.register_into_compiler_registry(&mut compiler);
    
    compiler.compile(&ast);

    let mut vm = LucyVM::new();
    for np in compiler.native_protos.drain(..) {
        vm.native_protos.push(np);
    }
    let root_idx = vm.load_proto(&compiler.root_proto().clone());
    vm.call_closure(Closure { proto_idx: root_idx, upvalues: vec![] }, vec![]);

    match vm.globals.get("config") {
        Some(val) => runtime_value_to_lucy_config(val),
        None => {
            eprintln!("Lucy.luc: no 'global config' found");
            LucyConfig::default()
        }
    }
}

pub fn lucy_config_module() -> Module {
    Module::new("Lucy")
        .class(ModuleClass {
            name: "LucyConfig",
            fields: vec![
                ModuleField {
                    name:      "lib_path",
                    ty:        Type::String,
                    is_public: true,
                },
            ],
            methods: vec![],
        })
}