#![allow(unused)]

use std::fs;
use std::collections::HashMap;

use lucy_lang::lexer;
use lucy_lang::typechecker::TypeChecker;
use lucy_lang::bytecode_debug;
use lucy_lang::parser::{LucyParser, AstNode, BindingNode, SAst};
use lucy_lang::compiler::{LucyCompiler, Namespace as CcNamespace};
use lucy_lang::typechecker::Namespace as TcNamespace;
use lucy_lang::vm::{LucyVM, Closure};
use lucy_lang::ty::{Type, FunctionType};
use lucy_lang::lib_std;
use lucy_lang::lucy_mod;

struct ParsedModule {
    name: String,
    src:  String,
    ast:  SAst,
}

fn stub_tc_namespace(ast: &SAst) -> TcNamespace {
    let mut ns = TcNamespace::new();
    let stmts = match &ast.node {
        AstNode::Program(s) => s,
        _ => return ns,
    };
    for stmt in stmts {
        let inner = match &stmt.node {
            AstNode::Public(i) => i.as_ref(),
            _ => continue,
        };
        match &inner.node {
            AstNode::FunctionDeclaration { name, params, return_type, .. } => {
                let fn_ty = FunctionType {
                    params: params.iter().map(|p| match p {
                        BindingNode::IdentifierBinding { ty, .. } =>
                            TypeChecker::compile_type_static(ty),
                        _ => Type::Unknown,
                    }).collect(),
                    return_type: Box::new(TypeChecker::compile_type_static(return_type)),
                };
                ns.locals.insert(name.clone(), Type::Function(Box::new(fn_ty)));
            }
            AstNode::ClassDefinition { name, .. } => {
                ns.types.insert(name.clone(), Type::TypeVar(name.clone()));
            }
            AstNode::VarDeclaration { binding, .. } => {
                if let BindingNode::IdentifierBinding { name, ty } = binding {
                    ns.locals.insert(name.clone(), TypeChecker::compile_type_static(ty));
                }
            }
            _ => {}
        }
    }
    ns
}

fn register_native_modules(
    tc_registry: &mut HashMap<String, TcNamespace>,
    cc_registry: &mut HashMap<String, CcNamespace>,
    compiler:    &mut LucyCompiler,
) {
    let stdio = lib_std::stdio_module();
    tc_registry.insert(stdio.name.to_string(), stdio.as_type_namespace());
    stdio.register_into_compiler_registry(compiler);
    cc_registry.insert(stdio.name.to_string(),
        compiler.module_registry.get(stdio.name).cloned().unwrap_or(CcNamespace::new()));
}

fn load_user_modules(lib_path: &str) -> (HashMap<String, TcNamespace>, HashMap<String, CcNamespace>) {
    let mut tc_registry: HashMap<String, TcNamespace> = HashMap::new();
    let mut cc_registry: HashMap<String, CcNamespace> = HashMap::new();

    // Native tc stubs so user modules can type-check against stdio etc.
    let stdio = lib_std::stdio_module();
    tc_registry.insert(stdio.name.to_string(), stdio.as_type_namespace());
    // cc_registry gets stdio registered in main against the real compiler

    let lib = std::path::Path::new(lib_path);
    if !lib.exists() {
        return (tc_registry, cc_registry);
    }

    // ── Collect + parse all .luc files except main.luc ──────────────────────
    let parsed: Vec<ParsedModule> = std::fs::read_dir(lib)
        .expect("failed to read lib_path")
        .filter_map(|e| e.ok())
        .filter(|e| {
            let path = e.path();
            path.extension().map(|x| x == "luc").unwrap_or(false)
                && path.file_stem().map(|s| s != "main").unwrap_or(false)
        })
        .map(|e| {
            let name = e.path().file_stem().unwrap()
                .to_string_lossy().to_string();
            let src = fs::read_to_string(e.path())
                .unwrap_or_else(|_| panic!("failed to read module '{}'", name));
            let tokens = lexer::tokenize(src.clone());
            let mut parser = LucyParser::new(tokens);
            let ast = parser.parse_file_source();
            if !parser.errors.is_empty() {
                panic!("ParseError in module '{}': {}", name, parser.errors[0].message);
            }
            ParsedModule { name, src, ast }
        })
        .collect();

    // ── Pass 1: stub every module so they can all see each other ────────────
    for m in &parsed {
        tc_registry.insert(m.name.clone(), stub_tc_namespace(&m.ast));
        cc_registry.insert(m.name.clone(), CcNamespace::new());
    }

    // ── Pass 2: full typecheck + compile with everyone visible ───────────────
    for m in &parsed {
        let mut checker = TypeChecker::new();
        checker.module_registry.modules = tc_registry.clone();
        checker.check_program(&m.ast);
        if !checker.errors.is_empty() {
            panic!("TypeError in module '{}': {}", m.name, checker.errors[0].message);
        }

        let mut tc_ns = TcNamespace::new();
        if let Some(top) = checker.scopes.scopes.first() {
            for (name, local) in &top.locals {
                tc_ns.locals.insert(name.clone(), local.ty.clone());
            }
            for (name, ty) in &top.types {
                tc_ns.types.insert(name.clone(), ty.clone());
            }
        }
        tc_registry.insert(m.name.clone(), tc_ns);

        let mut compiler = LucyCompiler::new(m.src.clone());
        compiler.module_registry = cc_registry.clone();
        compiler.compile(&m.ast);

        let mut cc_ns = CcNamespace::new();
        if let Some(top) = compiler.scopes.scopes.first() {
            for (name, reg) in &top.exports {
                if let Some(local) = top.locals.get(name) {
                    cc_ns.locals.insert(name.clone(), (*reg, true));
                    if let Some(cv) = &local.backing {
                        cc_ns.constants.insert(name.clone(), cv.clone());
                    }
                }
            }
            for name in &top.exported_types {
                if let Some(ty) = top.types.get(name) {
                    cc_ns.types.insert(name.clone(), ty.clone());
                }
            }
        }
        cc_registry.insert(m.name.clone(), cc_ns);
    }

    (tc_registry, cc_registry)
}

fn main() {
    let config   = lucy_mod::read_configs(".".into());
    let lib_path = config.lib_path;

    let (tc_registry, mut cc_registry) = load_user_modules(&lib_path);

    // ── Entry point ──────────────────────────────────────────────────────────
    let input_file_path = format!("{}/main.luc", lib_path);
    let source = fs::read_to_string(&input_file_path)
        .unwrap_or_else(|_| panic!("no main.luc found in '{}'", lib_path));

    let tokens = lexer::tokenize(source.clone());
    let mut parser = LucyParser::new(tokens);
    let program_ast = parser.parse_file_source();
    if !parser.errors.is_empty() {
        let err = &parser.errors[0];
        panic!("ParseError at {:?}: {}", err.span, err.message);
    }

    let mut checker = TypeChecker::new();
    checker.module_registry.modules = tc_registry;

    let mut compiler = LucyCompiler::new(source);

    // Register natives into the FINAL compiler so vm.native_protos gets filled
    let stdio = lib_std::stdio_module();
    stdio.register_into_compiler_registry(&mut compiler);

    // Merge the rest of cc_registry in after (stdio is already in there from load_user_modules)
    for (name, ns) in cc_registry {
        compiler.module_registry.entry(name).or_insert(ns);
    }

    checker.check_program(&program_ast);
    if !checker.errors.is_empty() {
        let err = &checker.errors[0];
        panic!("TypeError at {:?}: {}", err.span, err.message);
    }

    compiler.compile(&program_ast);
    bytecode_debug::dump_bytecode(&compiler);

    // ── VM ───────────────────────────────────────────────────────────────────
    let mut vm = LucyVM::new();
    for np in compiler.native_protos.drain(..) {
        vm.native_protos.push(np);
    }

    let root_idx = vm.load_proto(&compiler.root_proto().clone());
    vm.call_closure(Closure { proto_idx: root_idx, upvalues: vec![] }, vec![]);
}