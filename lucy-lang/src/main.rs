//main.rs

pub mod parser;
pub mod compiler;
pub mod lexer;
pub mod vm;
pub mod bytecode_debug;
pub mod typechecker;

pub mod ty;
pub mod operator;
pub mod span;

use ty::Type;

use std::env;
use std::fs;

use parser::{LucyParser};
use compiler::{LucyCompiler, NamespaceBuilder};
use vm::{RuntimeValue, LucyVM, Closure};

use crate::ty::FunctionType;
use crate::ty::Type::Empty;
use crate::ty::Type::Function;
use crate::ty::Type::Unknown;

fn main()
{
    let mut cli_args = env::args();
    cli_args.next(); // Skip main file name

    let input_file_path = cli_args.next().unwrap();
    let source = fs::read_to_string(input_file_path).unwrap();

    let tokens = lexer::tokenize(source.clone());
    let mut parser = LucyParser::new(tokens);
    
    let program_ast = parser.parse_file_source();
    if parser.errors.len() > 0
    {
        let err = &parser.errors[0];
        panic!("ParseError at {:?}: {}", err.span, err.message);
    }
    println!("{:#?}", program_ast);

    let mut checker = typechecker::TypeChecker::new();
    let mut compiler = LucyCompiler::new(source);

    let stdio_impl = NamespaceBuilder::construct(&mut compiler, |ns| ns
        .function("println", 1, |args| {
            println!("{:?}", args[0]);
            RuntimeValue::Empty
        })
    );
    let stdio_type = typechecker::NamespaceBuilder::construct(|ns| ns
        .member("println", Type::Function(Box::new(FunctionType {
            params: vec![Unknown],
            return_type: Box::new(Empty)
        })))
    );
    
    checker.scopes.define_namespace("stdio".into(), stdio_type);
    compiler.register_native_namespace("stdio", stdio_impl);
    
    checker.check_program(&program_ast);
    if checker.errors.len() > 0
    {
        let err = &checker.errors[0];
        panic!("TypeError at {:?}: {}", err.span, err.message);
    }
    
    compiler.compile(&program_ast);

    bytecode_debug::dump_bytecode(&compiler);

    let mut vm = LucyVM::new();
    for np in compiler.native_protos.drain(..) {
        vm.native_protos.push(np);
    }
    
    let root_proto =
        compiler
            .root_proto()
            .clone();

    let root_idx =
        vm.load_proto(&root_proto);

    let module_closure =
        Closure {
            proto_idx: root_idx,
            upvalues: vec![],
        };

    vm.call_closure(
        module_closure,
        vec![],
    );
}