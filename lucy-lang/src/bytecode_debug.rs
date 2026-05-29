#![allow(unused)]

use crate::vm::{Opcode, FunctionProto, ConstantValue, unpack_abc, unpack_abx};
use crate::compiler::LucyCompiler;

fn opcode_name(op: u32) -> &'static str {
    match op {
        x if x == Opcode::LOADK    as u32 => "LOADK",
        x if x == Opcode::CALL     as u32 => "CALL",
        x if x == Opcode::RET      as u32 => "RET",
        x if x == Opcode::MOVE     as u32 => "MOVE",
        x if x == Opcode::GETUPVAL as u32 => "GETUPVAL",
        x if x == Opcode::TYCAST as u32 => "TYPECAST",

        x if x == Opcode::NEWCLASS as u32 => "NEWCLASS",
        x if x == Opcode::SETFIELD as u32 => "SETFIELD",
        x if x == Opcode::GETFIELD as u32 => "GETFIELD",
        x if x == Opcode::GETMETHOD as u32 => "GETMETHOD",

        x if x == Opcode::ADD as u32 => "ADD",
        x if x == Opcode::SUB as u32 => "SUB",
        x if x == Opcode::DIV as u32 => "DIV",
        x if x == Opcode::MUL as u32 => "MUL",

        x if x == Opcode::JMP as u32 => "JMP",
        x if x == Opcode::JEQ as u32 => "JEQ",
        x if x == Opcode::JNE as u32 => "JNE",

        x if x == Opcode::LE as u32 => "LE",
        x if x == Opcode::GE as u32 => "GE",

        x if x == Opcode::LT as u32 => "LT",
        x if x == Opcode::GT as u32 => "GT",

        x if x == Opcode::ADDOV as u32 => "ADD OVERLOADED",
        x if x == Opcode::SUBOV as u32 => "SUB OVERLOADED",
        x if x == Opcode::DIVOV as u32 => "DIV OVERLOADED",
        x if x == Opcode::MULOV as u32 => "MUL OVERLOADED",
        _                                  => "???",
    }
}

fn is_abx(op: u32) -> bool {
    op == Opcode::LOADK as u32
}

fn fmt_constant(c: &ConstantValue) -> String {
    match c {
        ConstantValue::Bool(b) => format!("<bool {}>", b),
        ConstantValue::ClassProto {name, ..} => format!("<class {}>", name),
        ConstantValue::U8(n)                  => format!("{}", n),
        ConstantValue::I8(n)                  => format!("{}", n),
        ConstantValue::U16(n)                  => format!("{}", n),
        ConstantValue::I16(n)                  => format!("{}", n),
        ConstantValue::U32(n)                  => format!("{}", n),
        ConstantValue::I32(n)                  => format!("{}", n),
        ConstantValue::U64(n)                  => format!("{}", n),
        ConstantValue::I64(n)                  => format!("{}", n),
        ConstantValue::F32(n)                  => format!("{}", n),
        ConstantValue::F64(f)                => format!("{}", f),
        ConstantValue::String(s)               => format!("\"{}\"", s),
        ConstantValue::Type(t)                 => format!("<type {:?}>", t),
        ConstantValue::FunctionProto(idx)      => format!("<proto {}>", idx),
        ConstantValue::NativeFunctionProto(idx)=> format!("<native {}>", idx),
    }
}

fn dump_proto(proto: &FunctionProto, compiler: &LucyCompiler, depth: usize) {
    let indent = "│  ".repeat(depth);
    let bar    = "─".repeat(52 - depth * 2);

    println!("{}┌{}", indent, bar);
    println!("{}│ proto: {:?}  arity: {}  instructions: {}  constants: {}  nested: {}",
        indent, proto.name, proto.arity,
        proto.code.len(), proto.constants.len(), proto.protos.len(),
    );

    if !proto.constants.is_empty() {
        println!("{}│", indent);
        println!("{}│  constants:", indent);
        for (i, c) in proto.constants.iter().enumerate() {
            println!("{}│    [{:>3}]  {}", indent, i, fmt_constant(c));
        }
    }

    println!("{}│", indent);
    println!("{}│  bytecode:", indent);

    for (pc, &raw) in proto.code.iter().enumerate() {
        let op = raw & 0x3F;
        let name = opcode_name(op);
        let operands = if is_abx(op) {
            let (_, a, bx) = unpack_abx(raw);
            let comment = proto.constants.get(bx as usize)
                .map(|c| format!("  ; {}", fmt_constant(c)))
                .unwrap_or_default();
            format!("A={:<3} Bx={:<5}{}", a, bx, comment)
        } else {
            let (_, a, b, c) = unpack_abc(raw);
            format!("A={:<3} B={:<3}  C={:<3}", a, b, c)
        };
        println!("{}│    [{:>4}]  {:<8}  {}", indent, pc, name, operands);
    }

    if !proto.protos.is_empty() {
        println!("{}│", indent);
        println!("{}│  nested protos:", indent);
        for nested in &proto.protos {
            dump_proto(nested, compiler, depth + 1);
        }
    }

    println!("{}└{}", indent, bar);
}

// ── public entry point ─────────────────────────────────────────────────────

pub fn dump_bytecode(compiler: &LucyCompiler) {
    println!();
    println!("══════════════════════════════════════════════════════");
    println!("  LUCY BYTECODE DUMP");
    println!("══════════════════════════════════════════════════════");

    // ── native protos ──────────────────────────────────────────────────────
    if !compiler.native_protos.is_empty() {
        println!();
        println!("  native functions:");
        for (i, np) in compiler.native_protos.iter().enumerate() {
            println!("    [{:>3}]  {:?}  arity: {}", i, np.name, np.arity);
        }
    }

    // ── native namespaces ──────────────────────────────────────────────────
    if !compiler.native_namespaces.is_empty() {
        println!();
        println!("  native namespaces:");
        for (path, ns) in &compiler.native_namespaces {
            println!("    \"{}\"", path);
            for (name, idx) in &ns.locals {
                println!("      {} → native[{:?}]", name, idx);
            }
            for (child, _) in &ns.children {
                println!("      :: {}", child);
            }
        }
    }

    // ── top-level proto (and everything nested inside) ─────────────────────
    println!();
    if compiler.proto_stack.is_empty() {
        println!("  (no protos — was compile() called?)");
    } else {
        for proto in &compiler.proto_stack {
            dump_proto(proto, compiler, 0);
            println!();
        }
    }

    println!("══════════════════════════════════════════════════════");
    println!();
}