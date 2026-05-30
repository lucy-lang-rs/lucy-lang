#![allow(unused)]

pub mod parser;
pub mod compiler;
pub mod lexer;
pub mod vm;
pub mod bytecode_debug;
pub mod typechecker;

pub mod ty;
pub mod operator;
pub mod span;
pub mod module;
pub mod lib_std;

pub mod builtin_types;
pub mod lucy_mod;

fn main(){}