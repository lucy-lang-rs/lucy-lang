//main.rs

use std::env;
use std::fs;

use lucy_lang::lexer;
use lucy_lang::typechecker;
use lucy_lang::bytecode_debug;

use lucy_lang::parser::{LucyParser};
use lucy_lang::compiler::{LucyCompiler};
use lucy_lang::vm::{LucyVM, Closure};

use lucy_lang::lib_std;

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

    let mut checker  = typechecker::TypeChecker::new();
    let mut compiler = LucyCompiler::new(source);

    let stdio = lib_std::stdio_module();
    stdio.register_into_type_registry(&mut checker.module_registry);
    stdio.register_into_compiler_registry(&mut compiler);
    
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