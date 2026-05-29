#![allow(unused)]

use crate::ty::Type;
use crate::operator::Operator;

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

#[derive(Clone, Debug)]
pub enum UpvalueCellInner {
    Open(usize),          // index into flat register array
    Closed(RuntimeValue), // captured after enclosing frame returned
}

#[derive(Clone, Debug)]
pub struct UpvalueCell(pub Rc<RefCell<UpvalueCellInner>>);

impl UpvalueCell {
    fn new_open(reg: usize) -> Self {
        Self(Rc::new(RefCell::new(UpvalueCellInner::Open(reg))))
    }
    fn close(&self, value: RuntimeValue) {
        *self.0.borrow_mut() = UpvalueCellInner::Closed(value);
    }
}

#[derive(Clone, Debug)]
pub struct Closure {
    pub proto_idx: usize,
    pub upvalues:  Vec<UpvalueCell>,
}

impl PartialEq for Closure {
    fn eq(&self, other: &Self) -> bool { self.proto_idx == other.proto_idx }
}

#[derive(Debug)]
pub struct NativeFunctionProto {
    pub name:  String,
    pub arity: u8,
    pub func:  fn(args: Vec<RuntimeValue>) -> RuntimeValue,
}

#[derive(Debug, Clone)]
pub enum UpvalueSource {
    ParentRegister(usize),
    ParentUpvalue(usize),
}

#[derive(Debug, Clone)]
pub struct UpvalueDescriptor {
    pub name:   String,
    pub source: UpvalueSource,
    pub ty:     Type,
}

#[derive(Clone)]
pub struct FunctionProto {
    pub name:          String,
    pub arity:         u8,
    pub max_regs:      u8,
    pub code:          Vec<u32>,
    pub constants:     Vec<ConstantValue>,
    pub protos:        Vec<FunctionProto>,
    pub upvalues:      Vec<UpvalueDescriptor>,
    pub saved_reg_top: usize,
}

#[repr(u32)]
pub enum Opcode {
    LOADK,
    CALL,
    RET,
    MOVE,
    GETUPVAL,

    JEQ,
    JNE,
    JMP,

    NEWCLASS,
    SETFIELD,
    GETFIELD,
    GETMETHOD,

    ADD,
    SUB,
    MUL,
    DIV,
    POW,
    MOD,

    LOR,
    LAND,

    BOR,
    BAND,
    BLSHIFT,
    BRSHIFT,

    EQ,
    NEQ,
    LE,
    LT,
    GE,
    GT,
    
    NEG,
    LNOT,
    BNOT,

    //
    ADDOV,
    SUBOV,
    MULOV,
    DIVOV,
    POWOV,
    MODOV,

    LOROV,
    LANDOV,

    BOROV,
    BANDOV,
    BLSHIFTOV,
    BRSHIFTOV,

    EQOV,
    NEQOV,
    LEOV,
    LTOV,
    GEOV,
    GTOV,
    
    NEGOV,
    LNOTOV,
    BNOTOV,

    TYCAST,
    TYOF,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ConstantValue {
    U8(u8), I8(i8),
    U16(u16), I16(i16),
    U32(u32), I32(i32),
    U64(u64), I64(i64),
    F32(f32), F64(f64),

    String(String),
    Bool(bool),
    Type(Type),
    FunctionProto(usize),
    NativeFunctionProto(usize),

    ClassProto {
        name: String,
        fields: Vec<bool>,
        methods: Vec<(usize, bool)>,
        operators: HashMap<Operator, usize>
    }
}

#[derive(Clone, Debug)]
pub struct ClassInstance {
    pub class_name: String,

    pub field_values: Vec<RuntimeValue>,
    pub field_visibility: Vec<bool>,

    pub method_table: Vec<(usize, bool)>,
    pub operator_table: HashMap<Operator, usize>,
}

impl PartialEq for ClassInstance {
    fn eq(&self, other: &Self) -> bool { Rc::ptr_eq(&Rc::new(()), &Rc::new(())) }
}

#[derive(Clone, Debug, PartialEq)]
pub enum RuntimeValue {
    U8(u8), I8(i8),
    U16(u16), I16(i16),
    U32(u32), I32(i32),
    U64(u64), I64(i64),
    F32(f32), F64(f64),

    String(String),
    Type(Type),
    Closure(Closure),
    NativeClosure(usize),
    Bool(bool),

    /// A heap-allocated class instance (shared via Rc<RefCell<...>>).
    Instance(Rc<RefCell<ClassInstance>>),

    Empty,
}

impl RuntimeValue {
    /// Coerce to f64 for arithmetic.
    pub fn as_f64(&self) -> f64 {
        match self {
            RuntimeValue::U8(n)  => *n as f64,
            RuntimeValue::I8(n)  => *n as f64,
            RuntimeValue::U16(n) => *n as f64,
            RuntimeValue::I16(n) => *n as f64,
            RuntimeValue::U32(n) => *n as f64,
            RuntimeValue::I32(n) => *n as f64,
            RuntimeValue::U64(n) => *n as f64,
            RuntimeValue::I64(n) => *n as f64,
            RuntimeValue::F32(n) => *n as f64,
            RuntimeValue::F64(n) => *n,
            other => panic!("Cannot coerce {:?} to number", other),
        }
    }

    /// True when the value is an integer kind (not float).
    pub fn is_integer(&self) -> bool {
        matches!(self,
            RuntimeValue::U8(_)  | RuntimeValue::I8(_)  |
            RuntimeValue::U16(_) | RuntimeValue::I16(_) |
            RuntimeValue::U32(_) | RuntimeValue::I32(_) |
            RuntimeValue::U64(_) | RuntimeValue::I64(_)
        )
    }

    /// True when the value is a float kind.
    pub fn is_float(&self) -> bool {
        matches!(self, RuntimeValue::F32(_) | RuntimeValue::F64(_))
    }

    /// Perform arithmetic, preserving the "wider" numeric type of the two operands.
    pub fn arith(lhs: &RuntimeValue, rhs: &RuntimeValue, op: u32) -> RuntimeValue {
        // Float promotion: if either side is a float, use f64 arithmetic
        if lhs.is_float() || rhs.is_float() {
            let l = lhs.as_f64();
            let r = rhs.as_f64();
            let result = match op {
                x if x == Opcode::ADD as u32 => l + r,
                x if x == Opcode::SUB as u32 => l - r,
                x if x == Opcode::MUL as u32 => l * r,
                x if x == Opcode::DIV as u32 => l / r,
                x if x == Opcode::POW as u32 => l.powf(r),
                x if x == Opcode::MOD as u32 => l % r,

                x if x == Opcode::EQ  as u32 => return RuntimeValue::Bool(l == r),
                x if x == Opcode::NEQ as u32 => return RuntimeValue::Bool(l != r),
                x if x == Opcode::LT  as u32 => return RuntimeValue::Bool(l <  r),
                x if x == Opcode::GT  as u32 => return RuntimeValue::Bool(l >  r),
                x if x == Opcode::LE  as u32 => return RuntimeValue::Bool(l <= r),
                x if x == Opcode::GE  as u32 => return RuntimeValue::Bool(l >= r),

                _ => unreachable!(),
            };
            // Preserve F32 if both inputs were F32
            if matches!(lhs, RuntimeValue::F32(_)) && matches!(rhs, RuntimeValue::F32(_)) {
                return RuntimeValue::F32(result as f32);
            }
            return RuntimeValue::F64(result);
        }

        // Integer arithmetic — use i64 as working type, then down-cast to the left-hand type
        let l = lhs.as_f64() as i64;
        let r = rhs.as_f64() as i64;
        let result: i64 = match op {
            x if x == Opcode::ADD as u32 => l.wrapping_add(r),
            x if x == Opcode::SUB as u32 => l.wrapping_sub(r),
            x if x == Opcode::MUL as u32 => l.wrapping_mul(r),
            x if x == Opcode::MOD as u32 => l % r,
            x if x == Opcode::POW as u32 => l.wrapping_pow(r as u32),
            x if x == Opcode::BLSHIFT as u32 => l.wrapping_shl(r as u32),
            x if x == Opcode::BRSHIFT as u32 => l.wrapping_shr(r as u32),
            x if x == Opcode::DIV as u32 => {
                if r == 0 { panic!("Integer division by zero"); }
                l.wrapping_div(r)
            },

            x if x == Opcode::EQ  as u32 => return RuntimeValue::Bool(l == r),
            x if x == Opcode::NEQ as u32 => return RuntimeValue::Bool(l != r),
            x if x == Opcode::LT  as u32 => return RuntimeValue::Bool(l <  r),
            x if x == Opcode::GT  as u32 => return RuntimeValue::Bool(l >  r),
            x if x == Opcode::LE  as u32 => return RuntimeValue::Bool(l <= r),
            x if x == Opcode::GE  as u32 => return RuntimeValue::Bool(l >= r),

            _ => unreachable!(),
        };
        // Mirror the left-hand type
        match lhs {
            RuntimeValue::U8(_)  => RuntimeValue::U8(result as u8),
            RuntimeValue::I8(_)  => RuntimeValue::I8(result as i8),
            RuntimeValue::U16(_) => RuntimeValue::U16(result as u16),
            RuntimeValue::I16(_) => RuntimeValue::I16(result as i16),
            RuntimeValue::U32(_) => RuntimeValue::U32(result as u32),
            RuntimeValue::I32(_) => RuntimeValue::I32(result as i32),
            RuntimeValue::U64(_) => RuntimeValue::U64(result as u64),
            RuntimeValue::I64(_) => RuntimeValue::I64(result),
            _ => unreachable!(),
        }
    }
}

pub fn pack_abc(op: u32, a: u32, b: u32, c: u32) -> u32 {
    (op & 0x3F) | ((a & 0xFF) << 6) | ((b & 0x1FF) << 14) | ((c & 0x1FF) << 23)
}
pub fn pack_abx(op: u32, a: u32, bx: u32) -> u32 {
    (op & 0x3F) | ((a & 0xFF) << 6) | ((bx & 0x3FFFF) << 14)
}
pub fn unpack_abc(instruction: u32) -> (u32, u32, u32, u32) {
    let op = instruction & 0x3F;
    let a  = (instruction >> 6)  & 0xFF;
    let b  = (instruction >> 14) & 0x1FF;
    let c  = (instruction >> 23) & 0x1FF;
    (op, a, b, c)
}
pub fn unpack_abx(instruction: u32) -> (u32, u32, u32) {
    let op = instruction & 0x3F;
    let a  = (instruction >> 6)  & 0xFF;
    let bx = (instruction >> 14) & 0x3FFFF;
    (op, a, bx)
}
pub fn opu32(o: Opcode) -> u32 { o as u32 }
pub struct CallFrame {
    pub closure:    Closure,
    pub pc:         usize,
    pub registers:  Vec<RuntimeValue>,
    pub return_reg: usize,
}

pub struct LucyVM {
    pub protos:        Vec<FunctionProto>,
    pub native_protos: Vec<NativeFunctionProto>,
    pub registers:     Vec<RuntimeValue>,
    pub call_stack:    Vec<CallFrame>,
    pub open_upvalues: Vec<UpvalueCell>,
}

impl LucyVM {
    pub fn new() -> Self {
        Self {
            protos:        vec![],
            native_protos: vec![],
            registers:     vec![RuntimeValue::Empty; 512],
            call_stack:    vec![],
            open_upvalues: vec![],
        }
    }

    pub fn load_proto(
        &mut self,
        proto: &FunctionProto,
    ) -> usize {
        self.load_proto_recursive(proto)
    }

    fn load_proto_recursive(
        &mut self,
        proto: &FunctionProto,
    ) -> usize {
        let nested =
            proto.protos.clone();

        let mut remap =
            Vec::<(usize, usize)>::new();

        for (local_idx, nested_proto)
            in nested.into_iter().enumerate()
        {
            let flat_idx =
                self.load_proto_recursive(
                    &nested_proto
                );

            remap.push((
                local_idx,
                flat_idx,
            ));
        }

        let mut flat_proto =
            proto.clone();

        Self::apply_remap_to_constants(
            &mut flat_proto.constants,
            &remap,
        );

        flat_proto.protos.clear();

        let idx =
            self.protos.len();

        self.protos.push(flat_proto);

        idx
    }

    fn apply_remap_to_constants(constants: &mut Vec<ConstantValue>, remap: &[(usize, usize)]) {
        for c in constants.iter_mut() {
            for &(local_idx, flat_idx) in remap {
                match c {
                    ConstantValue::FunctionProto(idx) if *idx == local_idx => {
                        *idx = flat_idx;
                    }
                    ConstantValue::ClassProto { methods, .. } => {
                        for (idx, _) in methods.iter_mut() {
                            if *idx == local_idx {
                                *idx = flat_idx;
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    pub fn call_closure(&mut self, closure: Closure, args: Vec<RuntimeValue>) -> RuntimeValue {
        let mut regs = vec![RuntimeValue::Empty; 256];
        for (i, arg) in args.into_iter().enumerate() {
            regs[i] = arg;
        }
        self.call_stack.push(CallFrame {
            closure,
            pc: 0,
            registers: regs,
            return_reg: 0,
        });
        self.run()
    }

    fn read_reg(&self, base: usize, reg: u32) -> RuntimeValue {
        self.registers[base + reg as usize].clone()
    }
    fn write_reg(&mut self, base: usize, reg: u32, value: RuntimeValue) {
        self.registers[base + reg as usize] = value;
    }

    fn close_upvalues(&mut self, from_reg: usize) {
        for cell in &self.open_upvalues {
            let should_close = match *cell.0.borrow() {
                UpvalueCellInner::Open(reg) if reg >= from_reg => true,
                _ => false,
            };
            if should_close {
                let reg = match *cell.0.borrow() {
                    UpvalueCellInner::Open(r) => r,
                    _ => unreachable!(),
                };
                let value = self.registers[reg].clone();
                cell.close(value);
            }
        }
        self.open_upvalues.retain(|cell| {
            matches!(*cell.0.borrow(), UpvalueCellInner::Open(_))
        });
    }

    fn current_proto_name(&self) -> &str {
        let frame = self.call_stack.last().expect("empty call stack");
        &self.protos[frame.closure.proto_idx].name
    }

    fn run(&mut self) -> RuntimeValue {
        loop {
            let (proto_idx, pc, return_reg) = {
                let f = self.call_stack.last().expect("empty call stack");
                (f.closure.proto_idx, f.pc, f.return_reg)
            };

            let instr = self.protos[proto_idx]
                .code[pc];
            self.call_stack.last_mut().unwrap().pc += 1;

            let op = instr & 0x3F;

            macro_rules! read {
                ($reg:expr) => { self.call_stack.last().unwrap().registers[$reg as usize].clone() };
            }
            macro_rules! write {
                ($reg:expr, $val:expr) => { self.call_stack.last_mut().unwrap().registers[$reg as usize] = $val; };
            }

            if op == Opcode::LOADK as u32 {
                let (_, a, bx) = unpack_abx(instr);
                let value = match &self.protos[proto_idx].constants[bx as usize] {
                    ConstantValue::Bool(b) => RuntimeValue::Bool(*b),
                    ConstantValue::U8(n)     => RuntimeValue::U8(*n),
                    ConstantValue::I8(n)     => RuntimeValue::I8(*n),
                    ConstantValue::U16(n)    => RuntimeValue::U16(*n),
                    ConstantValue::I16(n)    => RuntimeValue::I16(*n),
                    ConstantValue::U32(n)    => RuntimeValue::U32(*n),
                    ConstantValue::I32(n)    => RuntimeValue::I32(*n),
                    ConstantValue::U64(n)    => RuntimeValue::U64(*n),
                    ConstantValue::I64(n)    => RuntimeValue::I64(*n),
                    ConstantValue::F32(n)    => RuntimeValue::F32(*n),
                    ConstantValue::F64(n)    => RuntimeValue::F64(*n),
                    ConstantValue::String(s) => RuntimeValue::String(s.clone()),
                    ConstantValue::NativeFunctionProto(idx) => RuntimeValue::NativeClosure(*idx),
                    ConstantValue::FunctionProto(idx) => {
                        let idx = *idx;
                        let upvalue_descs = self.protos[idx].upvalues.clone();
                        let mut cells = Vec::new();
                        for desc in &upvalue_descs {
                            let cell = match desc.source {
                                UpvalueSource::ParentRegister(reg) => {
                                    // With per-frame registers, open upvalues track
                                    // (frame_depth, reg_index) — but since we haven't
                                    // changed UpvalueCell yet, close them immediately.
                                    let value = self.call_stack.last().unwrap()
                                        .registers[reg].clone();
                                    let cell = UpvalueCell(Rc::new(RefCell::new(
                                        UpvalueCellInner::Closed(value)
                                    )));
                                    cell
                                }
                                UpvalueSource::ParentUpvalue(uv_idx) => {
                                    self.call_stack.last().unwrap()
                                        .closure.upvalues[uv_idx].clone()
                                }
                            };
                            cells.push(cell);
                        }
                        RuntimeValue::Closure(Closure { proto_idx: idx, upvalues: cells })
                    }
                    ConstantValue::ClassProto { name, .. } => panic!(
                        "LOADK: ClassProto '{}' cannot be loaded as a value; use NEWCLASS", name
                    ),
                    ConstantValue::Type(t) => RuntimeValue::Type(t.clone()),
                };
                write!(a, value);
            }

            else if op == Opcode::MOVE as u32 {
                let (_, a, b, _) = unpack_abc(instr);
                let value = read!(b);
                write!(a, value);
            }

            else if op == Opcode::GETUPVAL as u32 {
                let (_, a, b, _) = unpack_abc(instr);
                let cell = self.call_stack.last().unwrap().closure.upvalues[b as usize].clone();
                let value = match &*cell.0.borrow() {
                    UpvalueCellInner::Open(_)     => panic!("GETUPVAL: upvalue was not closed — open upvalues are not supported with per-frame registers"),
                    UpvalueCellInner::Closed(val) => val.clone(),
                };
                write!(a, value);
            }

            else if op == Opcode::CALL as u32 {
                let (_, a, b, _) = unpack_abc(instr);
                let nargs = b as usize;

                let callee = read!(a);
                let args: Vec<RuntimeValue> = (1..=nargs)
                    .map(|i| read!(a as u32 + i as u32))
                    .collect();

                match callee {
                    RuntimeValue::NativeClosure(native_idx) => {
                        let func = self.native_protos[native_idx].func;
                        let result = func(args);
                        write!(a, result);
                    }
                    RuntimeValue::Closure(closure) => {
                        let mut regs = vec![RuntimeValue::Empty; 256];
                        for (i, arg) in args.into_iter().enumerate() {
                            regs[i] = arg;
                        }
                        self.call_stack.push(CallFrame {
                            closure,
                            pc: 0,
                            registers: regs,
                            return_reg: a as usize,
                        });
                    }
                    other => panic!("Attempt to call non-callable: {:?}", other),
                }
            }

            else if op == Opcode::RET as u32 {
                let (_, a, b, _) = unpack_abc(instr);
                let return_value = if b == 0 {
                    RuntimeValue::Empty
                } else {
                    read!(a)
                };
                self.call_stack.pop();
                if self.call_stack.is_empty() {
                    return return_value;
                }
                self.call_stack.last_mut().unwrap().registers[return_reg] = return_value;
            }

            else if op == Opcode::NEWCLASS as u32 {
                let (_, a, bx) = unpack_abx(instr);
                let (class_name, field_visibility, methods, operators) =
                    match &self.protos[proto_idx].constants[bx as usize] {
                        ConstantValue::ClassProto { name, fields, methods, operators } =>
                            (name.clone(), fields.clone(), methods.clone(), operators.clone()),
                        other => panic!("NEWCLASS: expected ClassProto, got {:?}", other),
                    };
                let field_count = field_visibility.len();
                let instance = Rc::new(RefCell::new(ClassInstance {
                    class_name,
                    field_values: vec![RuntimeValue::Empty; field_count],
                    field_visibility,
                    method_table: methods,
                    operator_table: operators,
                }));
                write!(a, RuntimeValue::Instance(instance));
            }

            else if op == Opcode::SETFIELD as u32 {
                let (_, a, b, c) = unpack_abc(instr);
                let value = read!(b);
                let obj   = read!(a);
                match obj {
                    RuntimeValue::Instance(inst) => {
                        let mut inst = inst.borrow_mut();
                        let field_idx = c as usize;
                        if field_idx >= inst.field_values.len() {
                            panic!("SETFIELD: invalid field index {}", field_idx);
                        }
                        if !inst.field_visibility[field_idx] {
                            let caller = self.current_proto_name();
                            if !caller.starts_with(&format!("{}::", inst.class_name)) {
                                panic!("field #{} is private", field_idx);
                            }
                        }
                        inst.field_values[field_idx] = value;
                    }
                    _ => panic!("SETFIELD: expected instance, got {:?} in reg {}", obj, a),
                }
            }

            else if op == Opcode::GETFIELD as u32 {
                let (_, a, b, c) = unpack_abc(instr);
                let obj = read!(b);
                match obj {
                    RuntimeValue::Instance(inst) => {
                        let inst_ref = inst.borrow();
                        let field_idx = c as usize;
                        if field_idx >= inst_ref.field_values.len() {
                            panic!("GETFIELD: invalid field index {}", field_idx);
                        }
                        if !inst_ref.field_visibility[field_idx] {
                            let caller = self.current_proto_name();
                            if !caller.starts_with(&format!("{}::", inst_ref.class_name)) {
                                panic!("field #{} is private", field_idx);
                            }
                        }
                        let val = inst_ref.field_values[field_idx].clone();
                        write!(a, val);
                    }
                    _ => panic!("GETFIELD: expected instance"),
                }
            }

            else if op == Opcode::GETMETHOD as u32 {
                let (_, a, b, c) = unpack_abc(instr);
                let obj = read!(b);
                match obj {
                    RuntimeValue::Instance(inst) => {
                        let (proto_idx, is_public) = {
                            let inst_ref = inst.borrow();
                            let method_idx = c as usize;
                            if method_idx >= inst_ref.method_table.len() {
                                panic!("GETMETHOD: invalid method index {}", method_idx);
                            }
                            if !inst_ref.method_table[method_idx].1 {
                                let caller = self.current_proto_name();
                                if !caller.starts_with(&format!("{}::", inst_ref.class_name)) {
                                    panic!("method #{} is private", method_idx);
                                }
                            }
                            inst_ref.method_table[method_idx]
                        };
                        let closure = Closure { proto_idx, upvalues: vec![] };
                        write!(a,     RuntimeValue::Closure(closure));
                        write!(a + 1, RuntimeValue::Instance(inst.clone()));
                    }
                    other => panic!("GETMETHOD: expected instance, got {:?}", other),
                }
            }

            else if op == Opcode::ADD  as u32 || op == Opcode::SUB as u32
                || op == Opcode::MUL  as u32 || op == Opcode::DIV as u32
                || op == Opcode::MOD  as u32 || op == Opcode::POW as u32
                || op == Opcode::BLSHIFT as u32 || op == Opcode::BRSHIFT as u32
                || op == Opcode::BNOT as u32 || op == Opcode::BAND as u32
                || op == Opcode::BOR  as u32 || op == Opcode::LNOT as u32
                || op == Opcode::LOR  as u32 || op == Opcode::LAND as u32
                || op == Opcode::EQ   as u32 || op == Opcode::NEQ  as u32
                || op == Opcode::LT   as u32 || op == Opcode::GT   as u32
                || op == Opcode::LE   as u32 || op == Opcode::GE   as u32
            {
                let (_, a, b, c) = unpack_abc(instr);
                let lhs = read!(b);
                let rhs = read!(c);
                let result = RuntimeValue::arith(&lhs, &rhs, op);
                write!(a, result);
            }

            else if op == Opcode::ADDOV as u32 || op == Opcode::SUBOV as u32
                || op == Opcode::MULOV as u32 || op == Opcode::DIVOV as u32
                || op == Opcode::MODOV as u32 || op == Opcode::POWOV as u32
                || op == Opcode::BLSHIFTOV as u32 || op == Opcode::BRSHIFTOV as u32
                || op == Opcode::BNOTOV as u32  || op == Opcode::BANDOV as u32
                || op == Opcode::BOROV as u32   || op == Opcode::LNOTOV as u32
                || op == Opcode::LOROV as u32   || op == Opcode::LANDOV as u32
                || op == Opcode::EQOV  as u32   || op == Opcode::NEQOV as u32
                || op == Opcode::LTOV  as u32   || op == Opcode::GTOV  as u32
                || op == Opcode::LEOV  as u32   || op == Opcode::GEOV  as u32
            {
                let (_, a, b, c) = unpack_abc(instr);
                let operator = if      op == Opcode::ADDOV as u32 { Operator::Add }
                            else if op == Opcode::SUBOV as u32 { Operator::Sub }
                            else if op == Opcode::MULOV as u32 { Operator::Mul }
                            else if op == Opcode::DIVOV as u32 { Operator::Div }
                            else { panic!("Unknown overloaded operator opcode: {}", op) };

                let lhs = read!(b);
                let rhs = read!(c);

                let closure = match &lhs {
                    RuntimeValue::Instance(inst_rc) => {
                        let inst = inst_rc.borrow();
                        let proto_idx = *inst.operator_table
                            .get(&operator)
                            .expect("No operator overload");
                        Closure { proto_idx, upvalues: vec![] }
                    }
                    _ => panic!("No operator overload on non-instance"),
                };

                // Build the new frame's registers directly — no flat array writes
                let mut regs = vec![RuntimeValue::Empty; 256];
                regs[0] = lhs;
                regs[1] = rhs;

                self.call_stack.push(CallFrame {
                    closure,
                    pc: 0,
                    registers: regs,
                    return_reg: a as usize,  // result goes into slot `a` of the *caller's* frame
                });
            }

            else if op == Opcode::TYCAST as u32 {
                let (_, a, b, c) = unpack_abc(instr);
                let src = read!(b);
                let target_ty = match &self.protos[proto_idx].constants[c as usize] {
                    ConstantValue::Type(t) => t.clone(),
                    other => panic!("TYCAST: expected Type constant, got {:?}", other),
                };
                let result = Self::tycast(src, &target_ty);
                write!(a, result);
            }

            else if op == Opcode::JMP as u32 {
                let (_, a, _, _) = unpack_abc(instr);
                // A is a signed offset stored as u32, reinterpret

                let offset = a as i32 - 128;
                let frame = self.call_stack.last_mut().unwrap();
                frame.pc = ((frame.pc as i32 - 1) + offset) as usize;
            }

            else if op == Opcode::JEQ as u32 {
                let (_, a, b, c) = unpack_abc(instr);
                let lhs = read!(b);
                let rhs = read!(c);

                let is_equal = match (lhs, rhs) {
                    (RuntimeValue::Bool(l), RuntimeValue::Bool(r)) => l == r,

                    (RuntimeValue::Bool(l), r) => l == (r.as_f64() != 0.0),
                    (l, RuntimeValue::Bool(r)) => (l.as_f64() != 0.0) == r,

                    (l, r) => l.as_f64() == r.as_f64(),
                };

                if is_equal {
                    let offset = a as i32 - 128;
                    let pc = &mut self.call_stack.last_mut().unwrap().pc;
                    *pc = (*pc as i32 + offset) as usize;
                }
            }

            else if op == Opcode::JNE as u32 {
                let (_, a, b, c) = unpack_abc(instr);
                let lhs = read!(b);
                let rhs = read!(c);
                if lhs != rhs {
                    let offset = a as i32 - 128;
                    let frame = self.call_stack.last_mut().unwrap();
                    frame.pc = ((frame.pc as i32 - 1) + offset) as usize;
                }
            }

            else if op == Opcode::TYOF as u32 {
                let (_, a, b, _) = unpack_abc(instr);
                let val = read!(b);
                let ty = match &val {
                    RuntimeValue::U8(_)           => Type::U8,
                    RuntimeValue::I8(_)           => Type::I8,
                    RuntimeValue::U16(_)          => Type::U16,
                    RuntimeValue::I16(_)          => Type::I16,
                    RuntimeValue::U32(_)          => Type::U32,
                    RuntimeValue::I32(_)          => Type::I32,
                    RuntimeValue::U64(_)          => Type::U64,
                    RuntimeValue::I64(_)          => Type::I64,
                    RuntimeValue::F32(_)          => Type::F32,
                    RuntimeValue::F64(_)          => Type::F64,
                    RuntimeValue::Bool(_)         => Type::Bool,
                    RuntimeValue::String(_)       => Type::String,
                    RuntimeValue::Empty           => Type::Empty,
                    RuntimeValue::Type(inner)         => inner.clone(), // type of a type
                    RuntimeValue::Closure(_)
                    | RuntimeValue::NativeClosure(_) => Type::Unknown,
                    RuntimeValue::Instance(inst)  => {
                        // Return the class name as a TypeVar since we don't have
                        // the TypeId at runtime — the compiler can resolve it if needed.
                        Type::TypeVar(inst.borrow().class_name.clone())
                    }
                };
                write!(a, RuntimeValue::Type(ty));
            }

            else {
                panic!("Unknown opcode: {}", op);
            }
        }
    }

    /// Numeric type cast at runtime.
    fn tycast(src: RuntimeValue, target: &Type) -> RuntimeValue {
        let n = src.as_f64();
        match target {
            Type::U8    => RuntimeValue::U8(n as u8),
            Type::I8    => RuntimeValue::I8(n as i8),
            Type::U16   => RuntimeValue::U16(n as u16),
            Type::I16   => RuntimeValue::I16(n as i16),
            Type::U32   => RuntimeValue::U32(n as u32),
            Type::I32   => RuntimeValue::I32(n as i32),
            Type::U64   => RuntimeValue::U64(n as u64),
            Type::I64   => RuntimeValue::I64(n as i64),
            other => panic!("TYCAST: unsupported target type {:?}", other),
        }
    }
}